# Conversational scheduling

Type: task
Status: claimed
Blocked by: none

## Question

What is the smallest change that lets anyone admitted to Michi request one-off or recurring work, manage it in
chat, and receive results without spending inference on routine time checks?

## Answer

The owner authorized implementation on 2026-10-10. The draft uses the existing
cron store and tools, with opt-in recovery and durable result delivery.

### Agreed behavior

- Tasks use the originating chat's current tools and accounts. Guest and group
  tasks never acquire owner permissions, even when the owner speaks in a group.
- Work runs in the background when useful. Results return to that chat.
- Michi can create, inspect, edit, pause, resume, remove, and test tasks and
  inspect their recent results.
- Missed work receives one bounded judgment: run now, wait for the next
  recurrence, skip, or ask the user. Remember the decision across restarts.
- Create checks only when requested. Confirm ambiguous times and timezones.
  Each task carries enough context to run without the original transcript.

### Implementation

The existing SQLite cron store remains the schedule authority. Opt-in
`scheduler.missed_run_policy = "review"` adds bounded missed-task judgment
with the owning chat's live tools and memory, background manual requests, and
persisted result delivery. The review uses scoped memory on demand, without
automatic memory injection or autosave. Its prompt asks the agent only to
gather context and decide. Current chat permissions govern tool access, and
scheduler mutation tools are excluded. Calls needing user approval are denied
because review has no interactive approval route. Due checks and message retries
use code, not a model. Chat-created announcements remain pinned to their origin.

The scheduler continues polling while work runs. Completion preserves edits
made during a run and retries saving the captured result without executing
again. Interrupted actions wait for user judgment. Notification delivery may
repeat after a crash between sending and acknowledgement; it never repeats
the action just to resend an answer. Legacy startup and synchronous delivery
remain the default.

Dotfiles grants the six `cron_*` tools to admitted chat profiles and supplies
shared conversational guidance. Shell remains owner-only. The runtime and
host changes must roll out together.

### Remaining acceptance checks

1. Pass focused runtime, channel, config, formatting, and documentation checks.
2. Complete independent review before opening draft PRs.
3. After merge and an authorized host rollout, test an owner DM, invited DM,
   and approved group through Telegram. Create and edit a one-shot and a
   recurrence, test a background run, receive results, and remove trial tasks.
4. Verify task ownership, current permission removal, no idle model requests,
   continued chat availability during slow work, and a remembered missed-task
   decision after restart.

No deployment or Telegram trial is included in the draft implementation.
To roll back, pause or remove active tasks before reverting runtime and host
settings. Revoking scheduling tools alone does not stop saved schedules.

## Artifacts

- [Background work lifecycle](../../../book/src/architecture/background-work-lifecycle.md)
- [Scheduler](../../../../crates/zeroclaw-runtime/src/cron/scheduler.rs)
- [Task store](../../../../crates/zeroclaw-runtime/src/cron/store.rs)
- [Host runbook](https://github.com/averagechris/dotfiles/blob/main/docs/zeroclaw.md)
