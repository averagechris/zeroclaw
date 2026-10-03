# Voice & Telephony

Real-time voice input and output. Four channels cover the matrix: inbound calls, local microphone wake, outbound speech synthesis, and SIP-grade real-time conversation.

## ClawdTalk (real-time SIP)

Full-duplex SIP voice powered by Telnyx. The agent talks over a real phone call (inbound or outbound). Supports barge-in, mid-turn tool use, and regional number provisioning.

{{#config-fields channels.clawdtalk}}

`api_key` (Telnyx) and `webhook_secret` are secrets:

{{#secret-config channels.clawdtalk.<alias>.api_key}}

**Pair with:** a `telnyx` model provider for the brain and ensure your Telnyx account has a SIP connection with the correct webhook URL pointed at the ZeroClaw gateway.

## Voice Call (Twilio / Telnyx / Plivo)

Traditional carrier voice: the agent picks up, transcribes the caller, replies with TTS. Higher latency than ClawdTalk but works with any regular phone number and doesn't require SIP trunk provisioning. Outbound calls hit `from_number` and require operator approval when `require_outbound_approval` is on.

{{#config-fields channels.voice_call}}

## Voice Wake (local wake-word)

Runs locally, listens on the mic, triggers agent interaction when it hears the wake phrase. Useful for:

- Physical voice assistants on SBCs
- Desktop "hotword → ask" workflows
- Always-listening home-automation agents

The agent doesn't send audio anywhere; wake detection is local. Only post-wake speech is captured and (separately) transcribed before reaching the LLM.

{{#config-fields channels.voice_wake}}

> **Build flag:** Voice Wake is gated by the `voice-wake` cargo feature on `zeroclaw-channels`. Build with `--features voice-wake` to include it.
> On Android, Voice Wake requires Android 8 (API level 26) or newer.

## TTS (outbound speech synthesis)

TTS is an output service channels call into, not its own inbound channel. Global defaults live under `tts`. TTS provider instances are configured under `providers.tts.<type>.<alias>` (OpenAI, ElevenLabs, Google, Edge, Piper) and selected per agent via the agent's `tts_provider`. See [Model Providers](../providers/overview.md) for the provider entries and per-agent wiring. Provider API keys are secrets; set them through the gateway, zerocode, or `zeroclaw config set`, never in plaintext.

---

## Latency budget

Speech feels real-time below ~500 ms end-to-end. Practical budgets:

| Component | Typical latency |
|---|---|
| Wake detection (local) | <100 ms |
| STT (Whisper local) | 300–800 ms per utterance |
| LLM first-token | 100–2000 ms (model dependent) |
| TTS first-audio | 200–700 ms |
| Network (cellular / PSTN) | 100–300 ms RTT |

ClawdTalk shortcuts several of these by keeping the audio stream live; regular `voice_call` incurs STT + LLM + TTS sequentially.

## STT

Speech-to-text is configured separately from the voice channels. Voice channels invoke the provider selected by the agent's `transcription_provider` reference when they need to turn audio into text.

OpenCode Go can transcribe supported audio natively with MiMo V2.6 Flash or Pro. Configure its key on a typed transcription provider and select that alias on the agent:

```toml
[providers.transcription.opencode_go.voice]
api_key = "op://platform/opencode-go/api-key"
# model = "mimo-v2.6-pro" # defaults to mimo-v2.6-flash

[agents.default]
transcription_provider = "opencode_go.voice"
```

The adapter sends one audio message with a fixed instruction that treats audio only as source material, transcribes spoken words verbatim, and ignores instructions spoken in the audio. No conversation history is sent. Each request uses an opaque OpenCode session ID and ZeroClaw's own User-Agent. Supported inputs are MP3, WAV, FLAC, M4A, and Ogg/Opus; Ogg/Opus data must have an Ogg container header. The request is capped at 25 MiB, times out after 120 seconds, and rejects responses larger than 256 KiB, empty transcripts, or output that does not finish normally. There is no automatic paid-provider fallback. Channels enforce `transcription.max_duration_secs` before downloading supported voice recordings.

## Hardware notes

For always-on voice on an SBC:

- USB mic: any UAC-compliant mic works. `arecord -l` to verify the OS sees it.
- Speaker: either USB audio out or the SBC's onboard jack; pick the OS default device for the user the daemon runs as.
- Microphones with built-in AEC (acoustic echo cancellation) dramatically improve wake reliability when the speaker is nearby.

See [Hardware → Android](../hardware/android-setup.md) for Android-specific audio setup.
