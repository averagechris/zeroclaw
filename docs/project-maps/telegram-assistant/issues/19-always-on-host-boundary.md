# Always-on host boundary

Type: grilling
Status: resolved
Blocked by: [Owner approval posture](07-owner-approval-posture.md)

## Question

Under which Linux user and service manager should the always-on daemon run,
and what filesystem/secret access is acceptable given that the owner's
Telegram agent may run shell without approval?

## Answer

Use the repository's existing NixOS module, `nix/module.nix`, with one
`services.zeroclaw.instances` entry. It creates a system service under a
dedicated service user, persists its state/config directory, and restarts on
failure. This is a systemd **system** service, not a systemd user service; the
owner chose it specifically so the bot's shell runs as the dedicated service
user instead of his login user. Host-specific import and instance settings
belong in the host configuration outside this repository.

Telegram uses outbound `getUpdates` long polling. It needs outbound HTTPS to
Telegram's Bot API, not an inbound port, webhook, or tunnel. Do not expose a
gateway to make this bot work.

Polling was rechecked against a webhook on 2026-09-27 and stays:

- Latency matches a webhook. ZeroClaw long-polls with `timeout: 30`
  (`crates/zeroclaw-channels/src/telegram.rs`); Telegram holds the request
  open and answers as soon as an update arrives, so an idle bot costs about
  two requests a minute and a message is picked up almost immediately.
  Perceived speed is dominated by model time.
- `getUpdates` is not the binding rate limit. Telegram's bot FAQ limits
  *sending*: about one message per second per chat and 20 per minute in a
  group. Streamed draft edits count toward that, which matters only in the
  group.
- Telegram keeps undelivered updates for 24 hours, so a desktop restart or
  brief offline period catches up on reconnect.
- A webhook would add a public HTTPS endpoint, TLS, a tunnel or port
  forward, and an exposed gateway, for no user-visible gain here.

For snappiness, turn on streaming: `stream_mode = "partial"` on the alias
(default is `off`, which waits for the whole reply). Keep
`draft_update_interval_ms` at its 1000 ms default for now. The setting is
per alias and the group's 20-per-minute cap applies to edits, so raise it
if the logs show `Too Many Requests`; ZeroClaw already retries 429s.

Disable the bot's group privacy mode in BotFather (`/setprivacy`) before
adding it to the group, or remove and re-add it afterward. With privacy mode
on, the bot sees only commands, mentions, and replies to itself. Keep
`mention_only = false` unless the household prefers to address the bot
explicitly.

### Owner shell reach

Keep the module's systemd hardening. The owner agent's shell stays inside
the unit sandbox: no `/home`, a read-only system, no privilege gain, and
writes only under `dataDir`. Grant extra reach explicitly, per path and per
tool, through the NixOS instance rather than by loosening hardening.

The module should make that one-line configuration. Today it offers
`bindReadOnlyPaths` only; read-write binds and extra shell tools need raw
`systemd.services."zeroclaw-<name>"` overrides. Add two instance options as
a small fork change to `nix/module.nix`, with eval tests in
`nix/eval-tests.nix` (names proposed):

- `bindPaths`: `target = source` read-write binds, rendered to systemd
  `BindPaths=` and added to `ReadWritePaths=`, mirroring
  `bindReadOnlyPaths`.
- `extraPackages`: packages added to the unit's `PATH` (for example `git`,
  `curl`, `jq`), so the shell has them without a hand-written `path`.

Bound paths still obey Unix permissions for the service user. Under
`PrivateUsers = true`, files owned by other users appear as `nobody`. The
service user therefore needs group or ACL access to a bound directory, for
example a shared group or `setfacl`. Document the pattern with the option.
This belongs with stage 3, before owner shell is enabled; no paths are
selected yet.

As implemented, both options exist with these names. `bindPaths` is not added
to `ReadWritePaths=`: systemd documents `BindPaths=` mounts as writable, and
a `ReadWritePaths=` entry for a target that does not exist yet would fail the
unit. The eval tests check the rendered unit; the KVM test in `nix/test.nix`
does not cover the new options yet.

Step-by-step bootstrap, including BotFather, ID collection, secrets, the
module instance, and the service-user Codex login, is in the [setup
runbook](../setup.md). The module supports an `environmentFile` for
tokens and writes a service-user-readable resolved config with mode 0600.
Provision credentials outside the Nix store, restrict the environment file,
and do not put any real token or host-specific path in this repository.
For Telegram admission, use the current `peer_groups` contract from the
Telegram channel guide; the module's older examples still mention
`allowed_users`, which the current channel schema does not accept.

Persist and back up the instance's state directory with its `.secret_key`;
restoring encrypted config without the matching key loses access to those
secrets. Arrange restart, logs, and secret rotation in host configuration.
The unrestricted owner shell can read the service user's rendered config,
token, Kagi key, and `.secret_key`. The owner explicitly accepts that for V1.
Never claim a command denylist or config file mode protects those secrets
from a tool running as the same service user. This decision does not grant
the partner or group shell, and it does not authorize host deployment yet.
