# Tool extension options

Helps answer [Tool extension DX](../issues/15-tool-extension-dx.md) in the
[Telegram assistant map](../map.md): which ZeroClaw extension path should
carry new tools over time, starting with Kagi. It also informs
[Kagi search tool](../issues/14-kagi-search-tool.md) because the path chosen
for Kagi fixes its tool name, its grant mechanism, and its approval shape.
This is a comparison grounded in the current fork checkout, not a decision.
The owner chose native Kagi search and to select future extension paths per
concrete tool. This comparison remains supporting evidence, not authority.

## The four paths ZeroClaw has today

| Path | Where it lives | Grant per agent | Secret handling | Build requirement |
|---|---|---|---|---|
| Native provider inside `web_search_tool` | `crates/zeroclaw-tools/src/web_search_tool.rs`, `web_search_provider_routing.rs`, `WebSearchConfig` in `crates/zeroclaw-config/src/schema.rs`, registration in `crates/zeroclaw-runtime/src/tools/mod.rs` | Risk profile `allowed_tools` / `excluded_tools`; per-channel `excluded_tools` | `#[secret]` field per provider, encrypted at rest, re-read and decrypted on demand (`resolve_*_api_key` / `reload_*_api_key`) | Rust change in this fork, then upstream PR |
| MCP server | `[[mcp.servers]]` plus `[mcp_bundles.*]`; docs in `docs/book/src/tools/mcp.md` | `agents.<alias>.mcp_bundles`; omission is not a grant | `headers` (http, sse) and `env` (stdio) stored as secrets | None. Config only |
| WASM tool plugin | `crates/zeroclaw-plugins`; docs in `docs/book/src/plugins/index.md`, `writing-a-tool-plugin.md` | Discovered tools "appear in the agent's tool set" (plugins/index.md, "Current wiring status"); narrowing goes through the risk profile tool lists | Manifest `config_schema` with `x-secret = true`; guest reads through scoped `secrets.get` at point of use | Source build with a `plugins-wasm-*` feature; prebuilt binaries omit it (`docs/book/src/tools/skill-bundles.md` warning) |
| Skill with `kind = "http"` | `SKILL.toml`; loader in `crates/zeroclaw-runtime/src/skills/mod.rs`, executor in `crates/zeroclaw-runtime/src/tools/skill_http.rs` | `agents.<alias>.skill_bundles` | None. `SkillHttpTool` issues `client.get(target.url)` only; no header or body support | None. Files only |

## Why Kagi fits the native provider path

Evidence from the checkout:

- `web_search_tool` already routes among nine providers.
  `resolve_web_search_provider` in `web_search_provider_routing.rs` maps the
  `[web_search] search_provider` string to a `WebSearchProviderRoute`
  variant, and `WebSearchTool::execute` dispatches on it. Adding Kagi is one
  more variant, one `search_kagi` plus `search_kagi_with_client` pair (the
  injectable-client test pattern Tavily, Bocha, Serply, and Keenable use),
  one `parse_kagi_results`, and one `kagi_api_key` secret field.
- The tool is wrapped in `RateLimitedTool` at registration
  (`crates/zeroclaw-runtime/src/tools/mod.rs`, "Web search tool" block), so
  the spend guard from [Kagi search options](kagi-search-options.md) comes
  for free.
- `web_search_tool` is in `default_auto_approve()`
  (`crates/zeroclaw-config/src/schema.rs`), so a search from Telegram runs
  without a prompt under `supervised`. That is the behavior a shared search
  tool wants.
- The repo's own inventory says keep it first-party: "Keep first-party while
  SSRF, allowlist, provider routing, and receipt behavior remain
  ZeroClaw-owned" (`docs/book/src/developing/tool-inventory.md`, row for
  `http_request`, `web_fetch`, `web_search_tool`).
- Nine providers landed this way upstream. A tenth is a routine upstream PR
  shape, which means the fork can carry nothing once it merges. The files
  involved are a few thousand lines, not the 24k and 52k line files a
  routing patch would touch.

Costs and limits of this path:

- `[web_search]` is one global section. One provider and one key per
  daemon, shared by every agent. That matches the owner's "one deployment
  key" constraint but cannot give the partner a separate key later without
  a per-agent override field that does not exist today.
- Tool contract is fixed by the existing schema: `query` only. Lenses,
  `workflow`, and filters would be config-side (a named inline lens under
  `[web_search]`), not model-visible parameters, unless the shared schema
  grows. That is arguably the right default for a household bot.
- It needs a Rust build, tests at the provider boundary, a docs line in
  `docs/book/src/tools/overview.md`, and the generated config field table.

## Why MCP is the strongest zero-code alternative for Kagi

Kagi ships its own MCP server (see [Kagi search options](kagi-search-options.md)).
Two stock configurations would work today:

```toml
# Hosted by Kagi. Sends queries to the same party as the raw API.
[[mcp.servers]]
name = "kagi"
transport = "http"
url = "https://mcp.kagi.com/mcp"
headers = { Authorization = "Bearer <set through the masked secret prompt>" }

# Or local stdio. Needs `uv` on the host; `KAGI_HIDDEN_PARAMS` trims the schema.
[[mcp.servers]]
name = "kagi"
command = "uvx"
args = ["kagimcp"]
# env.KAGI_API_KEY and env.KAGI_HIDDEN_PARAMS set as secrets

[mcp_bundles.search]
servers = ["kagi"]
```

What changes compared with the native path:

- Tool names become `kagi__kagi_search_fetch` and `kagi__kagi_extract`.
  Under `supervised`, an MCP tool prompts unless its prefixed name is in
  `auto_approve` (`docs/book/src/tools/mcp.md`, "Security and approval").
  A shared search tool that prompts every time is unusable in a group, so
  `auto_approve` must list it.
- `kagi__kagi_extract` bills separately. Under a non-empty `allowed_tools`
  it is auto-admitted with every other `<server>__<tool>` name, so
  `excluded_tools = ["kagi__kagi_extract"]` is required on every profile
  that gets the bundle. This is the "exact exclusions matter" rule from
  `docs/book/src/tools/mcp.md`, "Authorization".
- The model-facing schema is Kagi's, not ours. `KAGI_HIDDEN_PARAMS` can hide
  `extract_count`, `workflow`, `limit`, and lens fields on the stdio
  install; the hosted server offers no such control.
- The `RateLimitedTool` wrapper applies to built-ins at registration. Whether
  MCP tool calls get an equivalent cap was not verified in this pass and
  should be checked before relying on it.
- Per-agent grant is cleaner than the native path: a bundle per agent, and a
  second Kagi server entry with a second key is one more `[[mcp.servers]]`
  block if the partner ever needs her own key.

MCP is the right default for later tools that already exist as servers
(Google Workspace per principal in
[Per-principal integration mechanism](../issues/09-per-principal-integration-mechanism.md),
and most third-party services). For Kagi specifically it trades a small Rust
change for a Python process or a hosted hop, a noisier tool schema, and two
mandatory profile edits.

## Where WASM plugins fit

The plugin host gives the strongest boundary: typed manifest config,
schema-designated secrets read at point of use, fuel and memory ceilings,
an egress policy, and Ed25519 signature gating
(`docs/book/src/plugins/index.md`, "Where the trust boundary actually is").
It is also the most expensive path:

- Prebuilt binaries omit `plugins-wasm`; the fork would build with
  `--features plugins-wasm-cranelift` or similar, and the NixOS module would
  need to match.
- Plugins are "a pre-1.0 experimental surface" and typed config enforcement
  "ships with the feature" with no shim
  (`docs/book/src/plugins/migrating-to-typed-config.md`).
- One tool per component, plus signing and distribution machinery that a
  household of two does not need.

The plugin path earns its cost when a tool must run untrusted or third-party
code with a narrow egress allowlist, or when the tool will be distributed.
Neither is true for Kagi. It may be true for a later meme or image tool if it
pulls in an image-processing dependency the owner does not want in the core
binary.

## Where skills fit

Skills are instructions and workflows with optional tools of kind `shell`,
`http`, or `script`. The `http` kind is GET-only with no headers
(`SkillHttpTool` in `crates/zeroclaw-runtime/src/tools/skill_http.rs`), so it
cannot send a bearer token or a JSON body and cannot call Kagi. Skills are
the right place for "how to search well for recipes" prompts layered on top
of whichever search tool exists, and for slash commands
(`docs/book/src/tools/skills.md`, "Slash command options"). They are not a
vendor credential boundary.

## Likely mapping for later deferred tools

Not decisions, only where the evidence points:

- Meme creation and image manipulation: MCP if a maintained server exists;
  WASM plugin if the work needs a native image library and the owner wants
  it out of the core binary; native only if it becomes a core capability.
- Audio or video transcription from a URL: MCP first. ZeroClaw already has a
  per-agent `transcription_provider` reference for inbound voice, so check
  whether that provider path can be reused before adding a tool at all.

## Security and developer-experience summary

| Concern | Native provider | MCP | WASM plugin | Skill |
|---|---|---|---|---|
| Key at rest | Encrypted field, decrypted on demand | Encrypted `headers` / `env` | Encrypted, scoped `secrets.get` | Not supported |
| Who can call it | Risk profile tool lists | Bundle per agent, then tool lists | Tool lists after discovery | Skill bundle per agent |
| Unprompted from Telegram | Yes, in `default_auto_approve()` | Only with prefixed name in `auto_approve` | Only with name in `auto_approve` | n/a |
| Surprise cost surface | None beyond search | `kagi_extract` and `extract_count` unless excluded or hidden | Depends on plugin | n/a |
| Rate limit | `RateLimitedTool` wrapper | Unverified | Fuel and connection caps, not request rate | n/a |
| Code to write | Rust, one provider slice, upstream-able | None | Rust to WASM, manifest, signing, source build | None |
| Tests and docs | Existing provider test pattern; `tools/overview.md` line | Config and a smoke test | Native unit tests plus host fixtures | Frontmatter validation |
| Fork carry | Zero after upstream merge | Zero | Feature flag and build matrix | Zero |

## Recommendation (labeled)

For Kagi, the native provider path looks like the best fit because the
routing seam, the secret pattern, the rate limiter, the default
auto-approve, and the upstream PR shape already exist. For most later tools,
MCP is the default because per-agent bundles and per-server secrets are
exactly the grant model this effort needs. Reserve WASM plugins for
isolation or distribution needs that actually appear. Use skills for
workflow guidance on top of tools, never as the credential boundary.

The cheapest check that could change this: try the hosted Kagi MCP server in
a throwaway config root for an afternoon. If the model-facing schema and the
prompt behavior are acceptable with `auto_approve` and `excluded_tools` set,
the zero-code path may be good enough and the Rust slice can wait.
