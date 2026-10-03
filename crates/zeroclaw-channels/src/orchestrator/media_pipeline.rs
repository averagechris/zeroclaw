//! Automatic media understanding pipeline for inbound channel messages.

use base64::{Engine as _, engine::general_purpose::STANDARD};
use std::borrow::Cow;
use std::time::Duration;
use zeroclaw_config::schema::MediaPipelineConfig;

use super::super::transcription::TranscriptionManager;

// Re-export media types from zeroclaw-types for backwards compatibility.
pub use zeroclaw_api::media::{MarkerKind, MediaAttachment, MediaKind, RenderedMarker};

const VIDEO_MAX_INPUT_BYTES: usize = 20 * 1024 * 1024;
const VIDEO_MAX_DURATION_SECS: u64 = 120;
const VIDEO_MAX_FRAMES: usize = 4;
const VIDEO_FRAME_WIDTH: usize = 640;
const VIDEO_MAX_FRAME_OUTPUT_BYTES: usize = 8 * 1024 * 1024;
const VIDEO_MAX_AUDIO_OUTPUT_BYTES: usize = 1024 * 1024;
const VIDEO_FFMPEG_TIMEOUT: Duration = Duration::from_secs(12);
#[cfg(test)]
const VIDEO_FFMPEG_TEST_TIMEOUT: Duration = Duration::from_millis(100);

/// The media understanding pipeline.
/// Consumes a message's text and attachments, returning enriched text with
/// media annotations prepended.
pub struct MediaPipeline<'a> {
    config: &'a MediaPipelineConfig,
    transcription_manager: Option<&'a TranscriptionManager>,
    vision_available: bool,
    workspace_dir: Option<&'a std::path::Path>,
    ffmpeg_program: std::path::PathBuf,
    ffprobe_program: std::path::PathBuf,
    ffmpeg_timeout: Duration,
}

impl<'a> MediaPipeline<'a> {
    /// Create a new pipeline. `vision_available` indicates whether the current
    /// model provider supports vision (image description). `transcription_manager`
    /// is `None` when transcription is disabled at the channel level — audio
    /// attachments fall back to `[Audio: attached]` annotations.
    pub fn new(
        config: &'a MediaPipelineConfig,
        transcription_manager: Option<&'a TranscriptionManager>,
        vision_available: bool,
    ) -> Self {
        Self {
            config,
            transcription_manager,
            vision_available,
            workspace_dir: None,
            ffmpeg_program: "ffmpeg".into(),
            ffprobe_program: "ffprobe".into(),
            ffmpeg_timeout: VIDEO_FFMPEG_TIMEOUT,
        }
    }

    /// Store unmarked image uploads under the workspace selected for the
    /// already-resolved agent. The returned path can be reused by later tools
    /// in that agent without granting another routed chat access to it.
    pub fn with_workspace_dir(mut self, workspace_dir: &'a std::path::Path) -> Self {
        self.workspace_dir = Some(workspace_dir);
        self
    }

    #[cfg(test)]
    fn with_ffmpeg_program(mut self, program: std::path::PathBuf) -> Self {
        self.ffmpeg_program = program;
        self
    }

    #[cfg(test)]
    fn with_ffprobe_program(mut self, program: std::path::PathBuf) -> Self {
        self.ffprobe_program = program;
        self
    }

    #[cfg(test)]
    fn with_ffmpeg_timeout(mut self, timeout: Duration) -> Self {
        self.ffmpeg_timeout = timeout;
        self
    }

    /// Process a message's attachments and return enriched text.
    /// If the pipeline is disabled via config, returns `original_text` unchanged.
    pub async fn process(&self, original_text: &str, attachments: &[MediaAttachment]) -> String {
        if !self.config.enabled || attachments.is_empty() {
            return original_text.to_string();
        }

        let mut text = original_text.to_string();
        let mut annotations = Vec::new();

        for attachment in attachments {
            // Discord can render an image URL when saving the downloaded
            // bytes fails. The typed envelope still owns those bytes, so
            // replace exactly one channel-rendered URL marker with the inline
            // representation this pipeline is about to add. Removing from
            // the end preserves a sender-authored marker with the same URL in
            // the message caption.
            if self.vision_available
                && self.config.describe_images
                && let Some(target) = attachment.channel_rendered_remote_image_target()
            {
                text = remove_last_channel_image_marker(&text, target);
            }

            // A channel that saved these bytes and rendered a marker for them
            // has already classified this attachment, with more to go on than
            // the pipeline has: the payload, the sender's declared type, and
            // the transport's own notion of what was sent. When it committed to
            // an image or a document disposition, that verdict wins outright.
            //
            // Skipping the whole attachment — not just its image branch — is
            // what keeps the two classifiers from contradicting each other.
            // `kind()` resolves a single kind with the declared MIME first, so
            // a `photo.jpg` sent as `video/mp4` routes to video here while the
            // channel marked it an image; annotating it would put an image
            // marker and a `[Video: ...]` note on one attachment. It also stops
            // a channel-rendered `[Document: ...]` for a non-loadable image
            // (HEIC, TIFF, SVG, BMP) from being re-decided as an image by
            // `kind()` and gaining an `[IMAGE:data:...]` copy the provider
            // rejects. Deferring likewise avoids a second, base64-inlined copy
            // of an image the marker already carries, which would send it to
            // the provider twice and persist megabytes of base64 into session
            // history (the current turn is stored verbatim; only older turns
            // get inline payloads collapsed).
            //
            // The verdict is read from the typed envelope, not by scanning the
            // rendered text: the text also carries sender-authored content, so
            // a caption that types `[IMAGE:/other.jpg]` has no channel
            // provenance and cannot suppress a real attachment.
            if attachment.channel_rendered_owned_disposition() {
                continue;
            }

            match attachment.kind() {
                MediaKind::Audio if self.config.transcribe_audio => {
                    let annotation = self.process_audio(attachment).await;
                    annotations.push(annotation);
                }
                MediaKind::Image if self.config.describe_images => {
                    let annotation = self.process_image(attachment).await;
                    annotations.push(annotation);
                }
                MediaKind::Video if self.config.summarize_video => {
                    let annotation = self.process_video(attachment).await;
                    annotations.push(annotation);
                }
                _ => {}
            }
        }

        if annotations.is_empty() {
            return original_text.to_string();
        }

        let mut enriched = String::with_capacity(
            annotations.iter().map(|a| a.len() + 1).sum::<usize>() + original_text.len() + 2,
        );

        for annotation in &annotations {
            enriched.push_str(annotation);
            enriched.push('\n');
        }

        if !text.is_empty() {
            enriched.push('\n');
            enriched.push_str(&text);
        }

        enriched.trim().to_string()
    }

    /// Transcribe an audio attachment using the existing transcription infra.
    async fn process_audio(&self, attachment: &MediaAttachment) -> String {
        let Some(manager) = self.transcription_manager else {
            return "[Audio transcription unavailable. Do not guess the recording's contents; tell the user transcription is unavailable and ask them to resend it as text.]".to_string();
        };

        match manager
            .transcribe(&attachment.data, &attachment.file_name)
            .await
        {
            Ok(text) => {
                let trimmed = text.trim();
                if trimmed.is_empty() {
                    "[Audio transcription: (empty)]".to_string()
                } else {
                    format!("[Audio transcription: {trimmed}]")
                }
            }
            Err(err) => {
                ::zeroclaw_log::record!(WARN, ::zeroclaw_log::Event::new(module_path!(), ::zeroclaw_log::Action::Note).with_outcome(::zeroclaw_log::EventOutcome::Unknown).with_attrs(::serde_json::json!({"file": attachment.file_name, "error": format!("{}", err)})), "Media pipeline: audio transcription failed");
                "[Audio transcription failed. Do not guess the recording's contents; tell the user and ask them to resend it as text.]".to_string()
            }
        }
    }

    async fn process_image(&self, attachment: &MediaAttachment) -> String {
        if let Some(workspace_dir) = self.workspace_dir
            && let Ok(path) = persist_image_attachment(workspace_dir, attachment).await
        {
            let path = path.display();
            if self.vision_available && attachment.provider_loadable_image_mime().is_some() {
                return format!("[Image saved for this agent: {path}]\n[IMAGE:{path}]");
            }
            return format!("[Image saved for this agent: {path}]");
        }

        if self.vision_available {
            let (mime, data) = image_payload_for_vision(attachment);
            let b64 = STANDARD.encode(data.as_ref());
            format!(
                "[Image: {} attached, will be processed by vision model]\n[IMAGE:data:{};base64,{}]",
                attachment.file_name, mime, b64
            )
        } else {
            format!("[Image: {} attached]", attachment.file_name)
        }
    }

    /// Extract a few bounded video frames for vision and a bounded audio track
    /// for the resolved agent's transcription provider.
    async fn process_video(&self, attachment: &MediaAttachment) -> String {
        if attachment.data.len() > VIDEO_MAX_INPUT_BYTES {
            return "[Video exceeds the 20 MiB processing limit. Tell the user to send a smaller clip.]".to_string();
        }

        let Some(workspace_dir) = self.workspace_dir else {
            return "[Video could not be inspected because this agent has no media workspace.]"
                .to_string();
        };
        let workspace_root = match tokio::fs::canonicalize(workspace_dir).await {
            Ok(path) => path,
            Err(_) => {
                return "[Video could not be inspected because this agent's media workspace is unavailable.]"
                    .to_string();
            }
        };
        let tempdir = match tempfile::Builder::new()
            .prefix("telegram-video-")
            .tempdir_in(&workspace_root)
        {
            Ok(tempdir) => tempdir,
            Err(_) => {
                return "[Video could not be inspected because temporary media storage is unavailable.]"
                    .to_string();
            }
        };
        let extension = safe_video_extension(attachment);
        let input_format = safe_video_demuxer(extension);
        let input_path = tempdir.path().join(format!("input.{extension}"));
        if let Err(error) = tokio::fs::write(&input_path, &attachment.data).await {
            ::zeroclaw_log::record!(
                WARN,
                ::zeroclaw_log::Event::new(module_path!(), ::zeroclaw_log::Action::Note)
                    .with_outcome(::zeroclaw_log::EventOutcome::Unknown)
                    .with_attrs(::serde_json::json!({"error": format!("{error}")})),
                "Media pipeline: failed to stage video under the resolved agent workspace"
            );
            return "[Video could not be inspected because temporary media storage failed.]"
                .to_string();
        }
        let info = match probe_video(
            &self.ffprobe_program,
            &input_path,
            input_format,
            self.ffmpeg_timeout,
        )
        .await
        {
            Ok(info) => info,
            Err(error) => {
                ::zeroclaw_log::record!(
                    WARN,
                    ::zeroclaw_log::Event::new(module_path!(), ::zeroclaw_log::Action::Note)
                        .with_outcome(::zeroclaw_log::EventOutcome::Unknown)
                        .with_attrs(::serde_json::json!({"error": format!("{error:#}")})),
                    "Media pipeline: video metadata probe failed"
                );
                return "[Video could not be inspected. Do not guess its contents; tell the user the clip could not be processed.]"
                    .to_string();
            }
        };
        if info.duration_secs > VIDEO_MAX_DURATION_SECS as f64 {
            return "[Video exceeds the 120 second processing limit. Tell the user to send a shorter clip.]"
                .to_string();
        }

        let mut annotations = Vec::new();
        match persist_video_attachment(workspace_dir, attachment).await {
            Ok(path) => annotations.push(format!(
                "[Original video saved for this agent: {}]",
                path.display()
            )),
            Err(error) => {
                ::zeroclaw_log::record!(
                    WARN,
                    ::zeroclaw_log::Event::new(module_path!(), ::zeroclaw_log::Action::Note)
                        .with_outcome(::zeroclaw_log::EventOutcome::Unknown)
                        .with_attrs(::serde_json::json!({"error": format!("{error:#}")})),
                    "Media pipeline: could not persist video in agent workspace"
                );
                annotations
                    .push("[Original video could not be saved for later editing.]".to_string());
            }
        }
        if self.vision_available {
            match extract_video_frames(
                &self.ffmpeg_program,
                &input_path,
                input_format,
                info.duration_secs,
                self.ffmpeg_timeout,
            )
            .await
            {
                Ok(frames) => {
                    let mut frame_annotations = Vec::new();
                    for (index, frame) in frames.iter().enumerate() {
                        let frame_attachment = MediaAttachment {
                            file_name: format!("video_frame_{index}.jpg"),
                            data: frame.clone(),
                            mime_type: Some("image/jpeg".to_string()),
                            marker: None,
                        };
                        match persist_image_attachment(workspace_dir, &frame_attachment).await {
                            Ok(path) => {
                                let path = path.display().to_string();
                                frame_annotations.push(format!(
                                    "[Video frame {} of {}]\n[Image saved for this agent: {path}]\n[IMAGE:{path}]",
                                    index + 1,
                                    frames.len(),
                                ));
                            }
                            Err(error) => {
                                ::zeroclaw_log::record!(
                                    WARN,
                                    ::zeroclaw_log::Event::new(
                                        module_path!(),
                                        ::zeroclaw_log::Action::Note,
                                    )
                                    .with_outcome(::zeroclaw_log::EventOutcome::Unknown)
                                    .with_attrs(
                                        ::serde_json::json!({"error": format!("{error:#}")})
                                    ),
                                    "Media pipeline: could not persist video frame in agent workspace"
                                );
                                frame_annotations.push(
                                    "[Video frames could not be saved for vision. Tell the user the clip could not be inspected.]".to_string(),
                                );
                                break;
                            }
                        }
                    }
                    if frame_annotations
                        .iter()
                        .any(|annotation| annotation.starts_with("[Video frame "))
                    {
                        annotations.push(
                            "When making a meme from this clip, use one of these saved frame paths as an image_gen reference.".to_string(),
                        );
                    }
                    annotations.extend(frame_annotations);
                }
                Err(error) => {
                    ::zeroclaw_log::record!(
                        WARN,
                        ::zeroclaw_log::Event::new(module_path!(), ::zeroclaw_log::Action::Note)
                            .with_outcome(::zeroclaw_log::EventOutcome::Unknown)
                            .with_attrs(::serde_json::json!({"error": format!("{error:#}")})),
                        "Media pipeline: video frame extraction failed"
                    );
                    annotations.push(
                        "[Video frames could not be extracted. Do not guess the visual content; tell the user the clip could not be inspected.]".to_string(),
                    );
                }
            }
        }

        if info.has_audio {
            match extract_video_audio(
                &self.ffmpeg_program,
                &input_path,
                input_format,
                info.duration_secs,
                self.ffmpeg_timeout,
            )
            .await
            {
                Ok(audio) => {
                    let Some(manager) = self.transcription_manager else {
                        annotations.push(
                        "[Video audio transcription unavailable. Do not guess what was said; tell the user transcription is unavailable.]".to_string(),
                    );
                        return annotations.join("\n");
                    };
                    match manager.transcribe(&audio, "video-audio.mp3").await {
                        Ok(transcript) if !transcript.trim().is_empty() => {
                            annotations.push(format!(
                                "[Video audio transcription: {}]",
                                transcript.trim()
                            ));
                        }
                        Ok(_) => annotations
                            .push("[Video audio contained no recognizable speech.]".to_string()),
                        Err(error) => {
                            ::zeroclaw_log::record!(
                                WARN,
                                ::zeroclaw_log::Event::new(
                                    module_path!(),
                                    ::zeroclaw_log::Action::Note
                                )
                                .with_outcome(::zeroclaw_log::EventOutcome::Unknown)
                                .with_attrs(::serde_json::json!({"error": format!("{error:#}")})),
                                "Media pipeline: video audio transcription failed"
                            );
                            annotations.push(
                            "[Video audio transcription failed. Do not guess what was said; tell the user and ask them to resend the speech as a voice note.]".to_string(),
                        );
                        }
                    }
                }
                Err(error) => {
                    ::zeroclaw_log::record!(
                        WARN,
                        ::zeroclaw_log::Event::new(module_path!(), ::zeroclaw_log::Action::Note)
                            .with_outcome(::zeroclaw_log::EventOutcome::Unknown)
                            .with_attrs(::serde_json::json!({"error": format!("{error:#}")})),
                        "Media pipeline: video audio extraction failed"
                    );
                    annotations.push(
                    "[Video audio could not be extracted. Do not guess what was said; tell the user the audio could not be processed.]".to_string(),
                );
                }
            }
        }

        if annotations.is_empty() {
            format!("[Video: {} attached]", attachment.file_name)
        } else {
            annotations.join("\n")
        }
    }
}

fn safe_video_extension(attachment: &MediaAttachment) -> &'static str {
    match attachment
        .file_name
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase())
        .as_deref()
    {
        Some("mov") => "mov",
        Some("mkv") => "mkv",
        Some("webm") => "webm",
        Some("mp4") => "mp4",
        _ => match attachment.mime_type.as_deref() {
            Some("video/quicktime") => "mov",
            Some("video/x-matroska") => "mkv",
            Some("video/webm") => "webm",
            _ => "mp4",
        },
    }
}

fn safe_video_demuxer(extension: &str) -> &'static str {
    match extension {
        "mp4" | "mov" => "mov",
        "mkv" | "webm" => "matroska",
        _ => "mov",
    }
}

#[derive(Debug)]
struct VideoInfo {
    duration_secs: f64,
    has_audio: bool,
}

async fn probe_video(
    ffprobe: &std::path::Path,
    input_path: &std::path::Path,
    input_format: &str,
    timeout: Duration,
) -> anyhow::Result<VideoInfo> {
    let input_path = input_path.to_string_lossy();
    let mut args = vec!["-v", "error"];
    append_video_input_options(&mut args, input_format, &input_path);
    args.extend([
        "-show_entries",
        "format=duration:stream=codec_type,duration",
        "-of",
        "json",
    ]);
    let output = run_media_command(ffprobe, &args, 16 * 1024, timeout, "ffprobe").await?;
    let metadata: serde_json::Value = serde_json::from_slice(&output)?;
    let streams = metadata
        .get("streams")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    let duration_secs = metadata
        .pointer("/format/duration")
        .and_then(serde_json::Value::as_str)
        .and_then(|duration| duration.parse::<f64>().ok())
        .or_else(|| {
            streams
                .iter()
                .filter_map(|stream| stream.get("duration"))
                .filter_map(serde_json::Value::as_str)
                .filter_map(|duration| duration.parse::<f64>().ok())
                .reduce(f64::max)
        })
        .filter(|duration| duration.is_finite() && *duration > 0.0)
        .ok_or_else(|| anyhow::Error::msg("ffprobe did not report a valid video duration"))?;
    let has_audio = streams.iter().any(|stream| {
        stream.get("codec_type").and_then(serde_json::Value::as_str) == Some("audio")
    });
    anyhow::ensure!(
        streams.iter().any(|stream| {
            stream.get("codec_type").and_then(serde_json::Value::as_str) == Some("video")
        }),
        "ffprobe found no video stream"
    );
    Ok(VideoInfo {
        duration_secs,
        has_audio,
    })
}

async fn extract_video_frames(
    ffmpeg: &std::path::Path,
    input_path: &std::path::Path,
    input_format: &str,
    duration_secs: f64,
    timeout: Duration,
) -> anyhow::Result<Vec<Vec<u8>>> {
    let input_path = input_path.to_string_lossy();
    let duration_arg = duration_secs.to_string();
    let frames_arg = VIDEO_MAX_FRAMES.to_string();
    let frame_rate = format!("{VIDEO_MAX_FRAMES}/{duration_secs}");
    let scale_filter = format!(
        "fps={frame_rate},scale=w=min({VIDEO_FRAME_WIDTH}\\,iw):h=min({VIDEO_FRAME_WIDTH}\\,ih):force_original_aspect_ratio=decrease"
    );
    let mut args = vec![
        "-hide_banner",
        "-loglevel",
        "error",
        "-threads",
        "1",
        "-max_pixels",
        "8294400",
    ];
    append_video_input_options(&mut args, input_format, &input_path);
    args.extend([
        "-t",
        &duration_arg,
        "-map",
        "0:v:0",
        "-vf",
        &scale_filter,
        "-frames:v",
        &frames_arg,
        "-f",
        "image2pipe",
        "-c:v",
        "mjpeg",
        "-q:v",
        "5",
        "pipe:1",
    ]);
    let output = run_media_command(
        ffmpeg,
        &args,
        VIDEO_MAX_FRAME_OUTPUT_BYTES,
        timeout,
        "ffmpeg",
    )
    .await?;
    let frames = split_mjpeg_frames(&output);
    anyhow::ensure!(!frames.is_empty(), "ffmpeg returned no video frames");
    anyhow::ensure!(
        frames.len() <= VIDEO_MAX_FRAMES,
        "ffmpeg returned too many video frames"
    );
    Ok(frames)
}

async fn extract_video_audio(
    ffmpeg: &std::path::Path,
    input_path: &std::path::Path,
    input_format: &str,
    duration_secs: f64,
    timeout: Duration,
) -> anyhow::Result<Vec<u8>> {
    let input_path = input_path.to_string_lossy();
    let duration_arg = duration_secs.to_string();
    let mut args = vec!["-hide_banner", "-loglevel", "error", "-threads", "1"];
    append_video_input_options(&mut args, input_format, &input_path);
    args.extend([
        "-t",
        &duration_arg,
        "-map",
        "0:a:0",
        "-vn",
        "-ac",
        "1",
        "-ar",
        "16000",
        "-b:a",
        "32k",
        "-f",
        "mp3",
        "pipe:1",
    ]);
    run_media_command(
        ffmpeg,
        &args,
        VIDEO_MAX_AUDIO_OUTPUT_BYTES,
        timeout,
        "ffmpeg",
    )
    .await
}

fn append_video_input_options<'a>(
    args: &mut Vec<&'a str>,
    input_format: &'a str,
    input_path: &'a str,
) {
    args.extend(["-protocol_whitelist", "file,pipe", "-f", input_format]);
    if input_format == "mov" {
        args.extend(["-enable_drefs", "0", "-use_absolute_path", "0"]);
    }
    args.extend(["-i", input_path]);
}

async fn run_media_command(
    program: &std::path::Path,
    args: &[&str],
    max_output_bytes: usize,
    timeout: Duration,
    program_name: &str,
) -> anyhow::Result<Vec<u8>> {
    use std::process::Stdio;
    use tokio::io::AsyncReadExt as _;
    use tokio::process::Command;

    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    let mut stdout = child.stdout.take().expect("stdout is piped");
    let read = async move {
        let mut output = Vec::new();
        let mut chunk = [0u8; 16 * 1024];
        loop {
            let len = stdout.read(&mut chunk).await?;
            if len == 0 {
                break;
            }
            anyhow::ensure!(
                output.len().saturating_add(len) <= max_output_bytes,
                "{program_name} output exceeded its size limit"
            );
            output.extend_from_slice(&chunk[..len]);
        }
        Ok::<Vec<u8>, anyhow::Error>(output)
    };
    let run = async {
        let wait = async { child.wait().await.map_err(anyhow::Error::from) };
        let (output, status) = tokio::try_join!(read, wait)?;
        anyhow::ensure!(status.success(), "{program_name} exited unsuccessfully");
        Ok::<Vec<u8>, anyhow::Error>(output)
    };
    tokio::time::timeout(timeout, run)
        .await
        .map_err(|_| anyhow::Error::msg(format!("{program_name} timed out after {timeout:?}")))?
}

fn split_mjpeg_frames(data: &[u8]) -> Vec<Vec<u8>> {
    let mut frames = Vec::new();
    let mut start = None;
    let mut index = 0;
    while index + 1 < data.len() {
        match (data[index], data[index + 1]) {
            (0xff, 0xd8) if start.is_none() => start = Some(index),
            (0xff, 0xd9) if start.is_some() => {
                let frame_start = start.take().expect("frame start exists");
                frames.push(data[frame_start..index + 2].to_vec());
                if frames.len() == VIDEO_MAX_FRAMES {
                    break;
                }
            }
            _ => {}
        }
        index += 1;
    }
    frames
}

/// Persist an image attachment using the media writer scoped to the resolved
/// agent's private workspace.
async fn persist_image_attachment(
    workspace_dir: &std::path::Path,
    attachment: &MediaAttachment,
) -> anyhow::Result<std::path::PathBuf> {
    let extension = attachment
        .file_name
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .filter(|ext| matches!(ext.as_str(), "jpg" | "jpeg" | "png" | "webp" | "gif"))
        .unwrap_or_else(|| "img".to_string());
    persist_media_bytes(workspace_dir, &attachment.data, &extension).await
}

async fn persist_video_attachment(
    workspace_dir: &std::path::Path,
    attachment: &MediaAttachment,
) -> anyhow::Result<std::path::PathBuf> {
    persist_media_bytes(
        workspace_dir,
        &attachment.data,
        safe_video_extension(attachment),
    )
    .await
}

/// Write supported media under the agent's canonical workspace. Generated
/// names and create-new preserve existing source files and avoid path input.
async fn persist_media_bytes(
    workspace_dir: &std::path::Path,
    data: &[u8],
    extension: &str,
) -> anyhow::Result<std::path::PathBuf> {
    use tokio::io::AsyncWriteExt as _;

    anyhow::ensure!(
        matches!(
            extension,
            "jpg"
                | "jpeg"
                | "png"
                | "webp"
                | "gif"
                | "img"
                | "mp4"
                | "mov"
                | "mkv"
                | "avi"
                | "webm"
        ),
        "unsupported media extension"
    );

    let workspace_root = tokio::fs::canonicalize(workspace_dir).await?;
    let media_dir = workspace_dir.join("telegram_files");
    tokio::fs::create_dir_all(&media_dir).await?;
    let media_root = tokio::fs::canonicalize(&media_dir).await?;
    anyhow::ensure!(
        media_root.starts_with(&workspace_root),
        "agent media directory escapes its workspace"
    );

    let path = media_root.join(format!("telegram_{}.{}", uuid::Uuid::new_v4(), extension));
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .await?;
    if let Err(error) = file.write_all(data).await {
        drop(file);
        let _ = tokio::fs::remove_file(&path).await;
        return Err(error.into());
    }
    if let Err(error) = file.flush().await {
        drop(file);
        let _ = tokio::fs::remove_file(&path).await;
        return Err(error.into());
    }
    Ok(path)
}

fn remove_last_channel_image_marker(text: &str, target: &str) -> String {
    let marker = format!("[IMAGE:{target}]");
    let Some(start) = text.rfind(&marker) else {
        return text.to_string();
    };
    let end = start + marker.len();
    let mut cleaned = String::with_capacity(text.len() - marker.len());
    cleaned.push_str(&text[..start]);
    cleaned.push_str(&text[end..]);
    cleaned
}

fn image_payload_for_vision(attachment: &MediaAttachment) -> (String, Cow<'_, [u8]>) {
    let mime = attachment.mime_type.as_deref().unwrap_or("image/jpeg");

    #[cfg(feature = "image-normalization")]
    if is_webp_attachment(attachment, mime) {
        match webp_to_png(&attachment.data) {
            Ok(png) => return ("image/png".to_string(), Cow::Owned(png)),
            Err(err) => {
                ::zeroclaw_log::record!(
                    WARN,
                    ::zeroclaw_log::Event::new(module_path!(), ::zeroclaw_log::Action::Note)
                        .with_outcome(::zeroclaw_log::EventOutcome::Unknown)
                        .with_attrs(::serde_json::json!({
                            "file": attachment.file_name,
                            "error": format!("{}", err),
                            "error_key": "media_pipeline_webp_to_png_failed",
                        })),
                    "Media pipeline: failed to normalize WebP image for vision"
                );
            }
        }
    }

    (mime.to_string(), Cow::Borrowed(&attachment.data))
}

#[cfg(feature = "image-normalization")]
fn is_webp_attachment(attachment: &MediaAttachment, mime: &str) -> bool {
    mime.eq_ignore_ascii_case("image/webp")
        || attachment
            .file_name
            .rsplit_once('.')
            .is_some_and(|(_, ext)| ext.eq_ignore_ascii_case("webp"))
}

#[cfg(feature = "image-normalization")]
fn webp_to_png(data: &[u8]) -> anyhow::Result<Vec<u8>> {
    let image = image::load_from_memory_with_format(data, image::ImageFormat::WebP)?;
    let mut cursor = std::io::Cursor::new(Vec::new());
    image.write_to(&mut cursor, image::ImageFormat::Png)?;
    Ok(cursor.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::Digest as _;

    fn default_pipeline_config(enabled: bool) -> MediaPipelineConfig {
        MediaPipelineConfig {
            enabled,
            transcribe_audio: true,
            describe_images: true,
            summarize_video: true,
        }
    }

    fn sample_audio() -> MediaAttachment {
        MediaAttachment {
            file_name: "voice.ogg".to_string(),
            data: vec![0u8; 100],
            mime_type: Some("audio/ogg".to_string()),
            marker: None,
        }
    }

    fn sample_image() -> MediaAttachment {
        MediaAttachment {
            file_name: "photo.jpg".to_string(),
            data: vec![0u8; 50],
            mime_type: Some("image/jpeg".to_string()),
            marker: None,
        }
    }

    fn sample_video() -> MediaAttachment {
        MediaAttachment {
            file_name: "clip.mp4".to_string(),
            data: vec![0u8; 200],
            mime_type: Some("video/mp4".to_string()),
            marker: None,
        }
    }

    fn generate_mp4_fixture(
        duration_secs: u64,
        with_audio: bool,
        width: u32,
        height: u32,
    ) -> Option<(tempfile::TempDir, std::path::PathBuf)> {
        generate_video_fixture(duration_secs, with_audio, width, height, "mp4")
    }

    fn generate_video_fixture(
        duration_secs: u64,
        with_audio: bool,
        width: u32,
        height: u32,
        container: &str,
    ) -> Option<(tempfile::TempDir, std::path::PathBuf)> {
        use std::process::Command;

        if Command::new("ffmpeg").arg("-version").output().is_err()
            || Command::new("ffprobe").arg("-version").output().is_err()
        {
            return None;
        }
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(format!("fixture.{container}"));
        let duration = duration_secs.to_string();
        let mut command = Command::new("ffmpeg");
        command.args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            &format!("testsrc=size={width}x{height}:rate=1:duration={duration}"),
        ]);
        if with_audio {
            command.args([
                "-f",
                "lavfi",
                "-i",
                &format!("sine=frequency=440:duration={duration}"),
            ]);
        }
        command.args(["-threads", "1"]);
        if container == "webm" {
            command.args([
                "-c:v",
                "libvpx-vp9",
                "-deadline",
                "realtime",
                "-cpu-used",
                "8",
                "-crf",
                "40",
                "-b:v",
                "0",
            ]);
            if with_audio {
                command.args(["-c:a", "libopus", "-b:a", "32k"]);
            }
        } else {
            command.args(["-c:v", "mpeg4", "-q:v", "10"]);
            if with_audio {
                command.args(["-c:a", "aac"]);
            }
        }
        let result = command
            .args(["-t", &duration, "-y"])
            .arg(&path)
            .output()
            .expect("ffmpeg is available");
        assert!(
            result.status.success(),
            "ffmpeg could not create the fixture: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        Some((directory, path))
    }

    #[test]
    fn media_kind_from_mime() {
        let audio = MediaAttachment {
            file_name: "file".to_string(),
            data: vec![],
            mime_type: Some("audio/ogg".to_string()),
            marker: None,
        };
        assert_eq!(audio.kind(), MediaKind::Audio);

        let image = MediaAttachment {
            file_name: "file".to_string(),
            data: vec![],
            mime_type: Some("image/png".to_string()),
            marker: None,
        };
        assert_eq!(image.kind(), MediaKind::Image);

        let video = MediaAttachment {
            file_name: "file".to_string(),
            data: vec![],
            mime_type: Some("video/mp4".to_string()),
            marker: None,
        };
        assert_eq!(video.kind(), MediaKind::Video);
    }

    #[test]
    fn media_kind_from_extension() {
        let audio = MediaAttachment {
            file_name: "voice.ogg".to_string(),
            data: vec![],
            mime_type: None,
            marker: None,
        };
        assert_eq!(audio.kind(), MediaKind::Audio);

        let image = MediaAttachment {
            file_name: "photo.png".to_string(),
            data: vec![],
            mime_type: None,
            marker: None,
        };
        assert_eq!(image.kind(), MediaKind::Image);

        let video = MediaAttachment {
            file_name: "clip.mp4".to_string(),
            data: vec![],
            mime_type: None,
            marker: None,
        };
        assert_eq!(video.kind(), MediaKind::Video);

        let unknown = MediaAttachment {
            file_name: "data.bin".to_string(),
            data: vec![],
            mime_type: None,
            marker: None,
        };
        assert_eq!(unknown.kind(), MediaKind::Unknown);
    }

    #[tokio::test]
    async fn disabled_pipeline_returns_original_text() {
        let config = default_pipeline_config(false);
        let pipeline = MediaPipeline::new(&config, None, false);

        let result = pipeline.process("hello", &[sample_audio()]).await;
        assert_eq!(result, "hello");
    }

    #[tokio::test]
    async fn empty_attachments_returns_original_text() {
        let config = default_pipeline_config(true);
        let pipeline = MediaPipeline::new(&config, None, false);

        let result = pipeline.process("hello", &[]).await;
        assert_eq!(result, "hello");
    }

    #[tokio::test]
    async fn image_annotation_with_vision() {
        let config = default_pipeline_config(true);
        let pipeline = MediaPipeline::new(&config, None, true);

        let result = pipeline.process("check this", &[sample_image()]).await;
        assert!(
            result.contains("[Image: photo.jpg attached, will be processed by vision model]"),
            "expected vision annotation, got: {result}"
        );
        assert!(
            result.contains("[IMAGE:data:image/jpeg;base64,"),
            "expected image data marker, got: {result}"
        );
        assert!(result.contains("check this"));
    }

    #[tokio::test]
    async fn routed_images_are_persisted_inside_each_resolved_agent_workspace() {
        let config = default_pipeline_config(true);
        let owner = tempfile::tempdir().unwrap();
        let friend = tempfile::tempdir().unwrap();
        let attachment = sample_image();

        let owner_pipeline =
            MediaPipeline::new(&config, None, true).with_workspace_dir(owner.path());
        let owner_prompt = owner_pipeline
            .process("what is this?", std::slice::from_ref(&attachment))
            .await;
        let owner_path = image_marker_path(&owner_prompt);
        assert!(owner_path.starts_with(owner.path().canonicalize().unwrap()));
        assert_eq!(std::fs::read(&owner_path).unwrap(), attachment.data);
        assert!(owner_prompt.contains("[Image saved for this agent:"));

        let friend_pipeline =
            MediaPipeline::new(&config, None, true).with_workspace_dir(friend.path());
        let friend_prompt = friend_pipeline
            .process("what is this?", std::slice::from_ref(&attachment))
            .await;
        let friend_path = image_marker_path(&friend_prompt);
        assert!(friend_path.starts_with(friend.path().canonicalize().unwrap()));
        assert_ne!(owner_path, friend_path);
        assert_eq!(std::fs::read(&friend_path).unwrap(), attachment.data);
        assert!(!friend_prompt.contains(owner_path.to_str().unwrap()));
    }

    fn image_marker_path(prompt: &str) -> std::path::PathBuf {
        let path = prompt
            .split("[IMAGE:")
            .nth(1)
            .and_then(|suffix| suffix.split(']').next())
            .expect("loadable routed image has an IMAGE marker");
        std::path::PathBuf::from(path)
    }

    #[tokio::test]
    async fn image_already_marked_by_channel_is_not_double_described() {
        let config = default_pipeline_config(true);
        let pipeline = MediaPipeline::new(&config, None, true);

        // A channel (Telegram, Discord) that saved the file to disk emits a
        // re-loadable path marker itself; the pipeline must not add a second,
        // base64-inlined copy of the same image.
        let original = "[IMAGE:/workspace/telegram_files/photo.jpg]\n\nlog this automatically";
        let attachment = marked(
            "photo.jpg",
            "image/jpeg",
            "/workspace/telegram_files/photo.jpg",
        );
        let result = pipeline.process(original, &[attachment]).await;
        assert_eq!(
            result, original,
            "pre-marked image must pass through unchanged"
        );
        assert!(
            !result.contains("base64"),
            "no inline base64 may be added for a pre-marked image: {result}"
        );
    }

    #[tokio::test]
    async fn unrelated_image_marker_does_not_suppress_new_attachment() {
        let config = default_pipeline_config(true);
        let pipeline = MediaPipeline::new(&config, None, true);

        // A quoted older image marker for a DIFFERENT file must not swallow
        // the annotation for the newly attached one.
        let original = "[IMAGE:/workspace/telegram_files/old_photo.png] earlier pic";
        let result = pipeline.process(original, &[sample_image()]).await;
        assert!(
            result.contains("[IMAGE:data:image/jpeg;base64,"),
            "new attachment must still be annotated: {result}"
        );
    }

    /// An attachment as a channel hands it over when it rendered an image
    /// marker: bytes plus the target and the `Image` disposition it committed
    /// to.
    fn marked(file_name: &str, mime: &str, marker_target: &str) -> MediaAttachment {
        MediaAttachment {
            file_name: file_name.to_string(),
            data: vec![0xFF, 0xD8, 0xFF, 0xE0],
            mime_type: Some(mime.to_string()),
            marker: Some(RenderedMarker {
                target: marker_target.to_string(),
                kind: MarkerKind::Image,
            }),
        }
    }

    /// An attachment a channel rendered as a `[Document: ...]` even though its
    /// payload looks like an image: the non-loadable image-document case
    /// (HEIC, TIFF, SVG, BMP). `kind()` still reports `Image`, so only the
    /// recorded `Document` disposition can stop a second image annotation.
    fn marked_document(file_name: &str, mime: &str, marker_target: &str) -> MediaAttachment {
        MediaAttachment {
            file_name: file_name.to_string(),
            data: vec![0xFF, 0xD8, 0xFF, 0xE0],
            mime_type: Some(mime.to_string()),
            marker: Some(RenderedMarker {
                target: marker_target.to_string(),
                kind: MarkerKind::Document,
            }),
        }
    }

    #[tokio::test]
    async fn image_document_marked_by_channel_is_not_inlined() {
        let config = default_pipeline_config(true);
        let pipeline = MediaPipeline::new(&config, None, true);

        // An image sent "as file" (extensionless, image MIME): the channel
        // emits the same [IMAGE:<path>] marker as for photos, so the pipeline
        // must not add a second, base64-inlined copy.
        let attachment = marked("upload", "image/jpeg", "/workspace/telegram_files/upload");
        let original = "[IMAGE:/workspace/telegram_files/upload]\n\nplease describe";
        let result = pipeline.process(original, &[attachment]).await;
        assert_eq!(
            result, original,
            "channel-marked image document must pass through unchanged"
        );
        assert!(
            !result.contains("IMAGE:data:"),
            "no inline base64 may be added for a channel-marked image document: {result}"
        );
    }

    #[tokio::test]
    async fn discord_uuid_prefixed_marker_is_recognized_as_its_own_attachment() {
        let config = default_pipeline_config(true);
        let pipeline = MediaPipeline::new(&config, None, true);

        // Discord saves under a uniqueness-prefixed name while the envelope
        // keeps the sender's name, so the two never share a basename. Deferring
        // on the recorded disposition, not on any name comparison, is what
        // keeps this single-copy.
        let saved = "/ws/discord_files/6f1e4a4c-2b77-4a2f-9d0e-5c1f0b3a7e11_photo.jpg";
        let attachment = marked("photo.jpg", "image/jpeg", saved);
        let original = format!("[IMAGE:{saved}]\n\nwhat is this?");

        let result = pipeline.process(&original, &[attachment]).await;

        assert_eq!(
            result, original,
            "a Discord-saved image must not be inlined a second time"
        );
        assert!(
            !result.contains("IMAGE:data:"),
            "uuid-prefixed save names must still join to their marker: {result}"
        );
    }

    #[tokio::test]
    async fn discord_remote_image_fallback_replaces_its_channel_marker() {
        let config = default_pipeline_config(true);
        let pipeline = MediaPipeline::new(&config, None, true);
        let url = "https://cdn.discordapp.com/attachments/1/photo.jpg";
        let attachment = MediaAttachment {
            file_name: "photo.jpg".to_string(),
            data: vec![0xFF, 0xD8, 0xFF, 0xE0],
            mime_type: Some("image/jpeg".to_string()),
            marker: Some(RenderedMarker {
                target: url.to_string(),
                kind: MarkerKind::Image,
            }),
        };
        let original = format!("caption\n\n[IMAGE:{url}]");

        let result = pipeline.process(&original, &[attachment]).await;

        assert!(
            !result.contains(url),
            "the fallback URL must be replaced: {result}"
        );
        assert_eq!(
            result.matches("[IMAGE:data:").count(),
            1,
            "the typed bytes must produce one inline image marker: {result}"
        );
        assert!(result.contains("caption"));
    }

    #[test]
    fn discord_remote_image_fallback_removes_only_the_last_matching_marker() {
        let url = "https://cdn.discordapp.com/attachments/1/photo.jpg";
        let original = format!("[IMAGE:{url}] is part of the caption\n\n[IMAGE:{url}]");

        let result = remove_last_channel_image_marker(&original, url);

        assert_eq!(result, format!("[IMAGE:{url}] is part of the caption\n\n"));
    }

    #[tokio::test]
    async fn sender_authored_marker_cannot_suppress_a_real_attachment() {
        let config = default_pipeline_config(true);
        let pipeline = MediaPipeline::new(&config, None, true);

        // The sender typed a marker in the caption, but the real attachment
        // carries no channel-rendered disposition. Deference reads the typed
        // envelope, not the text, so the sender's marker cannot impersonate a
        // channel verdict and drop the only copy of the bytes.
        let mut attachment = sample_image();
        attachment.marker = None;
        let original = "[IMAGE:/ws/telegram_files/photo.jpg] describe the attached one";

        let result = pipeline.process(original, &[attachment]).await;

        assert!(
            result.contains("[IMAGE:data:image/jpeg;base64,"),
            "a sender-authored marker must not drop the only copy of the bytes: {result}"
        );
    }

    #[tokio::test]
    async fn contradictory_signals_cannot_produce_contradictory_annotations() {
        let config = default_pipeline_config(true);
        let pipeline = MediaPipeline::new(&config, None, true);

        // `kind()` reads the declared MIME and says video; the channel read
        // the name and the payload and marked it an image. One attachment
        // must not end up with both an image marker and a video note.
        let attachment = MediaAttachment {
            file_name: "photo.jpg".to_string(),
            data: vec![0xFF, 0xD8, 0xFF, 0xE0],
            mime_type: Some("video/mp4".to_string()),
            marker: Some(RenderedMarker {
                target: "/ws/telegram_files/photo.jpg".to_string(),
                kind: MarkerKind::Image,
            }),
        };
        assert_eq!(
            attachment.kind(),
            MediaKind::Video,
            "this test is only meaningful while the declared MIME wins routing"
        );

        let original = "[IMAGE:/ws/telegram_files/photo.jpg]\n\nwhat is this?";
        let result = pipeline.process(original, &[attachment]).await;

        assert_eq!(
            result, original,
            "the channel's rendered verdict must stand alone"
        );
        assert!(
            !result.contains("[Video:"),
            "an image-marked attachment must not also be annotated as video: {result}"
        );
    }

    #[tokio::test]
    async fn unmarked_attachment_is_always_annotated() {
        let config = default_pipeline_config(true);
        let pipeline = MediaPipeline::new(&config, None, true);

        // A channel that supplies bytes without rendering a marker gets the
        // pipeline's annotation even when the text mentions a same-named file.
        let original = "[IMAGE:/ws/telegram_files/photo.jpg] and also photo.jpg";
        let result = pipeline.process(original, &[sample_image()]).await;

        assert!(
            result.contains("[IMAGE:data:image/jpeg;base64,"),
            "an attachment with no channel marker must be annotated: {result}"
        );
    }

    #[tokio::test]
    async fn channel_rendered_document_is_not_reclassified_as_image() {
        let config = default_pipeline_config(true);
        let pipeline = MediaPipeline::new(&config, None, true);

        // The reviewer's boundary: a non-loadable image document (HEIC) the
        // channel deliberately rendered as `[Document: ...]`. `kind()` still
        // reports Image from the MIME, so without the recorded Document
        // disposition the pipeline would add an `[IMAGE:data:...]` copy the
        // provider then rejects.
        let attachment =
            marked_document("photo.heic", "image/heic", "/ws/telegram_files/photo.heic");
        assert_eq!(
            attachment.kind(),
            MediaKind::Image,
            "this test is only meaningful while the declared image MIME wins routing"
        );

        let original = "[Document: photo.heic] /ws/telegram_files/photo.heic\n\nwhat is this?";
        let result = pipeline.process(original, &[attachment]).await;

        assert_eq!(
            result, original,
            "the channel's document verdict must stand alone"
        );
        assert!(
            !result.contains("IMAGE:data:"),
            "a channel-rendered document must not gain an inline image copy: {result}"
        );
        assert!(
            !result.contains("[Image:"),
            "a channel-rendered document must not gain an image annotation: {result}"
        );
    }

    #[cfg(feature = "image-normalization")]
    #[tokio::test]
    async fn webp_image_is_normalized_to_png_for_vision() {
        let config = default_pipeline_config(true);
        let pipeline = MediaPipeline::new(&config, None, true);
        let mut cursor = std::io::Cursor::new(Vec::new());
        let webp = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            1,
            1,
            image::Rgba([255, 0, 0, 255]),
        ));
        webp.write_to(&mut cursor, image::ImageFormat::WebP)
            .expect("test WebP should encode");

        let sticker = MediaAttachment {
            file_name: "sticker.webp".to_string(),
            data: cursor.into_inner(),
            mime_type: Some("image/webp".to_string()),
            marker: None,
        };

        let result = pipeline.process("what is this?", &[sticker]).await;

        assert!(result.contains("[IMAGE:data:image/png;base64,"));
        assert!(!result.contains("[IMAGE:data:image/webp;base64,"));
        assert!(result.contains("what is this?"));
    }

    #[tokio::test]
    async fn image_annotation_without_vision() {
        let config = default_pipeline_config(true);
        let pipeline = MediaPipeline::new(&config, None, false);

        let result = pipeline.process("check this", &[sample_image()]).await;
        assert!(
            result.contains("[Image: photo.jpg attached]"),
            "expected basic image annotation, got: {result}"
        );
        assert!(
            !result.contains("[IMAGE:data:"),
            "non-vision path must not inline image data, got: {result}"
        );
    }

    #[tokio::test]
    async fn video_without_agent_workspace_is_not_processed() {
        let config = default_pipeline_config(true);
        let pipeline = MediaPipeline::new(&config, None, false);

        let result = pipeline.process("watch", &[sample_video()]).await;
        assert!(
            result.contains("no media workspace"),
            "video without a resolved agent workspace must fail closed: {result}"
        );
    }

    #[tokio::test]
    async fn real_silent_mp4_extracts_scoped_frames_across_duration_and_leaves_no_temp_files() {
        let Some((_fixture_dir, fixture_path)) = generate_mp4_fixture(8, false, 720, 1280) else {
            eprintln!("skipping ffmpeg fixture test: ffmpeg/ffprobe unavailable");
            return;
        };
        let bytes = std::fs::read(&fixture_path).unwrap();
        let mdat = bytes.windows(4).position(|atom| atom == b"mdat").unwrap();
        let moov = bytes.windows(4).position(|atom| atom == b"moov").unwrap();
        assert!(
            moov > mdat,
            "fixture must exercise a normal moov-at-end MP4"
        );

        let workspace = tempfile::tempdir().unwrap();
        let config = default_pipeline_config(true);
        let pipeline = MediaPipeline::new(&config, None, true).with_workspace_dir(workspace.path());
        let original_bytes = bytes.clone();
        let attachment = MediaAttachment {
            file_name: "clip.mp4".to_string(),
            data: bytes,
            mime_type: Some("video/mp4".to_string()),
            marker: None,
        };

        let result = pipeline
            .process("make a meme from this clip", &[attachment])
            .await;

        assert_eq!(result.matches("[Video frame ").count(), 4, "{result}");
        assert!(result.contains("[Video frame 4 of 4]"));
        assert!(result.contains("make a meme from this clip"), "{result}");
        assert!(
            result.contains("use one of these saved frame paths as an image_gen reference"),
            "{result}"
        );
        let video_path_hint = result
            .lines()
            .find_map(|line| {
                line.strip_prefix("[Original video saved for this agent: ")?
                    .strip_suffix(']')
            })
            .expect("a valid bounded video should be retained in its agent workspace");
        let (cleaned_result, marker_paths) =
            zeroclaw_providers::multimodal::parse_image_markers(&result);
        assert_eq!(marker_paths.len(), 4, "{marker_paths:?}");
        assert!(cleaned_result.contains("make a meme from this clip"));
        assert!(cleaned_result.contains("one of these saved frame paths"));

        let messages = [zeroclaw_providers::ChatMessage::user(result.clone())];
        let prepared = zeroclaw_providers::multimodal::prepare_messages_for_provider_scoped(
            &messages,
            &zeroclaw_config::schema::MultimodalConfig::default(),
            workspace.path(),
            None,
        )
        .await
        .unwrap();
        let (prepared_text, normalized_images) =
            zeroclaw_providers::multimodal::parse_image_markers(&prepared.messages[0].content);
        assert_eq!(normalized_images.len(), 4, "{normalized_images:?}");
        assert!(prepared_text.contains("make a meme from this clip"));
        assert!(prepared_text.contains("one of these saved frame paths"));
        assert!(
            !result.contains("transcription") && !result.contains("Video audio"),
            "a silent clip must not produce an audio failure annotation: {result}"
        );
        let media_dir = workspace.path().join("telegram_files");
        let entries = std::fs::read_dir(&media_dir)
            .unwrap()
            .map(Result::unwrap)
            .collect::<Vec<_>>();
        assert_eq!(entries.len(), 5);
        let mut frame_hashes = std::collections::HashSet::new();
        let mut retained_video = false;
        for entry in entries {
            let path = tokio::fs::canonicalize(entry.path()).await.unwrap();
            let path_text = path.display().to_string();
            if path.extension().and_then(|extension| extension.to_str()) == Some("mp4") {
                assert_eq!(path_text, video_path_hint);
                assert!(cleaned_result.contains(&format!(
                    "[Original video saved for this agent: {path_text}]"
                )));
                assert!(prepared_text.contains(&format!(
                    "[Original video saved for this agent: {path_text}]"
                )));
                assert_eq!(std::fs::read(&path).unwrap(), original_bytes);
                retained_video = true;
                continue;
            }
            assert!(
                result.contains(&format!("[Image saved for this agent: {path_text}]")),
                "the frame path should be explicit in model-visible text: {result}"
            );
            assert!(
                result.contains(&format!("[IMAGE:{path_text}]")),
                "the same saved frame should be sent through vision: {result}"
            );
            assert!(
                cleaned_result.contains(&format!("[Image saved for this agent: {path_text}]")),
                "marker parsing should retain the saved-image path hint: {cleaned_result}"
            );
            assert!(
                prepared_text.contains(&format!("[Image saved for this agent: {path_text}]")),
                "provider preparation should retain the saved-image path hint: {prepared_text}"
            );
            let frame = std::fs::read(path).unwrap();
            assert!(frame.starts_with(&[0xff, 0xd8]));
            let decoded = image::load_from_memory(&frame).unwrap();
            assert!(decoded.width() <= VIDEO_FRAME_WIDTH as u32);
            assert!(decoded.height() <= VIDEO_FRAME_WIDTH as u32);
            assert_eq!(decoded.height(), VIDEO_FRAME_WIDTH as u32);
            frame_hashes.insert(sha2::Sha256::digest(frame));
        }
        assert!(
            frame_hashes.len() > 1,
            "samples should span changing frames"
        );
        assert!(
            retained_video,
            "the source clip should remain in the agent workspace"
        );
        assert_eq!(
            std::fs::read_dir(workspace.path()).unwrap().count(),
            1,
            "only the persistent agent-owned media directory should remain"
        );
    }

    #[tokio::test]
    async fn actual_video_duration_over_limit_is_rejected_before_frame_extraction() {
        let Some((_fixture_dir, fixture_path)) = generate_mp4_fixture(121, false, 1280, 720) else {
            eprintln!("skipping ffmpeg fixture test: ffmpeg/ffprobe unavailable");
            return;
        };
        let workspace = tempfile::tempdir().unwrap();
        let config = default_pipeline_config(true);
        let pipeline = MediaPipeline::new(&config, None, true).with_workspace_dir(workspace.path());
        let attachment = MediaAttachment {
            file_name: "long.mp4".to_string(),
            data: std::fs::read(fixture_path).unwrap(),
            mime_type: Some("video/mp4".to_string()),
            marker: None,
        };

        let result = pipeline.process("inspect", &[attachment]).await;

        assert!(result.contains("120 second processing limit"), "{result}");
        assert!(!workspace.path().join("telegram_files").exists());
        assert_eq!(std::fs::read_dir(workspace.path()).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn real_audio_track_is_detected_and_compressed_audio_stays_below_cap() {
        let Some((_fixture_dir, fixture_path)) = generate_mp4_fixture(8, true, 1280, 720) else {
            eprintln!("skipping ffmpeg fixture test: ffmpeg/ffprobe unavailable");
            return;
        };
        let workspace = tempfile::tempdir().unwrap();
        let tempdir = tempfile::tempdir_in(workspace.path()).unwrap();
        let input_path = tempdir.path().join("fixture.mp4");
        std::fs::copy(fixture_path, &input_path).unwrap();
        let info = probe_video(
            std::path::Path::new("ffprobe"),
            &input_path,
            "mov",
            VIDEO_FFMPEG_TIMEOUT,
        )
        .await
        .unwrap();
        assert!(info.has_audio);
        assert!(info.duration_secs <= VIDEO_MAX_DURATION_SECS as f64);

        let audio = extract_video_audio(
            std::path::Path::new("ffmpeg"),
            &input_path,
            "mov",
            info.duration_secs,
            VIDEO_FFMPEG_TIMEOUT,
        )
        .await
        .unwrap();
        assert!(!audio.is_empty());
        assert!(audio.len() <= VIDEO_MAX_AUDIO_OUTPUT_BYTES);
    }

    #[tokio::test]
    async fn matroska_and_webm_inputs_probe_decode_and_extract_audio() {
        for container in ["mkv", "webm"] {
            let Some((_fixture_dir, fixture_path)) =
                generate_video_fixture(6, true, 640, 360, container)
            else {
                eprintln!("skipping ffmpeg fixture test: ffmpeg/ffprobe unavailable");
                return;
            };
            let workspace = tempfile::tempdir().unwrap();
            let tempdir = tempfile::tempdir_in(workspace.path()).unwrap();
            let input_path = tempdir.path().join(format!("fixture.{container}"));
            std::fs::copy(fixture_path, &input_path).unwrap();
            let input_format = safe_video_demuxer(container);

            let info = probe_video(
                std::path::Path::new("ffprobe"),
                &input_path,
                input_format,
                VIDEO_FFMPEG_TIMEOUT,
            )
            .await
            .unwrap();
            assert!(
                info.has_audio,
                "{container} audio stream should be detected"
            );

            let frames = extract_video_frames(
                std::path::Path::new("ffmpeg"),
                &input_path,
                input_format,
                info.duration_secs,
                VIDEO_FFMPEG_TIMEOUT,
            )
            .await
            .unwrap();
            assert!(!frames.is_empty(), "{container} frames should decode");

            let audio = extract_video_audio(
                std::path::Path::new("ffmpeg"),
                &input_path,
                input_format,
                info.duration_secs,
                VIDEO_FFMPEG_TIMEOUT,
            )
            .await
            .unwrap();
            assert!(!audio.is_empty(), "{container} audio should extract");
            assert!(audio.len() <= VIDEO_MAX_AUDIO_OUTPUT_BYTES);
        }
    }

    #[tokio::test]
    async fn playlist_payload_cannot_read_a_sibling_video_file() {
        let Some((_fixture_dir, fixture_path)) = generate_mp4_fixture(8, true, 1280, 720) else {
            eprintln!("skipping ffmpeg fixture test: ffmpeg/ffprobe unavailable");
            return;
        };
        let workspace = tempfile::tempdir().unwrap();
        let sibling_video = workspace.path().join("private-sibling.mp4");
        std::fs::copy(fixture_path, &sibling_video).unwrap();
        let playlist = format!("ffconcat version 1.0\nfile '{}'\n", sibling_video.display());
        let config = default_pipeline_config(true);
        let pipeline = MediaPipeline::new(&config, None, true).with_workspace_dir(workspace.path());
        let attachment = MediaAttachment {
            file_name: "playlist.mp4".to_string(),
            data: playlist.into_bytes(),
            mime_type: Some("video/mp4".to_string()),
            marker: None,
        };

        let result = pipeline.process("inspect this clip", &[attachment]).await;

        assert!(result.contains("Video could not be inspected"), "{result}");
        assert!(!result.contains("[Video frame"), "{result}");
        assert!(!result.contains("transcription"), "{result}");
        assert_eq!(
            std::fs::read_dir(workspace.path()).unwrap().count(),
            1,
            "the sibling file remains, and no extracted media was persisted"
        );
    }

    #[cfg(unix)]
    fn executable_script(
        workspace: &std::path::Path,
        name: &str,
        body: &str,
    ) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt as _;

        let path = workspace.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        path
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn probe_timeout_kills_child_and_cleans_staged_video() {
        let workspace = tempfile::tempdir().unwrap();
        let sleeper = executable_script(workspace.path(), "slow-ffprobe", "exec sleep 5");
        let config = default_pipeline_config(true);
        let pipeline = MediaPipeline::new(&config, None, true)
            .with_workspace_dir(workspace.path())
            .with_ffprobe_program(sleeper)
            .with_ffmpeg_timeout(VIDEO_FFMPEG_TEST_TIMEOUT);
        let started = std::time::Instant::now();

        let result = pipeline.process("inspect", &[sample_video()]).await;

        assert!(result.contains("could not be inspected"), "{result}");
        assert!(started.elapsed() < Duration::from_secs(2));
        assert_eq!(std::fs::read_dir(workspace.path()).unwrap().count(), 1);
        assert!(workspace.path().join("slow-ffprobe").exists());
        assert!(
            std::fs::read_dir(workspace.path())
                .unwrap()
                .all(|entry| !entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with("telegram-video-")),
            "temporary video input must be removed after timeout"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn probe_output_limit_cleans_staged_video() {
        let workspace = tempfile::tempdir().unwrap();
        let noisy_probe =
            executable_script(workspace.path(), "noisy-ffprobe", "printf '%17000s' ''");
        let config = default_pipeline_config(true);
        let pipeline = MediaPipeline::new(&config, None, true)
            .with_workspace_dir(workspace.path())
            .with_ffprobe_program(noisy_probe);

        let result = pipeline.process("inspect", &[sample_video()]).await;

        assert!(result.contains("could not be inspected"), "{result}");
        let entries = std::fs::read_dir(workspace.path())
            .unwrap()
            .map(Result::unwrap)
            .collect::<Vec<_>>();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].file_name(), "noisy-ffprobe");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn ffmpeg_output_limit_kills_child_preserves_source_and_cleans_staged_video() {
        let workspace = tempfile::tempdir().unwrap();
        let probe = executable_script(
            workspace.path(),
            "fake-ffprobe",
            "printf '{\"format\":{\"duration\":\"1\"},\"streams\":[{\"codec_type\":\"video\"}]}'",
        );
        let noisy_ffmpeg =
            executable_script(workspace.path(), "noisy-ffmpeg", "printf '%9000000s' ''");
        let config = default_pipeline_config(true);
        let pipeline = MediaPipeline::new(&config, None, true)
            .with_workspace_dir(workspace.path())
            .with_ffprobe_program(probe)
            .with_ffmpeg_program(noisy_ffmpeg);

        let video = sample_video();
        let source_bytes = video.data.clone();
        let result = pipeline.process("inspect", &[video]).await;

        assert!(
            result.contains("Video frames could not be extracted"),
            "{result}"
        );
        assert!(result.contains("[Original video saved for this agent:"));
        let entries = std::fs::read_dir(workspace.path())
            .unwrap()
            .map(Result::unwrap)
            .collect::<Vec<_>>();
        assert_eq!(entries.len(), 3);
        assert!(entries.iter().all(|entry| {
            matches!(
                entry.file_name().to_string_lossy().as_ref(),
                "fake-ffprobe" | "noisy-ffmpeg" | "telegram_files"
            )
        }));
        let media_entries = std::fs::read_dir(workspace.path().join("telegram_files"))
            .unwrap()
            .map(Result::unwrap)
            .collect::<Vec<_>>();
        assert_eq!(media_entries.len(), 1);
        assert_eq!(
            std::fs::read(media_entries[0].path()).unwrap(),
            source_bytes
        );
        assert!(
            std::fs::read_dir(workspace.path())
                .unwrap()
                .all(|entry| !entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with("telegram-video-")),
            "temporary video input must be removed while the scoped source copy remains"
        );
    }

    #[tokio::test]
    async fn audio_without_transcription_enabled() {
        let config = default_pipeline_config(true);
        let pipeline = MediaPipeline::new(&config, None, false);

        let result = pipeline.process("", &[sample_audio()]).await;
        assert!(result.contains("Audio transcription unavailable"));
        assert!(result.contains("Do not guess the recording's contents"));
    }

    #[tokio::test]
    async fn multiple_attachments_produce_multiple_annotations() {
        let config = default_pipeline_config(true);
        let pipeline = MediaPipeline::new(&config, None, false);

        let attachments = vec![sample_audio(), sample_image(), sample_video()];
        let result = pipeline.process("context", &attachments).await;

        assert!(
            result.contains("[Audio transcription unavailable."),
            "missing audio annotation"
        );
        assert!(
            result.contains("[Image: photo.jpg attached]"),
            "missing image annotation"
        );
        assert!(
            result.contains(
                "[Video could not be inspected because this agent has no media workspace.]"
            ),
            "missing video annotation"
        );
        assert!(result.contains("context"), "missing original text");
    }

    #[tokio::test]
    async fn disabled_sub_features_skip_processing() {
        let config = MediaPipelineConfig {
            enabled: true,
            transcribe_audio: false,
            describe_images: false,
            summarize_video: false,
        };
        let pipeline = MediaPipeline::new(&config, None, false);

        let attachments = vec![sample_audio(), sample_image(), sample_video()];
        let result = pipeline.process("hello", &attachments).await;
        assert_eq!(result, "hello");
    }
}
