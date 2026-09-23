# Credential and OAuth ownership

Type: research
Status: resolved
Blocked by: none

## Question

Where would each person's integration tokens (email, calendar, and similar)
live, who runs the OAuth or PKCE consent, and can ZeroClaw keep the owner's
tokens out of reach of the partner's and household's turns?

## Answer

ZeroClaw does not own user-integration OAuth today, and the built-in
integration tools are single-account. Per-principal credentials are
expressible only by running one integration process per principal and
granting it per agent. That works at the connection boundary, but the secret
store underneath is flat.

Evidence, point by point:

- ZeroClaw's PKCE code is model-provider login only:
  `crates/zeroclaw-providers/src/auth/{oauth_common,openai_oauth,gemini_oauth,xai_oauth}.rs`.
  Nothing in the MCP client (`crates/zeroclaw-tools/src/mcp_transport.rs`,
  `mcp_tool.rs`) performs an OAuth authorization-code flow; HTTP and SSE
  servers take static `headers`, stdio servers take static `env`. Both are
  stored as secrets (`docs/book/src/tools/mcp.md`).
- The built-in `google_workspace` tool wraps the `gws` CLI and reads one
  global `[google_workspace]` section with one `credentials_path` and one
  `default_account` (`GoogleWorkspaceConfig` in
  `crates/zeroclaw-config/src/schema.rs`). Consent happens in `gws auth login`
  on the host. There is no per-agent account selection. The same shape holds
  for `[composio]` (one `api_key`, one `entity_id`) and `[ms365]`.
- Per-agent secret namespacing is not supported: "there is a single
  workspace-wide `SecretStore`" (`docs/book/src/agents/internals.md`,
  "Not supported today"). Master key acquisition follows ADR-013 with one
  `.secret_key` per config root.
- `mcp_bundles` decide which servers an agent connects to at session
  construction (`Config::mcp_servers_for_agent`). Omission is not a grant.
  Bundle changes apply on session restart.

What that buys and what it does not:

- Buys: define `gmail-owner`, `gmail-partner`, and `gmail-household` as
  separate `[[mcp.servers]]` entries, each with its own `env` (token directory
  or client secrets path) and grant each through its own bundle. The partner
  agent never opens a connection to the owner's server, so the partner's
  turns cannot call the owner's mailbox. Token files land wherever the MCP
  server writes them, so per-server directories give per-principal token
  ownership on disk.
- Does not buy: protection against an agent that can read files or run shell
  on the host. The encrypted secret values and the `.secret_key` file sit in
  one config root. The fence for the partner and household agents is
  therefore the capability list (no `shell`, no `file_read` outside their
  workspace), not the secret store. Sandboxing and `forbidden_paths` help,
  but they are defence in depth, not the boundary.
- The consent step for a stdio MCP server that speaks Google OAuth normally
  opens a browser to a loopback callback on the machine running the server.
  The partner would complete consent on the owner's desktop, or through a
  URL the server prints with a port forward. Which servers support a
  headless or printed-URL flow is unresolved; see
  [Per-principal integration mechanism](09-per-principal-integration-mechanism.md).
- A household integration identity means a third account (or a shared
  calendar owned by one person and exposed through the household server).
  That is a product choice, tracked in
  [Household identity and integrations](08-household-identity-and-integrations.md).

## Artifacts

None.
