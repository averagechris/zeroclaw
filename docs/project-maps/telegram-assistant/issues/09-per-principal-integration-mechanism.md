# Per-principal integration mechanism

Type: research
Status: open
Blocked by: [Household identity boundary](08-household-identity-and-integrations.md), [Household integration set](13-household-integration-set.md)

## Question

Which concrete MCP server (or other mechanism) gives each principal their
own Gmail and calendar credentials on the owner's host, how does each person
complete consent for their own account, and where does each token file end
up?

## Answer

Deferred beyond V1. No personal or household account integration was selected
for this release. When a specific service is requested, narrow this question
to that service before researching an OAuth server or token storage. Do not
prototype Google consent solely because this earlier example named Gmail and
Calendar.

Fixed inputs ([Credential and OAuth ownership](03-credential-and-oauth-ownership.md)):

- The built-in `google_workspace` tool is out because it is one global
  account. The mechanism must be one `[[mcp.servers]]` entry per principal
  with per-server `env` or `headers`, granted through one `[mcp_bundles.*]`
  per agent.
- ZeroClaw has no MCP OAuth client. Consent must be completed by the MCP
  server's own flow or by an external CLI before ZeroClaw connects.

What to find out, in this order:

1. Candidate servers: pick one or two Google Workspace MCP servers (Gmail
   plus Calendar) and record, per server, whether the token location is
   configurable through an environment variable, whether consent can be
   completed from a printed URL without a local browser, and whether the
   token refresh happens inside the server process without operator action.
   Prefer a server whose destructive tools have distinct names so
   `excluded_tools` can target them.
2. Consent procedure for the partner: can she complete consent from her own
   phone or laptop, or does she need to sit at the owner's desktop? A
   loopback redirect plus a temporary port forward is the usual workaround;
   record whether the chosen server supports it.
3. Disk layout: one token directory per principal under the ZeroClaw config
   root or under each agent's workspace, with file permissions that the
   partner and household agents' sandboxes cannot read. Record the exact
   paths so the risk profiles' `forbidden_paths` can name them.
4. Startup and restart: MCP bundle changes and `env` changes take effect on
   session restart. Record which restarts are needed after consent.

Exit criteria: a written procedure the owner can follow for one principal
end to end, and a list of the destructive prefixed tool names for the
chosen server.

Prototype only after the owner selects a service and approves using a
throwaway account.

## Artifacts

None yet. A prototype note may be added here once one exists.
