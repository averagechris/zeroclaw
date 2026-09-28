# Session and memory scoping

Type: research
Status: resolved
Blocked by: none

## Question

With the sessions ZeroClaw already builds, do the owner DM, partner DM, and
household group get the separate conversation contexts the owner asked for,
and what leaks across them?

## Answer

Conversation history is already split the way the owner wants. Long-term
memory is not split by sender; it is split by agent.

History key. `conversation_history_key_in_scope`
(`crates/zeroclaw-channels/src/orchestrator/mod.rs`) builds the key from the
channel scope (`telegram.<alias>`), the `reply_target` (chat ID, plus forum
topic when present), and the sender when the scope is `Sender`.
`TelegramChannel::conversation_scope_for` (`crates/zeroclaw-channels/src/telegram.rs`)
returns `Sender` for every DM and, by default, for groups too. With
`channels.telegram.<alias>.per_user_session = false` it returns `ReplyTarget`
for group and supergroup chats, which drops the sender from the key and gives
the whole group one session.

What that means for the three principals:

- Owner DM and owner-in-group are different sessions because the chat ID
  differs. The same holds for the partner.
- With `per_user_session = false` on the group's alias, the group shares one
  session. The Telegram doc records the shared controls: any member's `/new`
  resets the group conversation, a session-level `/model` applies to all,
  `/stop` stays personal. Debounce and interruption keys keep the sender
  (`message_debounce_key`, `interruption_scope_key`) so one member cannot
  cancel another's in-flight turn.
- If the group and a DM live on the same alias (one-bot layout), the
  `per_user_session` flag is per alias. Setting it to `false` for the group
  does not affect DMs, which are always sender scoped.

Long-term memory. Each agent owns one `Arc<dyn Memory>`. SQLite and similar
backends stamp `agent_id` and filter recall per agent
(`AgentScopedMemory`, described in `docs/book/src/agents/internals.md`).
There is no per-sender dimension. In a one-bot, one-agent layout, anything
the partner tells the bot can surface in the owner's recall and the reverse.
In a three-agent layout, memory is isolated by construction, and any sharing
is an explicit `read_memory_from` grant. ADR-017 describes principal-owned
private memory as a future storage property; it has not landed.

Approval "Always" grants live on the `ApprovalManager` session allowlist
(`record_decision` in `crates/zeroclaw-runtime/src/approval/mod.rs`). In a
shared group session that allowlist is shared, so one member's "Always" on a
tool applies to the next member's request for the same tool. Details in
[Tool gating on the Telegram path](04-tool-gating-on-telegram-path.md).

## Artifacts

None.
