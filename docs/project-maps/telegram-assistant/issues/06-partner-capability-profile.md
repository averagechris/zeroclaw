# Partner capability profile

Type: grilling
Status: resolved
Blocked by: [Bot identity topology](05-bot-identity-topology.md)

## Question

Which capabilities should be available to the partner's private agent?

## Answer

The partner's private agent may use all tools exposed by MCP integrations
that she later connects with using her own OAuth/PKCE account. V1 has no
account integrations; its tools are the built-in memory tools and shared
Kagi search. It must not receive shell, host-execution, or system-control
tools, and it must never use the owner's personal integration credentials.
This is not a strict hostile-tenant isolation requirement.

For later standard MCP tools, use the normal tool behavior; see
[Partner action confirmation](12-partner-action-confirmation.md).

Constraints from [Tool gating on the Telegram path](04-tool-gating-on-telegram-path.md):

- Approvals do not fence `shell` on the channel path. The partner profile
  must exclude `shell` outright, either through `level = "readonly"` or a
  non-empty `allowed_tools` that omits it. `excluded_tools` should also name
  it so a later profile edit cannot readmit it by accident.
- `allowed_tools = []` means unrestricted. The partner profile must not rely
  on an empty list.
- If the profile pins `allowed_tools`, MCP tools named `<server>__<tool>`
  are auto-admitted for her separately authorized MCP server entries. Do not
  grant her the owner's or household agent's personal server aliases. Exact
  `excluded_tools` entries are only needed if a particular MCP action is later
  deliberately withheld; see [Partner action confirmation](12-partner-action-confirmation.md)
  for confirmation policy.
- Prompts land in the partner's DM and only the partner can answer them
  there, which is the correct approver for her own account.

## Artifacts

None.
