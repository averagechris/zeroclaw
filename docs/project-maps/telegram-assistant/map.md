# Personal Telegram assistant on ZeroClaw

## Destination

Extend the deployed Telegram assistant with concrete tools. For each capability,
choose the integration, chat permissions, account ownership, and smallest useful
end-to-end check before implementation.

## Notes

Current scope, 2026-10-03:

- The [deployed baseline](issues/23-deployed-baseline.md) includes dynamic
  invitations, isolated memo stores, media understanding, image generation and
  editing, Kagi search and extraction, location input, and owner video editing.
- Invitation enrollment and private memory passed the operator's Telegram test.
  Memo wakes use 256 lines, configured in Nix. Useful memories are saved quietly.
- Cross-chat memory sharing remains deferred. Owner shell remains private to the
  owner DM and keeps the host sandbox. Invited DMs and groups do not get shell.
- The next extension is [owner shell and browser tools](issues/24-remote-codex-control.md),
  using `rdny` and `gh`. GitHub CLI will use a personal token limited to selected
  personal repositories. Thorny will use a separate ChatGPT/Codex account. Do not
  connect work sessions or credentials. Linear, Granola, and Slack are excluded.
  Native desktop control and Codex task orchestration are deferred.
- The original V1 decisions below are historical where later decisions supersede
  them. Live host configuration and rollout procedures live in
  [the host runbook](https://github.com/averagechris/dotfiles/blob/main/docs/zeroclaw.md).

Historical constraints from the original discovery:

- One daemon, one host. Hostile multi-tenant isolation is not a goal. The
  goal is that the partner cannot accidentally act through the owner's
  accounts or reach shell-class tools.
- Three principals: owner DM, partner DM, and one Telegram group containing
  both humans and the bot. DMs need separate conversation contexts; the group
  has one shared session.
- Future account integrations require each human's own consent. The household
  group uses its own integration identity and never borrows a personal
  account implicitly. None are in V1.
- One Telegram bot identity is the selected V1 preference, but it is not
  immutable; three separate bot identities remain a fallback if one-bot
  routing proves unsafe or disproportionate.
- Temporary cross-principal access (for example the household agent borrowing
  the owner's calendar with approval) is explicitly deferred to a later
  version.
- Kagi web search is the first shared tool after the access, routing, session,
  and permission-boundary baseline. It uses the owner's deployment-level Kagi
  API key, plain search with safe search, and the existing result cap. The
  owner's Kagi API usage limit is already configured in his account. Kagi
  misconfiguration fails at startup. If Kagi fails at runtime, the agent asks
  before using DuckDuckGo; the person's "yes" in chat is enough.
- All three agents get the built-in memory tools (`memory_recall`,
  `memory_store`, `memory_forget`) on the default SQLite backend. Each
  agent's memory stays its own.
- Once the memory-tools-only baseline works, only the owner's private agent may run
  shell from Telegram, without a per-call human prompt. A command denylist
  is not a reliable safety boundary; route isolation and the host-user
  boundary matter. The partner and household agents must never receive shell.
  The owner accepts that shell can read credentials held by the daemon's
  service user, and everything else it owns, including the partner's DM
  transcripts, the group history, and every agent's memory. He is the
  system's administrator and is expected to have that access.
- Prompt injection into the owner agent is an accepted V1 risk. Untrusted
  text such as search results shares a context with unprompted shell, and
  that shell can rewrite the daemon's config. No mitigation is claimed for
  V1; candidate future mitigations are listed in
  [Owner approval posture](issues/07-owner-approval-posture.md).
- Deploy via the existing NixOS module as a system service under its dedicated
  user. The bot uses outbound Telegram long polling, not an inbound tunnel.
  Host-specific wiring and credentials live outside this repository.
  Keep the module's systemd sandbox for the owner's shell; grant specific
  paths and tools through small new module options (`bindPaths`,
  `extraPackages`), not by loosening hardening.
  Polling was rechecked against webhooks and kept. Turn on partial streaming
  at the default update interval, and disable the bot's group privacy mode in
  BotFather so it sees ordinary group messages. Bootstrap steps live in the
  [setup runbook](setup.md); keep it current as fork features land.
- V1 processes text-only Telegram messages. Inbound attachments must be
  rejected or ignored before shared channel storage; multimodal input is the
  intended fast follow, not part of the first isolation proof.
- Keep each agent's long-term memory separate in V1; do not grant the
  household agent `read_memory_from` a personal agent. Use existing logs
  during routing rollout; review richer receipts when account tools arrive.
- All three agents may use the owner's ChatGPT/Codex provider login. Allow
  session-level model switching, including a shared model choice in the group;
  deny agent-wide model rebinding from Telegram in V1.
- Choose future tool-extension mechanisms per concrete tool, with explicit
  per-agent grants, secure defaults, tests, and docs.

Repository facts that shape the plan:

- This checkout is the `averagechris/zeroclaw` fork. Its maintained branch is
  `main@origin`; upstream `zeroclaw-labs/zeroclaw` keeps `master@upstream`.
  See [fork maintenance](../../fork-maintenance.md).
- Inbound routing is channel-key to agent, one owner per key
  (`AgentRouter::resolve` in `crates/zeroclaw-channels/src/orchestrator/mod.rs`,
  `Config::agent_for_channel` in `crates/zeroclaw-config/src/schema.rs`).
  The fork now also supports explicit peer routes and dynamic enrollment within
  a Telegram alias; see the [Telegram guide](../../book/src/channels/telegram.md).
- Upstream is building per-sender authorization under tracker
  [#8290](https://github.com/zeroclaw-labs/zeroclaw/issues/8290) and
  [ADR-017](../../book/src/architecture/decisions/ADR-017-inbound-authentication-and-principals.md)
  (status: proposed). Channel identity is listed there as a later extension,
  not a closure requirement.
- Tool extension seams that exist today: a provider slot inside the native
  `web_search_tool` (`crates/zeroclaw-tools/src/web_search_provider_routing.rs`,
  global `[web_search]` section, one key per daemon), MCP servers granted per
  agent through `mcp_bundles`, WASM tool plugins behind a source-build
  feature, and GET-only `http` skills. Kagi also publishes its own MCP server
  (`https://mcp.kagi.com/mcp`, bearer auth). Comparison in
  [Tool extension options](artifacts/tool-extension-options.md).
- Docs to reread before implementation: [Telegram](../../book/src/channels/telegram.md),
  [Peer groups](../../book/src/channels/peer-groups.md),
  [Autonomy levels](../../book/src/security/autonomy.md),
  [MCP](../../book/src/tools/mcp.md),
  [Runtime internals](../../book/src/agents/internals.md).

Skills to consult: `how` for runtime traces, `architect` for the topology and
credential boundaries, `why` only when upstream rationale matters.

Decision frontier: [issues/](issues/)

Agents scan this directory for open, unblocked child decisions.

## Decisions so far

- [Deployed baseline](issues/23-deployed-baseline.md): invitations and isolated memo are verified; media tools, Kagi extraction, owner video editing, and location input are deployed.
- [Memo trial](issues/22-memo-trial.md): use a workspace-bound CLI tool with empty stores; disable native durable memory and defer sharing.

- [Launch sequence](issues/16-launch-sequence.md): establish the
  memory-tools-only baseline first, then add Kagi as the first shared tool;
  defer other tools.
- [Original memory tools](issues/21-memory-tools.md): the SQLite bootstrap decision is superseded by the memo trial.
- [Inbound routing model](issues/01-inbound-routing-model.md): one Telegram
  alias maps to exactly one agent; principals cannot be split across agents
  within one bot without a code change.
- [Session and memory scoping](issues/02-session-and-memory-scoping.md): DM
  and group sessions are already distinct per sender and chat; the group can
  share one session with `per_user_session = false`; long-term memory is
  per agent, not per sender.
- [Credential and OAuth ownership](issues/03-credential-and-oauth-ownership.md):
  ZeroClaw's own PKCE is for model-provider login only; the built-in
  `google_workspace` tool is one global account; per-principal credentials are
  only expressible as separate MCP server entries granted through
  `mcp_bundles`, on top of one flat encrypted secret store.
- [Tool gating on the Telegram path](issues/04-tool-gating-on-telegram-path.md):
   approvals go to the originating chat and any authorized peer in that chat
   may answer; `shell` skips the outer approval prompt on channel turns unless
   listed in `always_ask`, so capability lists, not approvals, are the fence.
- [Bot identity topology](issues/05-bot-identity-topology.md): one bot is the
  selected V1 preference for routing the owner DM, partner DM, and shared group
  to distinct agent profiles with separate grants; three bots remain a
  fallback if one-bot routing is unsafe or disproportionate.
- [Partner capability profile](issues/06-partner-capability-profile.md): the
   partner's private agent gets shared Kagi in V1; later she may use her own
   authorized account tools, never the owner's credentials or host-control
   tools.
- [Owner approval posture](issues/07-owner-approval-posture.md): only the owner
  agent may run shell from Telegram without per-call approval, after routing
  isolation is proved; a command denylist is not a security boundary.
- [Partner action confirmation](issues/12-partner-action-confirmation.md): use
   company or third-party MCP tools with their normal intended/default behavior
   and ZeroClaw's ordinary policy; do not add a custom confirmation gate for
   standard tools. Revisit action and approval behavior during plan refinement
   if a future tool is custom-built by us. Connector credentials remain
   account-scoped and do not grant access to another person's account.
- [Household identity boundary](issues/08-household-identity-and-integrations.md):
   use a dedicated household account or provider-scoped resources explicitly
   shared with the household agent; never grant it an entire personal account
   implicitly.
- [Household integration set](issues/13-household-integration-set.md): no
  household account integrations in V1; memory tools, then the shared Kagi
  tool.
- [Group approval semantics](issues/10-group-approval-semantics.md): either
  member can approve future household actions, but sensitive calls must
  prompt every time; V1 has no household action tools.
- [Kagi integration path](issues/14-kagi-search-tool.md): implement Kagi as a
  provider backend inside the existing `web_search_tool`, sharing the owner's
  deployment-level API key across approved agent profiles rather than using
   Kagi's separate MCP server.
- [Kagi feature and cost settings](issues/17-kagi-feature-and-cost-settings.md):
  plain safe search, existing result cap, no lenses or Extract; the existing
  Kagi account usage limit stays owner-managed. Misconfiguration fails at
  startup; runtime failures let the agent ask before using DuckDuckGo.
- [Tool extension DX](issues/15-tool-extension-dx.md): choose the mechanism
  per future tool rather than commit to one default across the fork.
- [Upstream alignment](issues/11-upstream-alignment.md): principal/RBAC work
  will not supply shared-bot agent routing soon; carry a narrow fork route
  after proving isolation, and defer owner-only approval changes.
- [Always-on host boundary](issues/19-always-on-host-boundary.md): use the
  existing NixOS system-service module, keep its state and key persistent, and
  accept that owner shell has access to service-user credentials; long polling
  stays over a webhook, with partial streaming and group privacy mode off.
- [Model login and switching](issues/20-model-login-and-switching.md): share
  one owner-controlled Codex subscription login; allow session-level model
  changes but no agent-wide rebinding from Telegram.
- [One-bot route and isolation proof](issues/18-one-bot-route-and-isolation-proof.md):
  use an exact numeric Telegram route table as the alias's sole agent binding;
  deny unknowns, keep V1 text-only, and prove partner/group cannot reach shell.
  Profiles use explicit per-stage `allowed_tools` lists, never
  `deny_all_tools`.

## Not yet specified

- rdny on Thorny: verify how a dedicated Chromium session can run within the
  existing service sandbox. See [owner shell and browser tools](issues/24-remote-codex-control.md).
- Location tools: choose reverse geocoding and native Telegram pin delivery.
  Coordinate input already works; an exact street address is not supplied by it.
- Kagi lenses: teach their behavior and choose useful filters before creating
  named lenses. Default search remains the current preference.
- Personal account tools: choose a concrete service before resuming
  [per-principal integrations](issues/09-per-principal-integration-mechanism.md).

## Out of scope

- Cross-chat memory sharing and temporary access to another principal's account.
  Both remain deferred until the owner revisits their consent rules.
- Automatic fallback to paid model APIs. The deployment uses the selected
  subscription providers and does not silently incur a new provider's charges.
- Hostile multi-tenant isolation, OIDC login, and gateway or web dashboard
  authentication. These are outside the personal assistant's current scope.
- Broad integration frameworks without a selected tool. Follow
  [Tool extension DX](issues/15-tool-extension-dx.md) for each concrete addition.
- Native desktop control and Codex task orchestration from Telegram. The current
  scope is owner-only shell access with `rdny` and `gh`.
