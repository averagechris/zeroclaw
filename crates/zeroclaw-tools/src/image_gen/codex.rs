//! Codex image generation and editing. Uploaded references and results belong to this agent.
use super::*;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use std::sync::OnceLock;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const INPUT_LIMIT: usize = 20 * 1024 * 1024;
const MAX_REFERENCES: usize = 4;

fn text(key: &str) -> String {
    crate::i18n::get_required_tool_string(key)
}

fn failure(key: &str) -> anyhow::Error {
    anyhow::Error::msg(text(key))
}

pub(super) fn description() -> &'static str {
    static DESCRIPTION: OnceLock<String> = OnceLock::new();
    DESCRIPTION.get_or_init(|| text("tool-image-gen-openai-description"))
}

pub(super) fn parameters_schema() -> serde_json::Value {
    json!({"type":"object", "required":["prompt"], "properties":{
        "prompt":{"type":"string","description":text("tool-image-gen-openai-prompt")},
        "images":{"type":"array","maxItems":MAX_REFERENCES,"items":{"type":"string"},"description":text("tool-image-gen-openai-images")},
        "size":{"type":"string","enum":["1024x1024","1536x1024","1024x1536"],"description":text("tool-image-gen-openai-size")},
        "quality":{"type":"string","enum":["low","medium","high"],"description":text("tool-image-gen-openai-quality")},
        "background":{"type":"string","enum":["auto","opaque","transparent"],"description":text("tool-image-gen-openai-background")}
    }})
}

fn option<'a>(
    args: &'a serde_json::Value,
    key: &str,
    default: &'a str,
    allowed: &[&str],
) -> anyhow::Result<&'a str> {
    let value = match args.get(key) {
        None => default,
        Some(v) => v
            .as_str()
            .ok_or_else(|| failure("tool-image-gen-openai-invalid-options"))?,
    };
    if !allowed.contains(&value) {
        return Err(failure("tool-image-gen-openai-invalid-options"));
    }
    Ok(value)
}

impl ImageGenTool {
    async fn read_codex_reference(&self, path: &str) -> anyhow::Result<(Vec<u8>, &'static str)> {
        let workspace = tokio::fs::canonicalize(&self.workspace_dir)
            .await
            .map_err(|_| failure("tool-image-gen-openai-file-error"))?;
        let resolved = tokio::fs::canonicalize(self.security.resolve_tool_path(path))
            .await
            .map_err(|_| failure("tool-image-gen-openai-file-error"))?;
        // Never inherit broader owner read grants for an image-edit reference.
        if !resolved.starts_with(&workspace) || !self.security.is_resolved_path_readable(&resolved)
        {
            return Err(failure("tool-image-gen-openai-outside-workspace"));
        }
        let file = tokio::fs::File::open(resolved)
            .await
            .map_err(|_| failure("tool-image-gen-openai-file-error"))?;
        let mut bytes = Vec::new();
        file.take(INPUT_LIMIT as u64 + 1)
            .read_to_end(&mut bytes)
            .await
            .map_err(|_| failure("tool-image-gen-openai-file-error"))?;
        if bytes.len() > INPUT_LIMIT {
            return Err(failure("tool-image-gen-openai-input-limit"));
        }
        let mime = match zeroclaw_api::media::image_mime_from_magic(&bytes) {
            Some("image/png") => "image/png",
            Some("image/jpeg") => "image/jpeg",
            Some("image/webp") => "image/webp",
            _ => return Err(failure("tool-image-gen-openai-image-type")),
        };
        Ok((bytes, mime))
    }

    pub(super) async fn generate_codex(
        &self,
        args: serde_json::Value,
    ) -> anyhow::Result<ToolResult> {
        let (provider, model) = self
            .codex
            .as_ref()
            .ok_or_else(|| failure("tool-image-gen-openai-api-failed"))?;
        let prompt = args
            .get("prompt")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| failure("tool-image-gen-openai-missing-prompt"))?;
        let size = option(
            &args,
            "size",
            "1024x1024",
            &["1024x1024", "1536x1024", "1024x1536"],
        )?;
        let quality = option(&args, "quality", "medium", &["low", "medium", "high"])?;
        let background = option(
            &args,
            "background",
            "auto",
            &["auto", "opaque", "transparent"],
        )?;
        let references = match args.get("images") {
            None => &[][..],
            Some(value) => value
                .as_array()
                .map(Vec::as_slice)
                .ok_or_else(|| failure("tool-image-gen-openai-invalid-options"))?,
        };
        if references.len() > MAX_REFERENCES {
            return Err(failure("tool-image-gen-openai-input-limit"));
        }
        let mut images = Vec::new();
        let mut total = 0;
        for reference in references {
            let path = reference
                .as_str()
                .ok_or_else(|| failure("tool-image-gen-openai-invalid-options"))?;
            let (bytes, mime) = self.read_codex_reference(path).await?;
            total += bytes.len();
            if total > INPUT_LIMIT {
                return Err(failure("tool-image-gen-openai-input-limit"));
            }
            images.push(format!("data:{mime};base64,{}", STANDARD.encode(bytes)));
        }
        let encoded = provider
            .generate_image(zeroclaw_providers::openai_codex::CodexImageRequest {
                model: model.clone(),
                image_model: self.default_model.clone(),
                prompt: prompt.to_owned(),
                images,
                size: size.to_owned(),
                quality: quality.to_owned(),
                background: background.to_owned(),
            })
            .await
            .map_err(|_| failure("tool-image-gen-openai-api-failed"))?;
        self.save_codex_image(&encoded, prompt).await
    }

    async fn save_codex_image(&self, encoded: &str, prompt: &str) -> anyhow::Result<ToolResult> {
        if encoded.len() > GENERATED_IMAGE_LIMIT_BYTES.div_ceil(3) * 4 {
            return Err(failure("tool-image-gen-openai-output-limit"));
        }
        let bytes = STANDARD
            .decode(encoded)
            .map_err(|_| failure("tool-image-gen-openai-response-failed"))?;
        if bytes.len() > GENERATED_IMAGE_LIMIT_BYTES {
            return Err(failure("tool-image-gen-openai-output-limit"));
        }
        if zeroclaw_api::media::image_mime_from_magic(&bytes) != Some("image/png") {
            return Err(failure("tool-image-gen-openai-response-failed"));
        }
        let workspace = tokio::fs::canonicalize(&self.workspace_dir)
            .await
            .map_err(|_| failure("tool-image-gen-openai-file-error"))?;
        let dir = workspace.join("images");
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|_| failure("tool-image-gen-openai-file-error"))?;
        let dir = tokio::fs::canonicalize(dir)
            .await
            .map_err(|_| failure("tool-image-gen-openai-file-error"))?;
        if !dir.starts_with(&workspace) {
            return Err(failure("tool-image-gen-openai-outside-workspace"));
        }
        let output = dir.join(format!("generated-{}.png", uuid::Uuid::new_v4()));
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&output)
            .await
            .map_err(|_| failure("tool-image-gen-openai-file-error"))?;
        file.write_all(&bytes)
            .await
            .map_err(|_| failure("tool-image-gen-openai-file-error"))?;
        let path = output.display().to_string();
        Ok(ToolResult {
            success: true,
            output: format_image_tool_output(
                &path,
                bytes.len() / 1024,
                &self.default_model,
                prompt,
            )
            .into(),
            error: None,
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use zeroclaw_config::autonomy::AutonomyLevel;
    const PNG: &[u8] = &[
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 2,
        0, 0, 0, 144, 119, 83, 222, 0, 0, 0, 12, 73, 68, 65, 84, 120, 156, 99, 248, 255, 255, 63,
        0, 5, 254, 2, 254, 13, 239, 70, 184, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
    ];

    fn tool(root: &std::path::Path) -> ImageGenTool {
        let security = Arc::new(SecurityPolicy {
            autonomy: AutonomyLevel::Full,
            workspace_dir: root.to_path_buf(),
            ..Default::default()
        });
        let options = zeroclaw_providers::ModelProviderRuntimeOptions {
            zeroclaw_dir: Some(root.join("test-auth")),
            ..Default::default()
        };
        ImageGenTool::new(
            security,
            root.to_path_buf(),
            "gpt-image-2".into(),
            "UNUSED".into(),
            vec![],
        )
        .unwrap()
        .with_provider(ImageGenProvider::OpenaiCodex)
        .with_codex(
            zeroclaw_providers::openai_codex::OpenAiCodexModelProvider::new("test", &options, None)
                .unwrap(),
            "gpt-6-luna".into(),
        )
    }

    #[tokio::test]
    async fn codex_image_tool_rejects_sibling_references_before_auth() {
        let root = tempfile::tempdir().unwrap();
        let sibling = tempfile::tempdir().unwrap();
        let reference = sibling.path().join("private.png");
        tokio::fs::write(&reference, PNG).await.unwrap();
        let error = tool(root.path())
            .execute(json!({"prompt":"edit", "images":[reference]}))
            .await
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            text("tool-image-gen-openai-outside-workspace")
        );
        assert!(!root.path().join("test-auth").exists());
        assert!(!root.path().join("images").exists());
    }

    #[tokio::test]
    async fn codex_image_tool_bounds_options_and_checks_image_bytes() {
        let root = tempfile::tempdir().unwrap();
        let tool = tool(root.path());
        assert!(
            tool.execute(json!({"prompt":"edit", "images":["1","2","3","4","5"]}))
                .await
                .is_err()
        );
        assert!(
            tool.execute(json!({"prompt":"draw", "size":"unbounded"}))
                .await
                .is_err()
        );
        let reference = root.path().join("fake.png");
        tokio::fs::write(&reference, b"<html>not an image</html>")
            .await
            .unwrap();
        assert_eq!(
            tool.read_codex_reference(reference.to_str().unwrap())
                .await
                .unwrap_err()
                .to_string(),
            text("tool-image-gen-openai-image-type")
        );
        assert!(
            tool.save_codex_image(&STANDARD.encode(b"not an image"), "draw")
                .await
                .is_err()
        );
        assert!(!root.path().join("images").exists());
    }

    #[tokio::test]
    async fn codex_image_result_is_owned_and_reusable() {
        let root = tempfile::tempdir().unwrap();
        let tool = tool(root.path());
        let result = tool
            .save_codex_image(&STANDARD.encode(PNG), "a kitten")
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.output.to_string().contains("[IMAGE:"));
        let files: Vec<_> = std::fs::read_dir(root.path().join("images"))
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        assert_eq!(files.len(), 1);
        assert_eq!(tokio::fs::read(&files[0]).await.unwrap(), PNG);
        assert_eq!(
            tool.read_codex_reference(files[0].to_str().unwrap())
                .await
                .unwrap()
                .0,
            PNG
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn codex_image_symlinks_cannot_escape_workspace() {
        let root = tempfile::tempdir().unwrap();
        let sibling = tempfile::tempdir().unwrap();
        let private = sibling.path().join("private.png");
        std::fs::write(&private, PNG).unwrap();
        std::os::unix::fs::symlink(private, root.path().join("ref.png")).unwrap();
        std::os::unix::fs::symlink(sibling.path(), root.path().join("images")).unwrap();
        let tool = tool(root.path());
        assert!(
            tool.read_codex_reference(root.path().join("ref.png").to_str().unwrap())
                .await
                .is_err()
        );
        assert!(
            tool.save_codex_image(&STANDARD.encode(PNG), "draw")
                .await
                .is_err()
        );
        assert_eq!(std::fs::read_dir(sibling.path()).unwrap().count(), 1);
    }
}
