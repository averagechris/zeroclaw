# Upstream alignment

Type: research
Status: resolved
Blocked by: [Bot identity topology](05-bot-identity-topology.md)

## Question

If the plan needs a fork change (sender-to-agent routing, or an approval
recipient override), does upstream already have a design or issue we should
align with, and is the change worth proposing upstream instead of carrying
locally?

## Answer

Do not wait for upstream #8290 to supply one-bot, three-agent routing. It
targets principal-owned sessions and per-sender authorization, while #5982
narrows a sender's risk profile **inside one agent**, not the agent selected
for a chat. The accepted [multi-agent RFC
#5890](https://github.com/zeroclaw-labs/zeroclaw/issues/5890) defers
shared-bot routing as a later slice. A small fork-specific route is the V1
candidate, provided [One-bot route and isolation
proof](18-one-bot-route-and-isolation-proof.md) can be met. Consider a focused
upstream routing proposal only after the local contract is clear; this plan
does not authorize posting one.

Do not invent a local owner-only approval-recipient field. V1 allows either
household member to approve, and upstream draft [#11068](https://github.com/zeroclaw-labs/zeroclaw/pull/11068)
and open [#10241](https://github.com/zeroclaw-labs/zeroclaw/pull/10241)
already explore recipient binding with different field names. Follow that
work if owner-only approval is ever requested.

The fork carry cost is meaningful: upstream changed `telegram.rs` 18 times
and `orchestrator/mod.rs` 51 times since 2026-09-01 as observed on
2026-09-27. Prefer an isolated route boundary and tests, and review every
upstream port deliberately; do not merge master blindly.

Known upstream state (read-only). The ADR and tracker status below was
checked on 2026-09-22; the carry-cost counts above were taken on
2026-09-27. Recheck both before starting the route patch.

- [ADR-017](../../../book/src/architecture/decisions/ADR-017-inbound-authentication-and-principals.md)
  is `proposed`. It defines canonical principals and permission profiles for
  RPC, WSS, and gateway callers. Channel identity is named as a compatible
  later extension.
- Tracker [#8290](https://github.com/zeroclaw-labs/zeroclaw/issues/8290)
  (open, updated 2026-09-17) lists per-sender authorization and per-sender
  RBAC ([#5982](https://github.com/zeroclaw-labs/zeroclaw/issues/5982)) as in
  scope, with principal-owned sessions and private memory. It frames the work
  as principals and grants, not as routing a sender to a different agent.
- Completed groundwork there is `/model --agent` per-sender authorization
  (`admin_for_agent_scope`), which is the pattern a per-sender routing or
  approval field would most resemble in config.

Further evidence: [#5982 scope refresh](https://github.com/zeroclaw-labs/zeroclaw/issues/5982#issuecomment-4664899012)
and [#11068](https://github.com/zeroclaw-labs/zeroclaw/pull/11068) use
peer-group risk profiles rather than routing to another agent. Both are
useful direction for sender policy but do not solve the household's three
agent-ownership boundaries.

## Artifacts

None.
