# Memo trial

Type: task
Status: resolved
Blocked by: none

## Question

How should the deployment use the memo CLI while keeping each DM and group separate?

## Answer

Expose one `memo` tool with `wake`, `note`, and `nap` actions. Bind it to the
current agent workspace's `memo` directory and pass `--store default` explicitly.
The model cannot choose a store, path, or another agent. Static and dynamically
enrolled agents use the same tool preparation and permission gate.

Isolation follows agent workspaces. The owner route and dynamically enrolled
DMs and groups each have a distinct alias and workspace. A custom configuration
that reuses one alias across chats also shares its memo store; it must allocate
separate aliases to retain this boundary. Keep `workspace.read_memory_from` empty.

Enable `[memo]`, set native `[memory] backend = "none"`, and disable native
`auto_save` and `hygiene_enabled`. Grant `memo` instead of the native memory tools.
Start with empty memo stores. The old memories are disposable for this trial;
no import is required. Dormant native files can remain for rollback.

Michi calls `wake` before answering. An incomplete wake supplies source records
and a pending range. Michi writes a faithful summary with `nap` and retries wake
until complete. After a `note`, settle pending naps too. Save new, verified,
useful durable details without repeating existing notes or recording routine
task chatter. Notes and summaries are single lines of at most 280 UTF-8 bytes.

Memo owns its append-only notes and write-once summaries. The tool returns its
structured workflow, including incomplete wake results, without translating it
into native search or CRUD operations. Corrections are new notes. There is no
forget action and no cross-chat sharing in this trial.

The child process receives an empty environment, a fixed store, bounded output,
and a timeout. Freeform text follows `--`. Canonical path checks reject stores
redirected outside the workspace. Read actions use read policy; writes use act
policy. The host pins the memo package and supplies its absolute executable path.

## Artifacts

- [Setup and rollback](../setup.md#memo-trial): configuration and operator checks.
- [Memo CLI](https://github.com/averagechris/memo): storage and compaction contract.

## Delivery links

Source and host pull requests will record the verified revision and rollout.
