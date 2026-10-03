//! Native image calls reuse the provider's selected, refreshable Codex login.
use super::*;
use serde_json::json;

const RESPONSE_LIMIT: usize = 40 * 1024 * 1024;
const IMAGE_LIMIT: usize = 20 * 1024 * 1024;

pub struct CodexImageRequest {
    pub model: String,
    pub image_model: String,
    pub prompt: String,
    /// Already validated, bounded image data URLs from the caller's workspace.
    pub images: Vec<String>,
    pub size: String,
    pub quality: String,
    pub background: String,
}

impl OpenAiCodexModelProvider {
    pub async fn generate_image(&self, request: CodexImageRequest) -> anyhow::Result<String> {
        let creds = self.resolve_credentials().await?;
        let mut content = vec![json!({"type":"input_text", "text":request.prompt})];
        for image in request.images {
            content.push(json!({"type":"input_image", "image_url":image}));
        }
        let body = json!({
            "model":normalize_model_id(&request.model),
            "instructions":"Create or edit the requested image using the image generation tool. Preserve requested text and details.",
            "input":[{"role":"user", "content":content}],
            "store":false, "stream":true,
            "reasoning":{"effort":resolve_reasoning_effort(&request.model, self.reasoning_effort.as_deref())},
            "tools":[{"type":"image_generation", "model":request.image_model,
                "size":request.size, "quality":request.quality, "background":request.background,
                "output_format":"png"}],
            "tool_choice":{"type":"image_generation"}
        });
        let response = self
            .responses_request_builder(
                &creds.bearer_token,
                creds.account_id.as_deref(),
                creds.access_token.as_deref(),
                creds.use_gateway_api_key_auth,
                true,
            )
            .timeout(std::time::Duration::from_secs(180))
            .json(&body)
            .send()
            .await
            .map_err(|_| anyhow::Error::msg("Codex image request failed"))?;
        if !response.status().is_success() {
            // Remote bodies may contain user data or credential-bearing URLs.
            anyhow::bail!("Codex image request was rejected");
        }
        let mut state = ImageStream::default();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| anyhow::Error::msg("Codex image stream failed"))?;
            state.push(&chunk)?;
        }
        state.finish()
    }
}

#[derive(Default)]
struct ImageStream {
    pending: Vec<u8>,
    received: usize,
    scanned: usize,
    completed: bool,
    image: Option<String>,
}

impl ImageStream {
    fn push(&mut self, chunk: &[u8]) -> anyhow::Result<()> {
        self.received = self.received.saturating_add(chunk.len());
        anyhow::ensure!(
            self.received <= RESPONSE_LIMIT,
            "Codex image response exceeded the limit"
        );
        self.pending.extend_from_slice(chunk);
        let mut consumed = 0;
        let mut scan_from = self.scanned;
        while let Some(offset) = self.pending[scan_from..].iter().position(|&b| b == b'\n') {
            let end = scan_from + offset;
            let line = self.pending[consumed..end].to_vec();
            self.line(&line)?;
            consumed = end + 1;
            scan_from = consumed;
        }
        self.pending.drain(..consumed);
        self.scanned = self.pending.len();
        Ok(())
    }

    fn line(&mut self, line: &[u8]) -> anyhow::Result<()> {
        let Some(data) = line.strip_prefix(b"data:") else {
            return Ok(());
        };
        let data = data.strip_prefix(b" ").unwrap_or(data);
        if data.trim_ascii() == b"[DONE]" {
            return Ok(());
        }
        let event: Value = serde_json::from_slice(data)
            .map_err(|_| anyhow::Error::msg("Invalid Codex image event"))?;
        match event.get("type").and_then(Value::as_str) {
            Some("response.output_item.done") => self.item(&event["item"])?,
            Some("response.completed") => {
                self.completed = true;
                if let Some(items) = event.pointer("/response/output").and_then(Value::as_array) {
                    for item in items {
                        self.item(item)?;
                    }
                }
            }
            Some("response.failed" | "response.incomplete" | "error") => {
                anyhow::bail!("Codex image generation failed")
            }
            _ => {}
        }
        Ok(())
    }

    fn item(&mut self, item: &Value) -> anyhow::Result<()> {
        if item.get("type").and_then(Value::as_str) != Some("image_generation_call") {
            return Ok(());
        }
        let image = item
            .get("result")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow::Error::msg("Codex returned no image"))?;
        anyhow::ensure!(
            image.len() <= IMAGE_LIMIT.div_ceil(3) * 4,
            "Codex image exceeded the limit"
        );
        self.image = Some(image.to_owned());
        Ok(())
    }

    fn finish(mut self) -> anyhow::Result<String> {
        if !self.pending.is_empty() {
            let line = std::mem::take(&mut self.pending);
            self.line(&line)?;
        }
        anyhow::ensure!(self.completed, "Codex image stream ended before completion");
        self.image
            .ok_or_else(|| anyhow::Error::msg("Codex returned no image"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn codex_image_item_survives_empty_completion_and_chunked_utf8() {
        let body = concat!(
            "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"image_generation_call\",\"result\":\"aW1hZ2U=\",\"revised_prompt\":\"gatito 😼\"}}\r\n\r\n",
            "data:{\"type\":\"response.completed\",\"response\":{\"output\":[]}}\n\n"
        );
        let mut state = ImageStream::default();
        for chunk in body.as_bytes().chunks(7) {
            state.push(chunk).unwrap();
        }
        assert_eq!(state.finish().unwrap(), "aW1hZ2U=");
    }
    #[test]
    fn failed_or_truncated_images_are_not_delivered() {
        let mut state = ImageStream::default();
        state.push(b"data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"image_generation_call\",\"result\":\"aW1hZ2U=\"}}\n").unwrap();
        assert!(state.finish().is_err());
        let mut state = ImageStream::default();
        assert!(state.push(b"data: {\"type\":\"response.failed\",\"error\":{\"message\":\"private fixture\"}}\n").unwrap_err().to_string().find("private fixture").is_none());
    }
    #[test]
    fn oversized_image_stream_is_rejected() {
        let mut state = ImageStream {
            received: RESPONSE_LIMIT,
            ..Default::default()
        };
        assert!(state.push(b"x").is_err());
        let mut state = ImageStream::default();
        assert!(state.item(&json!({"type":"image_generation_call", "result":"x".repeat(IMAGE_LIMIT.div_ceil(3)*4+1)})).is_err());
    }
}
