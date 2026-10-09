# Upstream review, 2026-10-09

The reviewed upstream `master` tip is
`726a86dfe3b3f6dffaede9de49c2786856736e13` in
[zeroclaw-labs/zeroclaw](https://github.com/zeroclaw-labs/zeroclaw).
The previous cutoff was `1f12418c`. The complete range contains 205 commits
touching 648 paths, recorded with full SHAs in
[the screening inventory](upstream-review-2026-10-09.tsv).

The inventory accounts for the whole range. Selected fixes received direct
source review against the fork; deferred rows are opportunities for further
subsystem review, not claims that those changes are safe or unnecessary.
This is a selective maintenance review, not a full security audit.

The range can be listed again after fetching the read-only upstream remote:

```sh
jj log --no-graph --reversed \
  -r '1f12418c..726a86dfe3b3f6dffaede9de49c2786856736e13' \
  -T 'commit_id ++ " " ++ description.first_line() ++ "\n"'
```

For each listed SHA, `jj diff --from SHA- --to SHA --summary` reproduces
its first-parent path delta. The 648-path count is the union of those deltas,
not just the final tree diff.

## Selected ports

- `48169a1a37ed56a9689397693a1efea78bd4d2b1` consumes the values of Git's
  global options before classifying the real subcommand. A command such as
  `git --attr-source log commit` must retain the approval requirement for
  `commit`. Read commands behind the same options remain classified as reads.
- `1886d1b11294e418dd51365f447573c95e909ab0` routes DuckDuckGo, Brave, and
  SearXNG request failures through the existing non-disclosing error helper.
  The same concept also covers response-body and JSON-decode failures, which
  retain query-bearing URLs in reqwest errors. The fork's existing search
  routing, query handling, and other provider adapters are preserved.
  The regression inspired by `aba511d362` exercises SearXNG through tool
  execution against failing local servers before and after response headers,
  including the complete error chain.
- `3b61e23b5a0bf1582d283c94ddf7e9a1d8514893` tolerates unsupported pricing
  shapes in compatible-provider model lists. A tiered pricing array or malformed
  optional pricing object becomes unknown pricing instead of breaking the
  entire list. Empty or unrelated pricing objects are also unknown; valid
  per-token pricing objects remain intact.

## Already present or intentionally excluded

The fork already pins Wasmtime, WASI, and WASI HTTP to 48.0.4. The upstream
48.0.x advisory fixes in `ad0dd3dec8` and `584d6da0d4` therefore do not require
a dependency downgrade or lockfile replacement. The fork also already has
host-native null-device matching; the surrounding upstream resolver changes
were not copied into its different path-policy implementation.

Upstream hosted release/CI operations, governance rewrites, and contributor
skills were excluded. Additional SOP, OIDC/principal, relay, plugin socket,
RPC subscription, and TUI features were deferred because they change runtime
or deployment contracts. Their usefulness needs a separate decision against
the fork's current operator and Telegram workflows. Independent dependency
and toolchain refreshes were deferred rather than replacing the fork's lockfile.

## Security follow-ups worth a separate port

These changes align with the fork's safety goals, but need coordinated work
across its diverged runtime and policy wiring:

- `5ba3e4da26` and `949e415d04`: capability-relative filesystem mutations and
  host-launcher resolution. The fork already rejects symlink targets and
  checks canonical parents, but those checks do not by themselves establish
  race-resistant mutation confinement.
- `df941bca5b`: SSRF protection for `file_download`, including explicit
  private-host opt-in. This changes configuration, runtime tool construction,
  channel/gateway wiring, and localized output together.
- `6108618d17` and `72324ab202`: reject unsafe SQLite entries in memory,
  response-cache, audit, and hygiene storage. Admission and ownership must
  remain consistent across the fork's existing storage entry points.
- `79987a5c45`: open dashboard assets relative to retained directory handles
  to avoid check/open races and special-file blocking.
- `7374ac7353`, `24e7324dc6`, `402cee32c3`, and `615f63aad6`: webhook
  destination pinning and RPC/gateway authorization fixes. Several depend on
  the upstream principal/auth-state rollout; cherry-picking them independently
  would not preserve the intended trust boundary.

The watermark records completion of this screening and the selected source
review. Future reviews must carry these deferred security items forward as
well as checking commits after the recorded tip.
