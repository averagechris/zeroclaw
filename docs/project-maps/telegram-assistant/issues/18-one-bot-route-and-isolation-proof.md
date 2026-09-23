# One-bot route and isolation proof

Type: research
Status: resolved
Blocked by: [Bot identity topology](05-bot-identity-topology.md), [Owner approval posture](07-owner-approval-posture.md)

## Question

What is the smallest fail-closed routing and state contract for one Telegram
alias to serve the owner DM, partner DM, and household group through separate
agents, given that only the owner may use shell?

## Answer

Use one bot with a strict, Telegram-only route table on its channel alias.
This is a design decision, **not** evidence that the feature already works.
If the implementation cannot meet the checks below, stop before enabling
owner shell and return to the three-bot fallback.

### Ownership and dispatch

- The route table on `[channels.telegram.<alias>]` is the sole agent binding
  for that alias. A routed alias cannot also appear in any
  `agents.<agent>.channels` list. Keep existing agent-to-channel bindings for
  aliases without routes. The new config field and its spelling are proposed,
  not part of the current ZeroClaw schema.
- Reject malformed or empty tables, duplicate IDs, missing or disabled agent
  aliases, conflicting conventional bindings, and a group route to an agent
  that is also assigned a DM in this V1 layout. Match private chats by
  positive numeric chat ID **and** equal numeric `platform_sender_id`;
  match the group by its exact negative numeric chat ID. Ignore mutable
  usernames for routing. Continue to apply the Telegram peer allowlist first.
- `AgentRouter::resolve` checks a routed alias before its legacy owner map.
  A routed alias returns its matched agent context or `None`, never a bare
  Telegram fallback or a default owner. Channel collection must treat the
  route table as a live binding; the legacy "first enabled agent" fallback
  must not assign a routed key. Check other single-owner lookups such as
  channel startup, model-picker, transcription, and session hydration rather
  than creating an implicit host agent.

### Tool and state boundary

- The selected `ChannelRuntimeContext` supplies that turn's risk profile,
  scoped tools, memory, and workspace. Each profile states exactly the tools
  it wants in a non-empty `allowed_tools` list, and each rollout stage
  appends to that list. No profile uses `deny_all_tools`, and no profile
  relies on an empty list, because `allowed_tools = []` means unrestricted.
  There are no MCP bundles in V1.

  | Stage | Owner | Partner | Household |
  |---|---|---|---|
  | 1. Routing baseline | memory tools | memory tools | memory tools |
  | 2. Kagi | + `web_search_tool` | + `web_search_tool` | + `web_search_tool` |
  | 3. Owner shell | + `shell` | unchanged | unchanged |

  "Memory tools" means `memory_recall`, `memory_store`, and `memory_forget`
  (see [Memory tools](21-memory-tools.md)). Partner and household profiles
  also list `shell` in `excluded_tools`, so a later edit to `allowed_tools`
  cannot readmit it by accident.
- The three fixed chats already have distinct history keys because the key
  includes Telegram chat ID. With `per_user_session = false`, both people in
  the group share its key but neither DM shares it. The key does **not**
  contain agent alias. Do not repoint an existing chat route to a different
  agent while reusing its old transcript. For V1, stop the daemon, reset or
  archive that chat's prior session before changing its route, and restart.
  Live route reassignment and automatic session migration are not supported.
- V1 is text-only. Attachment materialization must return before any
  `getFile` or download into a shared channel directory, and transcription
  stays disabled. Missing media tools alone do not block inbound downloads.
  Leaving `workspace_dir` unset is **not** a config option: the daemon's
  channel collection always calls
  `.with_workspace_dir(config.channel_workspace_dir("telegram.<alias>"))`
  (`crates/zeroclaw-channels/src/orchestrator/mod.rs`), and the same field
  drives channel-level skill loading. The route patch therefore needs an
  explicit text-only guard on routed aliases that drops attachments before
  materialization, rather than relying on an unset field. Design
  agent-owned media storage for the requested multimodal fast follow.
- The pre-route SOP gate reads shared channel/config handles, not the
  selected agent's tools, but leave SOP runtime off in V1. Approval callbacks
  are restricted to the originating chat and allowlisted responders. The only
  prompted V1 tool is `memory_forget`; in the group either member may answer
  its prompt. Neither pre-route messages nor model-picker callbacks
  may select the owner context for an unmapped identity.

### Proof and reversal

Before owner shell is enabled, test numeric DM routes, exact group routing,
unknown identities/chats, missing IDs, mixed DM/group IDs, conflicting
bindings, and no fallback. Test a real partner dispatch attempting `shell`
and show it is denied by the partner context. Test the three session keys,
group sharing, `/new`, `/model`, and no cross-agent memory recall. For
memory, store a fact through `memory_store` in one agent and show that
neither `memory_recall` nor turn-start memory injection surfaces it in the
other two. Send an
authorized photo to the routed channel with a mock Telegram API and verify
that no `getFile` request or channel-level file write occurs. Verify that
gate and model-picker inputs from unmapped senders cannot invoke owner tools.
Run the three profiles through the existing NixOS module before turning on
unprompted shell.

Three stock bot identities remain the fallback if startup validation or these
boundary tests expose a larger patch than expected. Reverting requires three
channel tokens and explicit agent bindings. Prior `telegram.<alias>_*`
sessions are not automatically migrated; archive them rather than exposing
them to another agent. The upstream principal/RBAC work does not replace this
agent route (see [Upstream alignment](11-upstream-alignment.md)).

## Artifacts

- Proposal: [Topology options](../artifacts/topology-options.md) compares the
  one-bot fork change and the three-bot fallback.
