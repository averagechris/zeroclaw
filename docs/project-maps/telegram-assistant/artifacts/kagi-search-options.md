# Kagi search options

Helps answer [Kagi search tool](../issues/14-kagi-search-tool.md) in the
[Telegram assistant map](../map.md): what is the smallest useful Kagi search
surface, and which Kagi features matter for it. This is evidence plus a
labeled recommendation. The decision's `Answer` stays authoritative; the
owner chose the plain-search starting point.

Facts below were checked against Kagi's published sources on 2026-09-27.
Anything not marked "verified" is an open question.

## Verified API facts

Sources: [Kagi API docs](https://kagi.com/api/docs),
[POST /search reference](https://redocly-api-docs.kagi.com/api/docs/openapi/search/search.md),
[OpenAPI spec](https://redocly-api-docs.kagi.com/api/docs/_bundle/openapi.yaml),
[Quick Start](https://help.kagi.com/kagi/api/quick-start.html),
[API pricing](https://kagi.com/api/pricing).

| Item | Verified value |
|---|---|
| Base URL | `https://kagi.com/api/v1` |
| Endpoint | `POST /search`, JSON body |
| Auth | HTTP bearer: `Authorization: Bearer <api key>`; keys come from <https://kagi.com/api/keys> |
| Required field | `query` (string) |
| Result type | `workflow`: `search` (default), `images`, `videos`, `news`, `podcasts` |
| Result cap | `limit` 1 to 1024. It caps what is returned, not what Kagi fetches or bills |
| Paging | `page` 1 to 10 |
| Safety | `safe_search` (boolean) |
| Time and region | `filters.after`, `filters.before`, `filters.region` (ISO 3166-1 alpha-2). Filters override overlapping lens fields |
| Lens by ID | `lens_id`: a built-in lens identifier, or a shareable lens ID or URL from <https://kagi.com/settings/lenses> |
| Inline lens | `lens` object: `sites_included`, `sites_excluded`, `keywords_included`, `keywords_excluded`, `file_type`, `time_after`, `time_before`, `time_relative` (`day`, `week`, `month`), `search_region` |
| Page extraction | `extract.count` 1 to 10 fetches page markdown into `snippet`. Billed separately at the Extract rate |
| Response | `data.search[]` with `url`, `title`, `snippet`, `time`, optional `image`; other typed arrays such as `data.news`, `data.direct_answer`, `data.related_search`; `meta.trace` for support |
| Errors | 400 with `error[].code`, `error[].message`, `error[].location` |
| Search price | $12 per 1,000 search requests, pay as you go |
| Extract price | $4 per 1,000 pages |
| Billing cadence | Invoiced every 30 days or when usage reaches $100 or the configured custom usage limit, whichever comes first. Kagi documents a configurable "Set Usage Limit"; the docs do not establish exact request rejection/overage semantics at the boundary |
| Account effects | The API inherits account settings such as blocked or promoted sites and snippet length ([Search API help page](https://help.kagi.com/kagi/api/search.html)) |
| Official clients | Generated OpenAPI clients for Rust, Python, Go, TypeScript ([API docs](https://kagi.com/api/docs)) |
| Kagi MCP server | Hosted at `https://mcp.kagi.com/mcp` with bearer auth, no OAuth yet. Source at [kagisearch/kagimcp](https://github.com/kagisearch/kagimcp/tree/rehan/v1-api). Tools: `kagi_search_fetch`, `kagi_extract`. Stdio install via `uvx kagimcp` with `KAGI_API_KEY`; `KAGI_HIDDEN_PARAMS` hides chosen search params from the model-facing schema |

One inconsistency worth knowing: the help page example still shows
`Authorization: Bot $TOKEN` with `GET ...?q=`. The OpenAPI reference and
Quick Start use `POST /search` with `Bearer`. Treat the OpenAPI reference as
authoritative and expect the help page to catch up.

## Cost model that matters for a household bot

Every `POST /search` is one request at $12 per 1,000, regardless of `limit`.
`extract.count` and the separate `kagi_extract` tool add Extract charges. A
model that reflexively searches three times per question at roughly 30
questions a day costs about a dollar a day. The two levers that bound spend
are the ZeroClaw side rate limiter on `web_search_tool` and keeping extract
off the first surface.

## Minimal initial wrapper (suggestion, not a decision)

The smallest useful surface is one call shape:

- `workflow = "search"` only. No images, videos, news, or podcasts.
- `limit` set from ZeroClaw's existing `[web_search] max_results` (1 to 10).
- `safe_search` on by default because the household group is a shared
  session.
- No `extract`. Full-page fetch already exists as `web_fetch` and would
  double bill.
- Owner's deployment-level API key, stored as an encrypted secret field the
  same way `brave_api_key` and `tavily_api_key` are today
  (`WebSearchConfig` in `crates/zeroclaw-config/src/schema.rs`).
- Return `title`, `url`, `snippet`, and `time` from `data.search`, in the
  same text shape the other providers produce, so the model sees no
  provider-specific format.

Lenses are the one feature with real leverage for a personal assistant, and
they cost nothing extra per request. Two ways to use them:

- Inline `lens` on the request. Fully under our control, no account state,
  and can be named in config (for example a `recipes` lens as a
  `sites_included` list). Good fit for "household lenses" that both humans
  share.
- `lens_id` pointing at a lens created in the owner's Kagi account. Less
  config, but the lens then lives in the owner's account and the API call
  runs under the owner's personalization anyway.

## Questions raised during research

The owner resolved the initial surface in [Kagi feature and cost
settings](../issues/17-kagi-feature-and-cost-settings.md) and the shared key
in [Kagi integration path](../issues/14-kagi-search-tool.md). The questions
below explain what the research compared; they are not a second decision
record.

1. Is one shared owner API key acceptable for all three principals, or does
   the partner's usage need a separate key for cost attribution? Kagi bills
   per key holder; there is no per-user sub-key documented.
2. Which lenses matter on day one, if any? A first slice can ship with none
   and add inline named lenses later without changing the tool contract.
3. Should the tool expose `workflow` (news, images) to the model at all in
   v1? Each extra parameter is a way for the model to spend money in a way
   the owner did not ask for.
4. What custom usage limit should be configured, and is the boundary behavior
   sufficient as the spend guard beyond the rate limiter?

## Open questions to research

- Whether Kagi enforces a request rate limit per key, and what it is. Not
  found in the public reference during this check.
- Whether `lens_id` for a built-in lens (for example Forums or Programming)
  accepts the display name or needs an identifier obtained elsewhere. The
  reference says "built-in lens's identifier" without listing them.
- Whether `data.search` is stable enough to parse without the typed extras,
  or whether `direct_answer` is worth surfacing to the model.

## Recommendation (labeled)

Start with ordinary search only, low result cap, owner key, no extract, no
lens. Add inline named lenses as the first enhancement once the tool is in
daily use. Keep `kagi_extract` and `extract.count` off the surface until the
owner asks for them, because they are the only path to surprise billing.

Whether this ships as a new provider inside `web_search_tool` or through
Kagi's MCP server is the question in
[Tool extension DX](../issues/15-tool-extension-dx.md), compared in
[Tool extension options](tool-extension-options.md).
