# Partner action confirmation

Type: grilling
Status: resolved
Blocked by: none

## Question

Should MCP operations on the partner's own account that send, delete, edit, or
otherwise cause external side effects execute immediately, or require
per-action confirmation?

All MCP tools may be available either way; tool availability and approval
requirements are distinct decisions.

## Answer

For company or third-party MCP integrations, use the integration's normal
intended/default tool behavior together with ZeroClaw's ordinary policy. Do not
add custom harness or setup confirmation behavior for standard MCP tools. If a
future tool is custom-built by us, ask the user during that tool's plan
refinement to decide its action and approval behavior.

Connector credentials remain account-scoped; this decision does not grant
access to another person's account.
