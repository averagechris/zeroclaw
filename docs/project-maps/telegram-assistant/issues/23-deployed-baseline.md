# Deployed baseline

Type: task
Status: resolved
Blocked by: none

## Question

Which deployed capabilities supersede the original text-only launch constraints
and form the starting point for further tools?

## Answer

The text-only launch is complete. One bot now admits private guests by a
single-use invitation or a 24-hour handle approval. Each enrolled DM and group
gets its own agent workspace and memo store. The operator confirmed invitation
enrollment and isolated private memory through Telegram after deployment.

Memo replaces native durable memory. The Nix configuration sets a 256-line wake
and preserves the complete tool result. Michi saves useful durable details
proactively and keeps routine memory work out of replies. The memo workflow and
sharing exclusion remain owned by [Memo trial](22-memo-trial.md).

Approved chats can send photos, voice notes, short videos, location pins, and
venues. They can use Kagi search and single-page extraction, plus image generation
and editing. Video frames remain available as image references. Only the owner
DM has shell and FFmpeg for editing and returning the original video.

Location input supplies a shared point and any venue metadata. Reverse geocoding
and native outgoing Telegram pins are not implemented. Kagi accepts an explicit
lens, but no named lenses have been selected. Remote Codex work and desktop
control also remain follow-up work.

The default model remains Luna with medium reasoning; session choices include
Sol and Astra. Current credentials and host wiring belong in the
[host runbook](https://github.com/averagechris/dotfiles/blob/main/docs/zeroclaw.md),
not in this map.

## Artifacts

- Verified source revision: `386873f0068df5dc70a5684cb60f5e3855df541e`, now included
  in fork main. All source CI passed at that revision.
- Verified host revision: `57f8474b6b19178087e7b9fc4f7b26340d5165e7`. All host CI
  passed; its evaluated system matched the running deployment.
- Operator confirmation: invitation enrollment and isolated private memory worked
  in Telegram. No personal chat contents or test identities are recorded here.

## Delivery links

- [Memo tool](https://github.com/averagechris/zeroclaw/pull/23)
- [256-line memo wake](https://github.com/averagechris/zeroclaw/pull/24)
- [Location input](https://github.com/averagechris/zeroclaw/pull/25)
- [Handle invitations and general location wording](https://github.com/averagechris/zeroclaw/pull/26)
- [Reproducible host configuration](https://github.com/averagechris/dotfiles/pull/39)
