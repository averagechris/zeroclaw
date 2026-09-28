# Kagi integration path

Type: grilling
Status: resolved
Blocked by: none

## Question

Which ZeroClaw integration path should carry Kagi as the first shared tool:
the native provider seam inside `web_search_tool`, or Kagi's separate MCP
server?

## Answer

Use Kagi as a provider backend inside ZeroClaw's existing `web_search_tool`,
with the owner's deployment-level Kagi API key shared across the approved
agent profiles. Keep the memory-tools-only access/routing baseline first, then add Kagi
immediately as the first shared tool.

The native path fits the existing provider-routing seam, encrypted provider
key configuration, `RateLimitedTool` spend guard, and default approval behavior
for `web_search_tool`. It also preserves the existing shared search-tool
surface and follows the established provider implementation and upstream-PR
shape. Kagi's MCP server would provide a separate Extract tool and its rate
limiting was not verified, while its additional tool and approval/exclusion
configuration would create a larger shared-tool surface.

The first call surface is ordinary search, with safe search on, the existing
tool result cap, and no lens or Extract. The owner already manages the Kagi
account's usage limit. A Kagi failure lets the agent ask before falling back
to DuckDuckGo, and misconfiguration fails at startup; see [Kagi feature and
cost settings](17-kagi-feature-and-cost-settings.md).

## Artifacts

- Evidence: [Kagi search options](../artifacts/kagi-search-options.md) records
  the verified API contract, pricing, lens forms, and conservative initial
  wrapper recommendation.
- Comparison: [Tool extension options](../artifacts/tool-extension-options.md)
  compares the native provider seam with Kagi's MCP server, including routing,
  encrypted key configuration, rate limiting, and approval behavior.
