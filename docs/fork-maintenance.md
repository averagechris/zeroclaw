# Fork maintenance

This repository is maintained at <https://github.com/averagechris/zeroclaw> as
a fork of <https://github.com/zeroclaw-labs/zeroclaw>.

## Remotes and bookmarks

- `origin` is the maintained GitHub fork. Local `main` tracks `main@origin`.
- `upstream` is the original GitHub repository. `master@upstream` is
  fetch-only and must not be tracked by the local bookmark.
- Upstream uses `master` as its default branch; there is no upstream `main`
  bookmark.
- Fork-only CI and Dependabot branch references use `main`. Upstream-specific
  links, release tooling, and the documentation site's `master` version name
  are not general instructions to rename upstream's branch.

Refresh the upstream remote with `nix run .#fetch-upstream` or
`jj git fetch --remote upstream`. Review the resulting commits and port wanted
changes deliberately. Do not blind-merge upstream into the maintained fork.

## Review watermark

The fork was created from upstream commit `1f12418c` ("docs(developing):
record the replacement-first integration policy (#11042)") on 2026-09-22.
Future upstream review starts after that commit. Advance this watermark only
after reviewing or deliberately excluding every intervening upstream change.

Keep fork-specific maintenance notes here and keep upstream project guidance
in its existing documentation.
