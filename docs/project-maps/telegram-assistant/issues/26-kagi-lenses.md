# Kagi lenses

Type: implementation
Status: open
Blocked by: none

## Outcome

Give every admitted Michi chat useful Kagi lenses and guidance for choosing them.
The owner approved automatic selection when helpful, with ordinary search as
its default, and requested a blind Luna evaluation using Michi's actual context.

The initial custom collections are Montevideo after hours, Buenos Aires en
escena, and Westside life for Santa Monica and Los Angeles. They cover movies,
events, cultural plans, and community activities. Programming and Forums remain
available through built-in lens identifiers.

Businesses and services use broad search. Florists, cafés, gelaterias, and
plumbers need business pages, directories, and recent local evidence beyond
an events source list. Use local Spanish terms for Uruguay and Argentina,
include the actual city or neighborhood, and check practical needs such as
flower delivery and urgent service. General interest in a place does not
establish the person's current location.

## Implementation

The canonical collections and selection guidance are in the host's
[shared search guide](https://github.com/averagechris/dotfiles/blob/main/flakes/hosts/thorny/michi/SEARCH.md).
It is included in the public identity inherited by owner, invited chats, and
groups. Private location context and owner tools remain scoped to their chat.

The runtime adds the official Kagi inline `lens.sites_included` request field,
limited to 1 through 10 domains. It rejects malformed definitions, combining
`lens` with `lens_id`, and lens arguments for extraction or non-Kagi providers.
No runtime catalog or new configuration state is needed. Existing account
lenses continue to work through `lens_id`.

Account lenses were not created. The existing rdny broker cannot be reached
from this workspace because its recorded process identity is stale or
mismatched. It was not restarted or replaced. Inline lenses remove the account
setup dependency and let the source lists be reviewed and changed with the host.
The [Kagi API reference](https://kagi.com/api/docs) documents this request shape.

## Validation and remaining work

- Validate request forwarding and rejected arguments through the tool boundary.
- Confirmed the evaluated host identity includes the complete guide for all chats.
- Completed the [blind Luna evaluation](../artifacts/kagi-search-evaluation.md)
  with the rendered shared identity and source-equivalent tool definition.
  It chose the intended lenses, broad searches, and verification steps.
- Sol high reviewed runtime, locale propagation, host guidance, and permissions.
  No correctness findings remain. The host source and both locks must pin the
  reviewed runtime revision.
- After review and rollout, compare representative live Kagi searches and check
  Telegram recommendations against original event and business pages. Source
  existence and a model evaluation do not prove useful Kagi results.

Scheduling and Codex follow-up live QA remain deferred until tomorrow.

## Discovery sources

[Kagi lenses](https://help.kagi.com/kagi/features/lenses.html) explains filtering
and built-ins. [OpenKagi](https://openkagi.com/lenses?sort=recent) provides community
ideas such as local-news, original-documentation, and firsthand-review
collections. These inspired the small purpose-specific set; their claims and
source lists were not adopted as verified recommendations. Official paperwork
and broader California or Uruguay searches use ordinary search with original
sources. Small Web and archive collections remain optional future experiments.
