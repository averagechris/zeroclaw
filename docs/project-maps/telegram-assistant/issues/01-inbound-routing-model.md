# Inbound routing model

Type: research
Status: resolved
Blocked by: none

## Question

Given one Telegram bot token, can ZeroClaw route the owner's DM, the partner's
DM, and the shared group to three different agents (and therefore three risk
profiles and three MCP grants), or does routing stop at the channel alias?

## Answer

Routing stops at the channel alias. A bot token lives under one
`[channels.telegram.<alias>]` block, and `AgentRouter::resolve`
(`crates/zeroclaw-channels/src/orchestrator/mod.rs`) looks up
`"<type>.<alias>"` in `owner_by_channel_key`, which
`build_owner_by_channel_key` fills from each enabled agent's `channels` list.
`Config::agent_for_channel` returns the first enabled agent that lists the
key. The sender, chat type, and chat ID are not inputs to that lookup.

Consequences for this effort:

- Peer groups (`[peer_groups.<name>]`) decide who may talk to an alias, not
  which agent answers. `external_peers` match sender identity (numeric
  Telegram ID or username), never a chat ID, so a group is not itself an
  authorization subject. Any group the bot is added to answers the owner and
  partner the same way, under the same agent.
- The only per-sender privilege that exists is `admin_for_agent_scope`, which
  gates `/model --agent`. There is no per-sender tool list.
- Per-channel `excluded_tools` is per alias, so it can narrow a whole bot, not
  one sender on that bot.
- Two aliases cannot share one token. Both would long-poll `getUpdates` and
  Telegram returns 409 for the second poller (documented in
  `docs/book/src/channels/telegram.md`).

So "one bot, three agents" is not expressible in stock ZeroClaw. The stock
expression of three agents is three bot tokens, three aliases, three agents.
Anything else is a fork change to the channel or router. Upstream tracker
[#8290](https://github.com/zeroclaw-labs/zeroclaw/issues/8290) and the
"per-sender RBAC" issue [#5982](https://github.com/zeroclaw-labs/zeroclaw/issues/5982)
point in the direction of per-sender authorization, but both are open and
scoped to principals and grants, not to sender-to-agent routing.

This resolution feeds [Bot identity topology](05-bot-identity-topology.md).

## Artifacts

- Proposal: [Topology options](../artifacts/topology-options.md) compares
  the stock three-bot layout against a one-bot fork change.
