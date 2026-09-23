# Group approval semantics

Type: grilling
Status: resolved
Blocked by: [Bot identity topology](05-bot-identity-topology.md)

## Question

In the household group, who may approve a tool call, should "Always" grants
persist for the whole group, and does the owner want a veto path that is not
expressible today?

## Answer

Either human may approve or deny an action for the household agent in the
shared group. When a sensitive household tool is added later, require a new
approval on every call using `always_ask`; a prior "Always" answer must not
bypass it. Owner-only approval is not needed for V1. The only prompted
household tool in V1 is `memory_forget`, which either member may approve.
There are no household action tools in V1, so this is a rule for evaluating
future additions, not a reason to build approval routing now.

What the evidence fixes ([Tool gating on the Telegram path](04-tool-gating-on-telegram-path.md)):

- Any allowed peer in the group chat can press approve or deny on any
  pending prompt in that chat. There is no requester binding.
- "Always" writes to the shared session's allowlist, so it holds for both
  humans until the group session resets.
- `approval_route` cannot send the household agent's prompts to the owner's
  DM, because the recipient stays the originating chat ID. Making the owner
  the sole approver for household actions is a fork change (a recipient
  field on `ApprovalRoute` or a per-approval recipient override), not
  config.
- `per_user_session = false` is required for one shared group session and it
  also shares `/new` and session-level `/model`.

The group session still shares `/new` and session-level `/model`. Do not add a
tool whose effects either household member should not be able to authorize.

## Artifacts

None.
