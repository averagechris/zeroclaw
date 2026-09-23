# Bot identity topology

Type: grilling
Status: resolved
Blocked by: none

## Question

Do we accept three Telegram bot identities (one per agent, stock ZeroClaw),
or is one bot identity worth a fork change that routes senders and chats to
different agents inside one alias?

## Answer

The selected V1 preference is one Telegram bot identity, but this is not an
absolute requirement. Route the approved owner DM, partner DM, and shared
group chat to distinct agent profiles, each with separate tool, MCP, and
credential grants. Keep three separate bot identities as a fallback only if
research shows that one-bot routing is unsafe or disproportionate.

The rationale is to preserve one-bot usability while keeping the three
contexts' capabilities and credentials distinct. Implementation is not yet
authorized; discovery should establish whether the required routing can be
made safe and proportionate before any fork change.

What the evidence fixes:

- Stock ZeroClaw binds one alias to one agent
  ([Inbound routing model](01-inbound-routing-model.md)). One agent means one
  risk profile and one MCP grant for everyone who talks to that bot, and one
  long-term memory shared by owner and partner
  ([Session and memory scoping](02-session-and-memory-scoping.md)).
- Therefore "one bot" plus "partner cannot reach shell" plus "partner's turns
  cannot touch the owner's mailbox" is only reachable with a code change in
  this fork.

Options considered (detail in the artifact):

1. Three bots, three agents, stock config. Owner and partner each DM their
   own bot; the household bot sits in the group. Per-principal risk profile,
   MCP grant, workspace, and memory come for free. Costs: three BotFather
   registrations, the owner talks to two different bots depending on
   context, and the household bot must not be added to other groups where
   the humans speak unless that is intended.
2. One bot, fork change. Add a sender-and-chat to agent map at the Telegram
   channel or router boundary. Likely shape: a config field on
   `[channels.telegram.<alias>]` or a new peer-group field that names the
   owning agent for a sender in a DM and for a group chat ID. Costs: a
   security-relevant patch the fork must carry across upstream syncs, new
   tests at the routing boundary, and a design that upstream may or may not
   want (#8290 leans toward principals and grants rather than routing).
3. One bot, one agent, accept shared capability. Rejected by the owner's
   constraints; listed only so it is visibly rejected.

The V1 preference is option 2, subject to research confirming that its
sender-and-chat routing can safely enforce the distinct agent profiles and
grants. Option 1 remains the fallback if that research finds one-bot routing
unsafe or disproportionate. Option 3 remains rejected because it would share
capabilities across principals.

## Artifacts

- Proposal: [Topology options](../artifacts/topology-options.md) lays the
  options side by side with what each one gives up.
