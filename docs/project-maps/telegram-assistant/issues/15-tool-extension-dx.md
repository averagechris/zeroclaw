# Tool extension DX

Type: grilling
Status: resolved
Blocked by: none

## Question

Which ZeroClaw tool-extension path should be the supported developer workflow
over time, considering native tools, MCP, and other existing extension
mechanisms? It must support per-agent grants, secure defaults, tests, and
documentation.

## Answer

Decide the extension mechanism for each concrete tool rather than committing
the fork to one default. Kagi already has its own native-provider decision;
no other new tool belongs in V1. For each later tool, compare the existing
first-party seam, agent-scoped MCP bundles, and, only where justified, a WASM
plugin. Spell out agent grants, secret ownership, approval behavior, tests,
and documentation before adding it. Skills are not a credential boundary.

The comparison artifact is input for those later decisions, not a blanket
endorsement of MCP or native Rust for every tool.

## Artifacts

- Proposal: [Tool extension options](../artifacts/tool-extension-options.md)
  compares the native `web_search_tool` provider seam, MCP servers, WASM
  tool plugins, and `http` skills on grant model, secret handling, approval
  shape, build cost, and fork carry, with a labeled recommendation.
- Related: [Kagi search options](../artifacts/kagi-search-options.md) holds
  the Kagi API facts the comparison relies on.
