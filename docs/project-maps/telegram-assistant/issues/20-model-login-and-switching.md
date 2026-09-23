# Model login and switching

Type: grilling
Status: resolved
Blocked by: none

## Question

Can the owner sign in using his ChatGPT account and let each Telegram agent
switch among supported models at runtime without one principal changing
another agent's model or private session?

## Answer

Use ZeroClaw's existing `openai-codex` subscription login for the owner's
ChatGPT account, without putting an API key in the provider entry. Documented
login is `zeroclaw auth login --model-provider openai-codex`; the provider uses
`wire_api = "responses"` and `requires_openai_auth = true` (see
`docs/book/src/providers/openai-codex-subscription.md`). Authentication is
stored under the daemon config root and encrypted with that root's secret
key. Do not perform login or inspect the account in discovery.
During deployment, complete the login under the module instance's service
user and `ZEROCLAW_CONFIG_DIR`, not the administrator's default home, so the
daemon reads the same auth profile and `.secret_key`. Verify auth status,
the signed-in account's actual served model IDs, and one live request; do not
assume API-only models are on the subscription backend. Prefer a fresh
`--device-code` login over importing `~/.codex/auth.json`, because refresh
tokens rotate and a shared imported credential breaks whichever side
refreshes second. Commands are in the [setup runbook](../setup.md).

All three agents may use this same owner-controlled provider login. This
shares the provider's allowance and sends partner and group prompts through
the owner's account; the owner explicitly accepts that. It does not grant
either other agent the owner's shell or any future personal account
integration.

Allow the documented `/models` and ordinary `/model` or `/model --user`
commands for runtime *session* choices. Keep `admin_for_agent_scope = false`
for both humans in V1, so `/model --agent` cannot rebind another agent's
default. Neither ordinary switching nor `/models` should write `config.toml`.
With `per_user_session = false`, either human can change the group's shared
session model; neither should change a DM session via a group message.
Verify these properties against the actual three-way routing tests before
enabling shell. Only models available under the signed-in account can be
selected; the owner can choose initial defaults in the host configuration.
