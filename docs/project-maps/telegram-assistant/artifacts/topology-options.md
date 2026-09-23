# Topology options

Helps evaluate [Bot identity topology](../issues/05-bot-identity-topology.md)
in the [Telegram assistant map](../map.md). This is a projection of the
evidence, not a decision record. The decision's `Answer` stays authoritative.

## The shape being compared

Three principals need three different capability and credential sets. In
ZeroClaw those sets attach to an agent. The question is how many Telegram
bot identities it takes to reach three agents.

```text
Option 1: three bots (stock)

  owner DM      ->  telegram.owner      ->  agents.owner      ->  risk owner,   mcp owner
  partner DM    ->  telegram.partner    ->  agents.partner    ->  risk partner, mcp partner
  household grp ->  telegram.household  ->  agents.household  ->  risk household, mcp household

Option 2: one bot (fork change)

  owner DM      -\
  partner DM    --->  telegram.home  --[new sender/chat -> agent map]--> agents.{owner,partner,household}
  household grp -/
```

## Option 1: three bots, three agents, stock config

Config only. Each `[channels.telegram.<alias>]` has its own token and its
own `[peer_groups.telegram_<alias>]`. The owner's peer group lists the owner
ID; the partner's lists hers; the household lists both. Each agent has one
risk profile and one MCP bundle.

What it gives:

- Per-principal risk profile, MCP grant, workspace, and long-term memory by
  construction. Nothing to patch, nothing to rebase.
- The household bot can set `per_user_session = false` without touching DM
  behavior on the other two.
- Each bot's approval prompts stay in the chat that owns the credentials.

What it gives up:

- The owner uses two bots: his own in DM, the household one in the group.
- Three BotFather registrations and three tokens to rotate.
- Any group the household bot is added to answers the owner or partner the
  same way; a peer group cannot pin a bot to one chat ID. Mitigation is
  operational (do not add it elsewhere), not enforced.
- Owner-only approval for household actions still is not possible; that gap
  is independent of topology.

## Option 2: one bot, fork change

A new mapping at the Telegram channel or `AgentRouter` boundary. Likely
config shape, sketched for comparison only:

```toml
[channels.telegram.home]
enabled = true
per_user_session = false        # applies to groups only

[channels.telegram.home.routes]  # proposed fork field; no default agent
dm = { "111111111" = "owner", "222222222" = "partner" }
groups = { "-1001234567890" = "household" }
```

The selected design does not use a peer-group field for routing. Peer groups
authorize inbound senders, including mutable usernames; they do not own
immutable numeric chat-to-agent binding.

What it gives:

- One bot in the owner's Telegram; one token.
- A place to hang owner-only approval later, since the router would know
  which principal a chat belongs to.

What it gives up:

- The fork carries a security-relevant route patch through channel config,
  collection, and `AgentRouter` (not just one lookup). The orchestrator file
  is large and changes often upstream, so deliberate porting will cost work.
- The three fixed chats have distinct channel-plus-chat session keys today,
  but the key does not encode agent alias. Reassigning a chat to another agent
  requires resetting or archiving its prior session. Incoming media is saved
  under the channel workspace before routing; text-only V1 must block its
  download, while the multimodal fast follow needs agent-owned storage.
- A DM from a sender not in the map has to fail closed to a deny, not to
  `default_agent`, or the partner's fence is one typo away from the owner's
  agent. Needs tests at the routing boundary and a docs update in
  `docs/book/src/channels/telegram.md`.
- Upstream's direction (#8290) is principals and grants, so a routing patch
  may be replaced rather than merged. Carry cost is open-ended.

## Option 3: one bot, one agent

Rejected by the owner's constraints. Everyone shares one risk profile, one
MCP grant, and one long-term memory. Listed so the rejection is visible.

## Smallest check that could reverse the selected option

Before enabling owner shell, prove the partner and group cannot reach the
owner context through an unknown route, a group message from the owner,
pre-route callbacks, or an old session. If that proof is not practical, use
the three stock bot identities rather than weaken the isolation rule.
