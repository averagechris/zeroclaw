# Michi search instruction evaluation

## Method

On 2026-10-10, a Luna agent at medium reasoning received no inherited
conversation. Its only supplied assistant context was the shared Michi identity
rendered by the host's Nix configuration and a source-equivalent Kagi tool
schema and Fluent description. Each question was a separate chat. It planned
calls without making searches or inventing businesses or events.

The host identity included `IDENTITY.md`, `SOUL.md`, `SCHEDULING.md`, and
`SEARCH.md`. A separate evaluation confirmed the complete search guide in the
owner, guest, and group identities. The probe did not receive owner tools,
expected answers, or the implementation discussion.

## Observed choices

| Question | Planned behavior |
| --- | --- |
| Flower delivery tomorrow in Pocitos | Ordinary Spanish search, Sunday delivery date, business delivery-page verification. |
| Urgent plumber in Montevideo | Ordinary search using `sanitario`, urgency and service area; advertised availability distinguished from a confirmed visit. |
| Gelateria near Plaza Armenia | Ordinary Spanish `heladería` search; addresses and walking distance checked before recommendation. |
| Movies in Montevideo this weekend | Exact Montevideo inline lens, October 10–11, 2026 in query, cinema-page verification. |
| Buenos Aires play next Friday | Exact Buenos Aires inline lens, October 16, 2026 in query, performance and ticket verification. |
| Free Santa Monica community events | Exact Westside inline lens, city and weekend dates in query, price and registration checked. |
| Nice café nearby, no location given | Location question before searching; no assumed country or city. |
| Rust async borrowing documentation | `lens_id: "programming"`; asks for code and diagnostic before explaining the particular failure. |
| Movie search returns stale listings | Removes the lens and broadens Kagi search, retaining city and dates. |
| Kagi unavailable | Asks before DuckDuckGo; only then plans `use_duckduckgo_fallback: true`. |
| Plausible event snippet | Extraction from an original page; does not claim ticket availability from the snippet. |

The planned inline calls used the complete domain lists from `SEARCH.md`,
without combining `lens` and `lens_id`. Extraction calls used `action` and
`url`, without search arguments.

## Limits

This checks whether one fresh agent understands the supplied instructions.
It does not establish Kagi indexing, recommendation quality, walking routes,
live availability, tool execution, or deployed Telegram behavior. Those require
representative searches after review and rollout. The runtime's request and
argument tests cover the separate inline-lens API contract.
