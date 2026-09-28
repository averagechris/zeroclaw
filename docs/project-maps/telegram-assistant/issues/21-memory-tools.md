# Memory tools

Type: grilling
Status: resolved
Blocked by: none

## Question

Should the three agents get long-term memory tools in V1, and which memory
mechanism should back them: ZeroClaw's built-in memory, or a community
option?

## Answer

Yes. Use ZeroClaw's built-in memory tools on the default SQLite backend for
all three agents from the first routing stage. Do not add a community memory
server in V1.

Grants, per agent (see the stage table in
[One-bot route and isolation proof](18-one-bot-route-and-isolation-proof.md)):

- `memory_recall` and `memory_store` run without a prompt. `memory_store` is
  not in the default `auto_approve` list, so without this every save would
  post an approval prompt in Telegram. Setting a profile's `auto_approve`
  replaces the serde default rather than extending it, so list the V1 tools
  explicitly: `memory_recall`, `memory_store`, and from stage 2
  `web_search_tool`.
- `memory_forget`: keep its default approval prompt. In the group either
  member may answer it.
- Do not grant `memory_export` or `memory_purge` in V1. The owner can manage
  memory from the host shell or CLI.

Keep `[memory] auto_save` at its default (`true`), which saves what the
humans say as conversation memory, not the agent's replies. Keep
`agents.<alias>.workspace.read_memory_from` empty, so each agent recalls only
its own rows.

What the evidence fixes:

- Built-in tools are `memory_recall`, `memory_store`, `memory_forget`,
  `memory_export`, and `memory_purge`
  (`docs/book/src/tools/overview.md`). They are a first-party runtime
  contract (`docs/book/src/developing/tool-inventory.md`).
- SQLite is the default backend (`default_memory_backend` in
  `crates/zeroclaw-config/src/schema.rs`, ADR-005). The shared store wraps
  each agent in `AgentScopedMemory`, which stamps `agent_id` on writes and
  filters recall, so the three agents are isolated by default
  (`docs/book/src/agents/internals.md`).
- Turn-start recall injects a bounded `[Memory context]` block independent of
  the tools (`docs/book/src/architecture/memory-payload-lifecycle.md`). The
  isolation test must cover this path as well as `memory_recall`.
- `embedding_provider` defaults to `"none"`, so recall is keyword-based
  rather than semantic. Whether the Codex subscription login can serve
  embeddings was not checked. Adding an embedding provider is a later
  quality improvement, not a V1 requirement.
- The opt-in `knowledge` tool adds a relationship graph. It is off by default
  and not needed for V1.

Why not a community option in V1:

- Other built-in backends (Markdown, PostgreSQL, Qdrant, Lucid) exist but add
  services or lose features without a V1 need.
- WASM memory plugins have a contract but are not yet constructible as a
  configured backend (`docs/book/src/plugins/writing-a-memory-plugin.md`,
  "Wiring status").
- An external MCP memory server would bypass `AgentScopedMemory` and turn-start
  injection. Isolation would need one server entry per agent, and it would add
  a second, overlapping memory surface. Community servers were not surveyed
  in depth; revisit on a concrete request.

## Artifacts

None.
