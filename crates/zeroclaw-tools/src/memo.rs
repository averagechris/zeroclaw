//! A workspace-bound adapter for the local `memo` CLI.

use async_trait::async_trait;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;
use zeroclaw_api::tool::{Tool, ToolOutput, ToolResult};
use zeroclaw_config::policy::{SecurityPolicy, ToolOperation};

const MAX_WAKE_LINES: usize = 256;
const MAX_TEXT_BYTES: usize = 280;
const MAX_OUTPUT_BYTES: usize = 512 * 1024;
const CLI_TIMEOUT: Duration = Duration::from_secs(10);

/// Exposes only memo's bounded operations, always against one agent workspace.
pub struct MemoTool {
    executable: String,
    workspace: PathBuf,
    wake_lines: usize,
    security: Arc<SecurityPolicy>,
}

impl MemoTool {
    #[must_use]
    pub fn new(
        executable: impl Into<String>,
        workspace: &Path,
        wake_lines: usize,
        security: Arc<SecurityPolicy>,
    ) -> Self {
        Self {
            executable: executable.into(),
            workspace: workspace.to_path_buf(),
            wake_lines: wake_lines.clamp(1, MAX_WAKE_LINES),
            security,
        }
    }

    async fn run(
        &self,
        data_dir: &Path,
        workspace: &Path,
        operation: &[String],
    ) -> anyhow::Result<CliOutput> {
        let executable = if Path::new(&self.executable).is_absolute() {
            PathBuf::from(&self.executable)
        } else {
            which::which(&self.executable)
                .map_err(|_| anyhow::Error::msg("memo executable could not be found"))?
        };
        let mut command = Command::new(executable);
        command
            .env_clear()
            .args(["--data-dir"])
            .arg(data_dir)
            .args(["--store", "default", "--output-format", "json"])
            .args(operation)
            .current_dir(workspace)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let mut child = command
            .spawn()
            .map_err(|_| anyhow::Error::msg("memo executable could not be started"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow::Error::msg("memo output is unavailable"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| anyhow::Error::msg("memo output is unavailable"))?;
        let result = tokio::time::timeout(CLI_TIMEOUT, async {
            let wait = async { child.wait().await.map_err(anyhow::Error::from) };
            let (stdout, stderr, status) =
                tokio::try_join!(read_capped(stdout), read_capped(stderr), wait)?;
            Ok::<_, anyhow::Error>(CliOutput {
                stdout,
                stderr,
                success: status.success(),
            })
        })
        .await
        .map_err(|_| anyhow::Error::msg("memo operation timed out"))??;
        Ok(result)
    }

    async fn invoke(&self, operation: &[String]) -> anyhow::Result<Value> {
        let is_mutation = matches!(operation.first().map(String::as_str), Some("note"))
            || (operation.first().is_some_and(|op| op == "nap") && operation.len() > 1);
        let Some((workspace, data_dir, initialized, store_exists)) =
            self.scoped_paths(is_mutation).await?
        else {
            return empty_read_response(operation.first().map(String::as_str), self.wake_lines);
        };
        if !store_exists && !is_mutation {
            return Ok(match operation.first().map(String::as_str) {
                Some("wake") => {
                    json!({"command":"wake", "ok":true, "complete":true, "lines":self.wake_lines, "items":[]})
                }
                Some("nap") => json!({"command":"nap", "ok":true, "status":"none", "pending":null}),
                _ => anyhow::bail!("memo operation requires an initialized store"),
            });
        }
        anyhow::ensure!(
            initialized || !store_exists,
            "memo store is not initialized"
        );
        if is_mutation && !initialized {
            let init = self
                .run(&data_dir, &workspace, &["init".to_string()])
                .await?;
            if !init.success {
                anyhow::bail!("memo store could not be initialized");
            }
        }
        let output = self.run(&data_dir, &workspace, operation).await?;
        let json_bytes = if output.success {
            &output.stdout
        } else {
            &output.stderr
        };
        let mut value: Value = serde_json::from_slice(json_bytes)
            .map_err(|_| anyhow::Error::msg("memo returned an invalid response"))?;
        if !output.success {
            let is_pending_wake = operation.first().is_some_and(|op| op == "wake")
                && value.pointer("/error/kind").and_then(Value::as_str) == Some("wake_incomplete");
            if !is_pending_wake {
                anyhow::bail!("memo operation failed");
            }
        }
        sanitize_cli_response(&mut value);
        Ok(value)
    }

    async fn scoped_paths(
        &self,
        create_workspace: bool,
    ) -> anyhow::Result<Option<(PathBuf, PathBuf, bool, bool)>> {
        if create_workspace {
            tokio::fs::create_dir_all(&self.workspace)
                .await
                .map_err(|_| anyhow::Error::msg("memo workspace is unavailable"))?;
        }
        match tokio::fs::symlink_metadata(&self.workspace).await {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && !create_workspace => {
                return Ok(None);
            }
            Err(_) => anyhow::bail!("memo workspace is unavailable"),
        }
        let workspace = tokio::fs::canonicalize(&self.workspace)
            .await
            .map_err(|_| anyhow::Error::msg("memo workspace is unavailable"))?;
        let metadata = tokio::fs::metadata(&workspace)
            .await
            .map_err(|_| anyhow::Error::msg("memo workspace is unavailable"))?;
        anyhow::ensure!(metadata.is_dir(), "memo workspace is unavailable");

        let candidate = workspace.join("memo");
        let data_dir = match tokio::fs::symlink_metadata(&candidate).await {
            Ok(_) => {
                let canonical = tokio::fs::canonicalize(&candidate)
                    .await
                    .map_err(|_| anyhow::Error::msg("memo store location is unavailable"))?;
                anyhow::ensure!(
                    canonical.starts_with(&workspace),
                    "memo store location is outside the agent workspace"
                );
                let metadata = tokio::fs::metadata(&canonical)
                    .await
                    .map_err(|_| anyhow::Error::msg("memo store location is unavailable"))?;
                anyhow::ensure!(metadata.is_dir(), "memo store location is unavailable");
                canonical
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => candidate,
            Err(_) => anyhow::bail!("memo store location is unavailable"),
        };
        let store_path = data_dir.join("default");
        let (initialized, store_exists) = match tokio::fs::symlink_metadata(&store_path).await {
            Ok(_) => {
                let canonical_store = tokio::fs::canonicalize(&store_path)
                    .await
                    .map_err(|_| anyhow::Error::msg("memo store location is unavailable"))?;
                anyhow::ensure!(
                    canonical_store.starts_with(&workspace),
                    "memo store location is outside the agent workspace"
                );
                let store_metadata = tokio::fs::metadata(&canonical_store)
                    .await
                    .map_err(|_| anyhow::Error::msg("memo store location is unavailable"))?;
                anyhow::ensure!(
                    store_metadata.is_dir(),
                    "memo store location is unavailable"
                );
                let marker = canonical_store.join("FORMAT_VERSION");
                let marker_exists = match tokio::fs::symlink_metadata(&marker).await {
                    Ok(_) => {
                        let canonical_marker = tokio::fs::canonicalize(&marker)
                            .await
                            .map_err(|_| anyhow::Error::msg("memo store is not initialized"))?;
                        anyhow::ensure!(
                            canonical_marker.starts_with(&canonical_store),
                            "memo store is outside the agent workspace"
                        );
                        tokio::fs::metadata(canonical_marker)
                            .await
                            .is_ok_and(|metadata| metadata.is_file())
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
                    Err(_) => anyhow::bail!("memo store is unavailable"),
                };
                (marker_exists, true)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (false, false),
            Err(_) => anyhow::bail!("memo store location is unavailable"),
        };
        Ok(Some((workspace, data_dir, initialized, store_exists)))
    }
}

fn empty_read_response(action: Option<&str>, wake_lines: usize) -> anyhow::Result<Value> {
    match action {
        Some("wake") => Ok(
            json!({"command":"wake", "ok":true, "complete":true, "lines":wake_lines, "items":[]}),
        ),
        Some("nap") => Ok(json!({"command":"nap", "ok":true, "status":"none", "pending":null})),
        _ => anyhow::bail!("memo workspace is unavailable"),
    }
}

struct CliOutput {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    success: bool,
}

async fn read_capped<R: AsyncRead + Unpin>(mut reader: R) -> anyhow::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut bounded = (&mut reader).take((MAX_OUTPUT_BYTES + 1) as u64);
    bounded.read_to_end(&mut bytes).await?;
    anyhow::ensure!(
        bytes.len() <= MAX_OUTPUT_BYTES,
        "memo output exceeded limit"
    );
    Ok(bytes)
}

fn sanitize_cli_response(value: &mut Value) {
    if let Some(object) = value.as_object_mut() {
        object.remove("store");
        object.remove("warning");
        if let Some(pending) = object.get_mut("pending").and_then(Value::as_object_mut) {
            pending.remove("command");
        }
        if let Some(error) = object.get_mut("error").and_then(Value::as_object_mut)
            && let Some(pending) = error.get_mut("pending").and_then(Value::as_object_mut)
        {
            pending.remove("command");
        }
    }
}

fn operation_args(args: &Value, wake_lines: usize) -> anyhow::Result<Vec<String>> {
    let object = args
        .as_object()
        .ok_or_else(|| anyhow::Error::msg("memo arguments must be an object"))?;
    let action = object
        .get("action")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::Error::msg("memo action is required"))?;
    let allowed: &[&str] = match action {
        "wake" => &["action"],
        "note" => &["action", "text"],
        "nap" => &["action", "range", "summary"],
        _ => anyhow::bail!("memo action must be wake, note, or nap"),
    };
    anyhow::ensure!(
        object.keys().all(|key| allowed.contains(&key.as_str())),
        "memo received an unsupported argument"
    );

    match action {
        "wake" => Ok(vec![
            "wake".into(),
            "--lines".into(),
            wake_lines.to_string(),
        ]),
        "note" => {
            let text = object
                .get("text")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow::Error::msg("memo note text is required"))?;
            anyhow::ensure!(
                !text.trim().is_empty()
                    && !text.contains(['\n', '\r'])
                    && text.len() <= MAX_TEXT_BYTES,
                "memo notes must be one nonempty line of at most 280 UTF-8 bytes"
            );
            Ok(vec!["note".into(), "--".into(), text.to_string()])
        }
        "nap" => match (object.get("range"), object.get("summary")) {
            (None, None) => Ok(vec!["nap".into()]),
            (Some(range), Some(summary)) => {
                let range = range
                    .as_str()
                    .ok_or_else(|| anyhow::Error::msg("memo nap range must be text"))?;
                validate_range(range)?;
                let summary = summary
                    .as_str()
                    .ok_or_else(|| anyhow::Error::msg("memo nap summary must be text"))?;
                anyhow::ensure!(
                    !summary.trim().is_empty()
                        && !summary.contains(['\n', '\r'])
                        && summary.len() <= MAX_TEXT_BYTES,
                    "memo summaries must be one nonempty line of at most 280 UTF-8 bytes"
                );
                Ok(vec![
                    "nap".into(),
                    "--".into(),
                    range.to_string(),
                    summary.to_string(),
                ])
            }
            _ => anyhow::bail!("memo nap requires both range and summary, or neither"),
        },
        _ => unreachable!("action was validated above"),
    }
}

fn validate_range(range: &str) -> anyhow::Result<()> {
    let Some((lo, hi)) = range.split_once('-') else {
        anyhow::bail!("memo nap range must use LO-HI form");
    };
    anyhow::ensure!(
        !lo.is_empty()
            && !hi.is_empty()
            && lo.bytes().all(|byte| byte.is_ascii_digit())
            && hi.bytes().all(|byte| byte.is_ascii_digit())
            && lo
                .parse::<u64>()
                .is_ok_and(|low| { hi.parse::<u64>().is_ok_and(|high| low <= high) }),
        "memo nap range must use an increasing numeric LO-HI range"
    );
    Ok(())
}

#[async_trait]
impl Tool for MemoTool {
    fn name(&self) -> &str {
        "memo"
    }

    fn description(&self) -> &str {
        "Use the append-only local memo store fixed to this agent's workspace. Call action=\"wake\" before every reply; it returns a bounded chronological window, not semantic search. For an incomplete wake, faithfully summarize only the supplied pending sources with action=\"nap\" (at most 280 UTF-8 bytes), then wake again until complete. After a note returns a pending request, summarize its two supplied sources and call nap; repeat nap until no request remains. Use note only for new, durable, verified facts or meaningful corrections, not duplicates, secrets, or task logs. Notes cannot be edited or deleted."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {"type": "string", "enum": ["wake", "note", "nap"]},
                "text": {"type": "string", "maxLength": MAX_TEXT_BYTES},
                "range": {"type": "string", "pattern": "^[0-9]+-[0-9]+$"},
                "summary": {"type": "string", "maxLength": MAX_TEXT_BYTES}
            },
            "required": ["action"],
            "additionalProperties": false
        })
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let operation = operation_args(&args, self.wake_lines)?;
        let read_only = operation.first().is_some_and(|op| op == "wake")
            || (operation.first().is_some_and(|op| op == "nap") && operation.len() == 1);
        let policy_operation = if read_only {
            ToolOperation::Read
        } else {
            ToolOperation::Act
        };
        if let Err(error) = self
            .security
            .enforce_tool_operation(policy_operation, "memo")
        {
            return Ok(ToolResult {
                success: false,
                output: ToolOutput::default(),
                error: Some(error),
            });
        }
        match self.invoke(&operation).await {
            Ok(value) => Ok(ToolResult {
                success: true,
                output: serde_json::to_string(&value)?.into(),
                error: None,
            }),
            Err(error) => Ok(ToolResult {
                success: false,
                output: ToolOutput::default(),
                error: Some(error.to_string()),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::from_str;

    fn result_value(result: ToolResult) -> Value {
        from_str(result.output.as_str()).expect("memo output is JSON")
    }

    #[test]
    fn operation_arguments_reject_paths_and_wrong_shapes() {
        assert!(operation_args(&json!({"action":"wake", "path":"/tmp/outside"}), 96).is_err());
        assert!(
            operation_args(&json!({"action":"note", "text":"x", "store":"shared"}), 96).is_err()
        );
        assert!(operation_args(&json!({"action":"nap", "range":"0-1"}), 96).is_err());
        assert!(
            operation_args(&json!({"action":"nap", "range":"2-1", "summary":"x"}), 96).is_err()
        );
    }

    #[test]
    fn note_and_summary_limits_are_utf8_bytes_and_single_line() {
        let valid = "é".repeat(140);
        assert!(operation_args(&json!({"action":"note", "text":valid}), 96).is_ok());
        assert!(operation_args(&json!({"action":"note", "text":"é".repeat(141)}), 96).is_err());
        assert!(operation_args(&json!({"action":"note", "text":"first\nsecond"}), 96).is_err());
        assert!(
            operation_args(&json!({"action":"nap", "range":"0-1", "summary":"x\r"}), 96).is_err()
        );
    }

    #[cfg(unix)]
    fn tool_with_cli_output(wake_lines: usize, output: &[u8]) -> (tempfile::TempDir, MemoTool) {
        use std::os::unix::fs::PermissionsExt;

        let workspace = tempfile::tempdir().expect("workspace");
        let store = workspace.path().join("memo/default");
        std::fs::create_dir_all(&store).expect("memo store");
        std::fs::write(store.join("FORMAT_VERSION"), b"1\n").expect("format marker");

        let response = workspace.path().join("response.json");
        std::fs::write(&response, output).expect("CLI response");
        let executable = workspace.path().join("memo-cli");
        let response_path = response.to_string_lossy().replace('\'', "'\\''");
        let script = format!(
            "#!/bin/sh\n[ \"$9\" = \"{wake_lines}\" ] || exit 3\nIFS= read -r output < '{response_path}' || :\nprintf '%s' \"$output\"\n"
        );
        std::fs::write(&executable, script).expect("fake CLI");
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700))
            .expect("executable permissions");

        let tool = MemoTool::new(
            executable.to_string_lossy(),
            workspace.path(),
            wake_lines,
            Arc::new(SecurityPolicy::default()),
        );
        (workspace, tool)
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn configured_256_line_wake_accepts_escaped_json_over_64_kib() {
        let escaped_max_length_text = "\u{1}".repeat(MAX_TEXT_BYTES);
        let items = (0..256)
            .map(|index| json!({"id":index, "text":escaped_max_length_text}))
            .collect::<Vec<_>>();
        let response = serde_json::to_vec(&json!({
            "command":"wake", "ok":true, "complete":true, "lines":256, "items":items
        }))
        .unwrap();
        assert!(response.len() > 64 * 1024);
        assert!(response.len() <= MAX_OUTPUT_BYTES);

        let (_workspace, tool) = tool_with_cli_output(256, &response);
        let result = tool.execute(json!({"action":"wake"})).await.unwrap();
        assert!(result.success, "{}", result.error.unwrap_or_default());
        let value = result_value(result);
        assert_eq!(value["lines"], 256);
        let items = value["items"].as_array().expect("wake items");
        assert_eq!(items.len(), 256);
        assert!(
            items
                .iter()
                .all(|item| item["text"].as_str() == Some(escaped_max_length_text.as_str()))
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn memo_cli_output_above_limit_is_rejected() {
        let oversized = vec![b' '; MAX_OUTPUT_BYTES + 1];
        let (_workspace, tool) = tool_with_cli_output(96, &oversized);
        let result = tool.execute(json!({"action":"wake"})).await.unwrap();
        assert!(!result.success);
        assert!(
            result
                .error
                .unwrap_or_default()
                .contains("memo output exceeded limit")
        );
    }

    #[tokio::test]
    async fn real_cli_isolates_workspaces_and_completes_pending_compaction() {
        let Ok(executable) = std::env::var("MEMO_TEST_EXECUTABLE") else {
            // The external CLI is not a workspace dependency. Set this to its
            // built binary for the focused local integration proof.
            return;
        };
        let first = tempfile::tempdir().expect("first workspace");
        let second = tempfile::tempdir().expect("second workspace");
        let security = Arc::new(SecurityPolicy::default());
        let first_tool = MemoTool::new(&executable, first.path(), 1, Arc::clone(&security));
        let second_tool = MemoTool::new(&executable, second.path(), 1, security);

        let empty = first_tool.execute(json!({"action":"wake"})).await.unwrap();
        assert!(empty.success);
        assert_eq!(result_value(empty)["items"], json!([]));

        for text in [
            "--store research remains note text",
            "second durable fact",
            "third durable fact",
            "fourth durable fact",
        ] {
            let stored = first_tool
                .execute(json!({"action":"note", "text":text}))
                .await
                .unwrap();
            assert!(stored.success, "{}", stored.error.unwrap_or_default());
            let stored = result_value(stored);
            assert!(stored.get("store").is_none());
            assert!(!stored.to_string().contains(first.path().to_str().unwrap()));
        }
        let mut pending_count = 0;
        let mut last_sources = Vec::new();
        loop {
            let wake = first_tool.execute(json!({"action":"wake"})).await.unwrap();
            assert!(wake.success, "{}", wake.error.unwrap_or_default());
            let wake = result_value(wake);
            if wake["complete"] == true {
                assert!(wake.get("store").is_none());
                assert!(!wake.to_string().contains(first.path().to_str().unwrap()));
                assert_eq!(wake["items"].as_array().unwrap().len(), 1);
                assert_eq!(pending_count, 3);
                assert!(last_sources.iter().all(|kind| kind == "summary"));
                assert_eq!(
                    wake["items"][0]["text"],
                    json!(
                        "--store research remains note text; second durable fact; third durable fact; fourth durable fact"
                    )
                );
                break;
            }
            let pending = wake
                .pointer("/error/pending")
                .expect("typed pending request");
            assert!(pending.get("command").is_none());
            last_sources = pending["sources"]
                .as_array()
                .unwrap()
                .iter()
                .map(|source| source["kind"].as_str().unwrap().to_owned())
                .collect();
            let source_text = pending["sources"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|source| source["text"].as_str())
                .collect::<Vec<_>>()
                .join("; ");
            let range = format!("{}-{}", pending["range"]["lo"], pending["range"]["hi"]);
            let nap = first_tool
                .execute(json!({"action":"nap", "range":range, "summary":source_text}))
                .await
                .unwrap();
            assert!(nap.success, "{}", nap.error.unwrap_or_default());
            let nap = result_value(nap);
            assert!(nap.get("store").is_none());
            assert!(!nap.to_string().contains(first.path().to_str().unwrap()));
            pending_count += 1;
        }

        let other = second_tool.execute(json!({"action":"wake"})).await.unwrap();
        assert!(other.success);
        let other = result_value(other);
        assert_eq!(other["items"], json!([]));
        assert!(!other.to_string().contains(first.path().to_str().unwrap()));
        let note_log = first.path().join("memo/default/notes.log");
        assert!(
            std::fs::read_to_string(note_log)
                .unwrap()
                .contains("--store research remains note text")
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symlinked_store_outside_workspace_is_rejected_before_cli_execution() {
        let workspace = tempfile::tempdir().expect("workspace");
        let outside = tempfile::tempdir().expect("outside store");
        std::os::unix::fs::symlink(outside.path(), workspace.path().join("memo")).unwrap();
        let tool = MemoTool::new(
            "/missing/memo",
            workspace.path(),
            96,
            Arc::new(SecurityPolicy::default()),
        );
        let result = tool
            .execute(json!({"action":"note", "text":"no escape"}))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(
            result
                .error
                .unwrap_or_default()
                .contains("outside the agent workspace")
        );
        assert!(std::fs::read_dir(outside.path()).unwrap().next().is_none());
    }
}
