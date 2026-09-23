# Tool gating on the Telegram path

Type: research
Status: resolved
Blocked by: none

## Question

When an agent serving a Telegram chat wants to run a tool, what actually
decides whether it runs, who gets asked, and who is allowed to answer?

## Answer

Three layers decide, and they behave differently from what the autonomy doc
implies for `shell`.

Layer 1, capability. The risk profile's `allowed_tools`, `excluded_tools`,
and `deny_all_tools` (`RiskProfileConfig` in
`crates/zeroclaw-config/src/schema.rs`) decide whether the model can see and
call a tool at all. An empty `allowed_tools` is the legacy unrestricted state,
not deny-all. When `allowed_tools` is non-empty, MCP tools whose name contains
`__` are auto-admitted; only `excluded_tools` removes them. Autonomy
`readonly` removes every side-effecting tool.

Layer 2, approval. `gate_tool_approval`
(`crates/zeroclaw-runtime/src/agent/turn/approval_gate.rs`) asks the
`ApprovalManager` whether the tool needs a prompt, then sends a
`ChannelApprovalRequest` to `ctx.channel` with `recipient =
ctx.channel_reply_target`, that is, the chat the message came from. The
channel path constructs the manager with `ApprovalManager::for_non_interactive`
(orchestrator, around line 16330). In that mode
`approval_requirement("shell")` returns `NotRequired` unless `shell` (or
`"*"`) is in `always_ask` (`crates/zeroclaw-runtime/src/approval/mod.rs`,
the `non_interactive && tool_name == "shell" &&
!non_interactive_shell_requires_approval` branch). The owner's unprompted
shell depends on that `non_interactive_shell_requires_approval` flag
staying false for the channel constructor; check it on every upstream port. The shell tool then
enforces its own "approval" through the model-supplied `approved: true`
argument (`crates/zeroclaw-runtime/src/tools/shell.rs`,
`validate_command_execution_for_shell` in `crates/zeroclaw-config/src/policy.rs`).
The model can set that flag itself. Under a default `supervised` profile
reached from Telegram, `shell` therefore runs with no human in the loop,
bounded only by `allowed_commands`, `block_high_risk_commands`,
`forbidden_paths`, and the sandbox. Other tools (for example `file_write` or
any MCP tool not in `auto_approve`) do prompt.

Layer 3, who may answer. `TelegramChannel::handle_approval_callback`
(`crates/zeroclaw-channels/src/telegram.rs`) accepts a button press when the
presser is any allowed peer on that alias and the callback comes from the
same chat the prompt was posted in (`resolve_pending_approval_with_tool` in
`crates/zeroclaw-channels/src/util.rs`). The prompt is not bound to the
member who triggered the tool call. In the household group either human can
approve either human's request. An "Always" answer adds the tool to the
session allowlist, which the whole shared group session then inherits.

Cross-channel approval routing. `risk_profiles.<alias>.approval_route`
sends the prompt to a different registered channel key instead of the
originator (`resolve_routed_approval` in
`crates/zeroclaw-runtime/src/agent/agent.rs`). It keeps the same `recipient`
string. With three Telegram bots, routing the household agent's approvals to
`telegram.owner` would make the owner bot post into the household group's
chat ID, which the owner bot may not be a member of, and there is no field
to name a different recipient. So "ask the owner in his DM before the
household agent does something destructive" is not expressible with stock
config today.

Implications recorded for the open decisions:

- The partner fence must be capability shaped: `readonly`, or `supervised`
  with an explicit `allowed_tools` list that omits `shell`, `file_write`,
  `browser`, and `http_request`, plus `excluded_tools` for destructive MCP
  siblings. Relying on approvals alone leaves `shell` open. See
  [Partner capability profile](06-partner-capability-profile.md).
- If the owner wants a human prompt before shell runs from Telegram, put
  `shell` in the owner profile's `always_ask`. See
  [Owner approval posture](07-owner-approval-posture.md).
- Group approvals are household approvals. Either member can say yes, and
  "Always" sticks for the group. See
  [Group approval semantics](10-group-approval-semantics.md).

## Artifacts

None.
