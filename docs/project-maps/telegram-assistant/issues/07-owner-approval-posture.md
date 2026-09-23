# Owner approval posture

Type: grilling
Status: resolved
Blocked by: none

## Question

For the owner's own agent, which tools should run unprompted from Telegram,
which should prompt in the owner's DM, and does the owner want a human
prompt before `shell` runs?

## Answer

For his private agent, the owner wants unrestricted shell access from Telegram
without a prompt on each call. Keep `shell` off the partner and household
agents entirely. The memory-tools-only routing and permission baseline must pass before
enabling the owner's shell. Add shared Kagi first, then enable owner shell only
after the route isolation checks explicitly cover tool access.

This gives the owner agent the effective privileges of the daemon's host user.
Do not describe `block_high_risk_commands` or a short list of forbidden
commands as protection against malicious or model-generated shell commands;
they cannot reliably enumerate dangerous shell behavior. The owner accepts
this risk for his own agent, not for other Telegram principals. Route unknown
identities to no agent and prove that partner and household turns cannot reach
the owner agent or its shell tool before enabling it. The host boundary is
settled in [Always-on host boundary](19-always-on-host-boundary.md).

Prompt injection is an accepted V1 risk. Kagi results and any other
untrusted text the owner agent reads enter the same context that can run
shell without a prompt. That shell can also rewrite the daemon's config,
including other agents' profiles and routes. The owner accepts this as the
system's administrator; the plan does not claim a mitigation. Candidate
mitigations to consider after V1 (none chosen):

- Split research from execution: give the owner agent a shell-less research
  subagent through delegation, so untrusted pages land in a context that
  cannot run commands.
- Prompt for shell only when the turn has already read untrusted content.
  This would need a fork change; stock `always_ask` is all-or-nothing.
- Put `shell` in the owner profile's `always_ask`, or gate it through
  `[security.otp] gated_actions`, accepting the per-call friction.
- Keep the daemon's config out of the service user's write path.
  The NixOS module already re-renders `config.toml` from the Nix store on
  every start, which undoes config edits on restart. The daemon's own writes
  (for example `/bind`) are lost for the same reason. An in-place reload
  (`/admin/reload`, `docs/book/src/ops/service.md`) restarts the daemon loop
  without a new `ExecStartPre`, so it would likely apply an edited file until
  the next unit restart; confirm before relying on this.
- The module's systemd hardening already limits damage: `ProtectSystem =
  "strict"`, `ProtectHome = true`, `NoNewPrivileges`, and a writable
  `dataDir` only (`nix/module.nix`). The owner's shell therefore cannot
  touch `/home`, system files, or sudo; it can reach the network and
  everything under `dataDir`.

What the evidence fixes ([Tool gating on the Telegram path](04-tool-gating-on-telegram-path.md)):

- Under `supervised` reached from Telegram, `shell` runs when the model sets
  `approved: true`, because the channel `ApprovalManager` is non-interactive
  and `non_interactive_shell_requires_approval` is false. That is the
  owner's chosen behavior. If it is ever reversed, `always_ask = ["shell"]`
  on the owner profile forces a prompt; `always_ask` beats `full`,
  `auto_approve`, and a prior "Always" answer.
- Do not add `allowed_commands` or `block_high_risk_commands` as a claimed
  safety boundary. They may be set for convenience, but they do not change
  the owner's accepted risk.

There are no owner MCP actions in V1. Choose the remaining risk-profile knobs
during implementation without accidentally granting shell to another agent.

## Artifacts

None.
