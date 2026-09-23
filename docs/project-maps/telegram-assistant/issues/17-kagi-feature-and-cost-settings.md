# Kagi feature and cost settings

Type: grilling
Status: resolved
Blocked by: [Kagi search tool](14-kagi-search-tool.md)

## Question

Once Kagi is implemented as a provider backend in the native
`web_search_tool`, which Kagi features and cost controls should be available
to the approved agent profiles initially?

- Which named Kagi lenses or workflows, if any, should be available on day
  one? Should the initial surface stay ordinary `search`, or expose options
  such as news, images, videos, or podcasts?
- What result cap and safe-search default should the shared tool use?
- Which Kagi API usage limit should be configured as the spending guard?

## Answer

Expose ordinary search only, with safe search on, no lens, no other workflow,
and no Extract. Use the existing `web_search_tool` result cap rather than
introducing a Kagi-specific cap. The current `[web_search] max_results`
default is 5 (`WebSearchConfig` in `crates/zeroclaw-config/src/schema.rs`).
The owner's Kagi account already has its API usage limit configured; do not
choose or change that account limit as part of this plan. Search calls still
need ZeroClaw's existing rate limiting.

Add lenses or other workflows only on a later, concrete request.

### Failure and fallback

Kagi is the primary and configured provider. DuckDuckGo is an allowed
fallback, used only after the person in the chat agrees. This is a
conversational check, not a formal approval prompt:

- Configuration errors fail loudly. The stock resolver silently maps an
  unknown `search_provider` to DuckDuckGo
  (`resolve_unknown_provider_falls_back_to_default` in
  `crates/zeroclaw-tools/src/web_search_provider_routing.rs`). With Kagi
  configured, a misspelled provider or a missing Kagi key must be a config
  validation error at startup, not a silent switch.
- A Kagi call that fails at runtime (network, 5xx, 429, account usage limit)
  returns a tool error saying Kagi failed and that the agent may offer
  DuckDuckGo. The agent then asks something like "Kagi isn't working right
  now; is it okay if I use DuckDuckGo for this?"
- The tool gets one optional parameter that selects the fallback. Its
  concise description states the owner's intention: use it only after Kagi
  failed and the person agreed in this conversation. Without it, the tool
  never calls DuckDuckGo.
- Enforcement is the model following the tool description. The owner
  accepts that; it is a preference about search quality, not a security
  boundary. In the group either member may say yes.

The fallback's config field and parameter names are proposed fork additions,
not current schema.

## Artifacts

- Evidence and labeled recommendation: [Kagi search options](../artifacts/kagi-search-options.md).
