# Setup runbook: household Telegram assistant

Bootstrap instructions for the one-bot, three-agent assistant described in
the [Telegram assistant map](map.md). Written for the owner and for agents
helping him set it up later. Follow the sections in order.

This file must never contain a real token, key, Telegram ID, or
host-specific path. Use placeholders here and keep real values in the host
configuration and its secret manager, outside this repository.

> [!IMPORTANT]
> Parts of this runbook depend on fork features that do not exist yet: the
> Telegram route table and text-only guard ([One-bot route and isolation
> proof](issues/18-one-bot-route-and-isolation-proof.md)) and the Kagi
> provider ([Kagi feature and cost
> settings](issues/17-kagi-feature-and-cost-settings.md)). Config shown for
> them is **proposed**. Update this file when those land, and check every
> field name against `crates/zeroclaw-config/src/schema.rs` before use.

## Overview

| Step | Where | Result |
|---|---|---|
| 1. Create the bot | BotFather in Telegram | Bot token, privacy mode off |
| 2. Create the group and collect IDs | Telegram, then one `curl` | Three numeric IDs |
| 3. Provision secrets | Host secret manager | Environment file |
| 4. Declare the instance | Host NixOS configuration | `zeroclaw-<name>.service` |
| 5. Log in to ChatGPT/Codex | Host shell, as the service user | Encrypted auth profile |
| 6. Start and verify | Host shell and Telegram | Stage 1 checks pass |
| 7. Later stages | Host configuration | Kagi, then owner shell |

## 1. Create the bot in BotFather

In Telegram, open `@BotFather`:

1. `/newbot`, then choose a display name and a username ending in `bot`.
   BotFather replies with the bot token. Treat it as a password: store it
   straight into the secret manager (step 3), not in chat, notes, or a repo.
2. `/setprivacy`, select the bot, choose **Disable**. With privacy mode on,
   the bot sees only commands, @mentions, and replies to its own messages in
   groups, so ordinary household chat would never reach it.
3. Do this **before** adding the bot to any group. Telegram applies the
   privacy setting when the bot joins; if the bot is already in the group,
   remove it and add it again after changing the setting.

Step 2 finishes BotFather setup with `/setjoingroups`.

## 2. Create the group and collect numeric IDs

Routing uses exact numeric IDs, never usernames. You need three:

| ID | Looks like | Where it comes from |
|---|---|---|
| Owner user ID | positive integer | `message.from.id` in the owner's DM |
| Partner user ID | positive integer | `message.from.id` in the partner's DM |
| Household group chat ID | negative, usually `-100…` | `message.chat.id` in the group |

In a private chat the chat ID equals the user's ID; the route matches both.

1. Create the household group with both humans, then add the bot.
2. In BotFather, `/setjoingroups` → **Disable**. Nobody can add the bot to
   another group afterward. Routing already ignores unknown chats; this is
   an extra fence.
3. Make sure no ZeroClaw daemon is running with this token. Telegram
   allows one poller per token and returns 409 to a second one.
4. Each human sends the bot a DM ("hi"), and one of you posts a message in
   the group.
5. On the host, read the pending updates without printing the token. For
   example, with the token in a root-only file:

   ```sh
   TOKEN="$(sudo cat /path/to/bot-token)"
   curl -s "https://api.telegram.org/bot${TOKEN}/getUpdates" \
     | jq '.result[] | (.message // .edited_message)
           | {chat_id: .chat.id, chat_type: .chat.type,
              from_id: .from.id, from_name: .from.first_name}'
   unset TOKEN
   ```

   If the list is empty, send another message and rerun; Telegram keeps
   undelivered updates for 24 hours. This call does not consume them.

Record the three IDs in the host configuration (step 4), not here.

If the group is ever converted to a supergroup, for example by enabling
some admin features, Telegram gives it a new `-100…` chat ID. The route then
stops matching and the bot goes silent in the group. Repeat this step and
update the route.

## 3. Provision secrets

Create an environment file through the host's secret manager, such as agenix
or sops-nix, readable only by root. systemd loads it into the unit:

```sh
BOT_TOKEN=<telegram bot token>
KAGI_API_KEY=<kagi api key>        # needed from stage 2 onward
```

The NixOS module substitutes `$BOT_TOKEN`-style references into the
rendered config at unit start. The world-readable Nix store copy keeps only
the placeholders. The resolved `config.toml` is mode 0600, owned by the
service user.

The owner agent's shell runs as that service user, so it can read these
values once shell is enabled (stage 3). That is accepted; see [Always-on
host boundary](issues/19-always-on-host-boundary.md).

## 4. Declare the instance in the host configuration

Import `nix/module.nix` from this fork and declare one instance. The module
creates user `zeroclaw-<name>`, state directory `/var/lib/zeroclaw-<name>`,
and unit `zeroclaw-<name>.service`.

Things that differ from the module's own examples:

- Do **not** use `allowed_users`. The module README and comments predate the
  current Telegram schema; admission uses `peer_groups`.
- The module re-renders `config.toml` from Nix on every unit start, so
  anything the daemon writes to it at runtime is lost on restart, including
  `/bind` pairings. Pre-authorize both humans in `peer_groups` instead of
  using `/bind`.
- For a routed alias, do **not** list `telegram.<alias>` in any agent's
  `channels`. The route table is the only binding.

Stage 1 sketch (the `routes` block is proposed fork config):

```nix
services.zeroclaw.instances.<name> = {
  environmentFile = config.age.secrets.<secret>.path;

  settings = {
    providers.models.openai.codex = {
      model = "<exact served model ID>";   # see step 5
      wire_api = "responses";
      requires_openai_auth = true;         # no api_key field
    };

    agents = {
      owner     = { model_provider = "openai.codex"; risk_profile = "owner"; };
      partner   = { model_provider = "openai.codex"; risk_profile = "partner"; };
      household = { model_provider = "openai.codex"; risk_profile = "household"; };
    };

    risk_profiles = let
      memory = [ "memory_recall" "memory_store" "memory_forget" ];
    in {
      owner = {
        allowed_tools = memory;
        auto_approve = [ "memory_recall" "memory_store" ];
      };
      partner = {
        allowed_tools = memory;
        excluded_tools = [ "shell" ];
        auto_approve = [ "memory_recall" "memory_store" ];
      };
      household = {
        allowed_tools = memory;
        excluded_tools = [ "shell" ];
        auto_approve = [ "memory_recall" "memory_store" ];
      };
    };

    peer_groups.telegram_home = {
      channel = "telegram.home";
      external_peers = [ "<owner user id>" "<partner user id>" ];
      # admin_for_agent_scope stays false: no /model --agent from Telegram.
    };

    channels.telegram.home = {
      enabled = true;
      bot_token = "$BOT_TOKEN";
      per_user_session = false;    # one shared group session; DMs unaffected
      stream_mode = "partial";     # replies appear as they are written
      # draft_update_interval_ms: leave at the default for now.

      # PROPOSED fork field; spelling not final.
      routes = {
        dm = {
          "<owner user id>" = "owner";
          "<partner user id>" = "partner";
        };
        groups = { "<household group chat id>" = "household"; };
      };
    };
  };
};
```

Leave `[memory]` at its defaults: the SQLite backend, and `auto_save = true`.
Do not set `read_memory_from` on any agent. `auto_approve` replaces the
default list rather than extending it, so list every unprompted tool.

## 5. Log in to ChatGPT/Codex as the service user

The daemon reads its auth profile from its own config directory, encrypted
with that directory's `.secret_key`. Log in as the service user, with the
unit's config directory, not from your own home:

```sh
sudo -u zeroclaw-<name> \
  env ZEROCLAW_CONFIG_DIR=/var/lib/zeroclaw-<name> \
  zeroclaw auth login --model-provider openai-codex --device-code

sudo -u zeroclaw-<name> \
  env ZEROCLAW_CONFIG_DIR=/var/lib/zeroclaw-<name> \
  zeroclaw auth status
```

Use the same `zeroclaw` build as the unit. `--device-code` prints a URL and
code to approve from any browser, so it works on a headless host.

Prefer this fresh login over `--import ~/.codex/auth.json`. Refresh tokens
rotate: if the daemon and your desktop Codex CLI share one imported
credential, each refresh invalidates the other's. A separate login should
get its own token chain; confirm by using both for a day.

Then pick the served model ID. The Codex backend accepts only exact served
IDs, and its catalog changes. Query it as described in
`docs/book/src/providers/openai-codex-subscription.md` and put the result in
`providers.models.openai.codex.model`.

Back up `/var/lib/zeroclaw-<name>`, including `.secret_key`. Encrypted
secrets and the auth profile are unreadable without it.

## 6. Start and verify stage 1

```sh
sudo nixos-rebuild switch
systemctl status zeroclaw-<name>
journalctl -u zeroclaw-<name> -f
```

Check in Telegram:

- The owner DM, partner DM, and group each get a reply.
- In the group, a message without an @mention gets a reply, which proves
  privacy mode is off.
- A message from a third account, or the bot added to another group
  (temporarily re-enable joining to test), gets no agent reply.
- `memory_store` a fact in one chat, then ask about it in the other two;
  it must not surface.
- `/model` in the group changes the group only; DMs keep their own model.

The full proof list is in [One-bot route and isolation
proof](issues/18-one-bot-route-and-isolation-proof.md). Stage 3 must not
start until it passes.

## 7. Later stages

**Stage 2: Kagi.** Add `KAGI_API_KEY` to the environment file. Add
`web_search_tool` to every profile's `allowed_tools` and `auto_approve`.
Configure the provider (proposed fork fields):

```nix
settings.web_search = {
  enabled = true;
  search_provider = "kagi";
  kagi_api_key = "$KAGI_API_KEY";
  # DuckDuckGo fallback: offered by the agent only after Kagi fails,
  # and used only after the person says yes.
};
```

A misspelled provider or a missing key must stop the daemon at startup.
If it starts and silently uses DuckDuckGo, the fork change is broken.

**Stage 3: owner shell.** Only after stage 1's isolation proof passes. Add
`shell` to the owner profile's `allowed_tools`. Unrestricted shell also
needs the owner profile to opt out of the stock command restrictions.
Verify these names against the schema at the time:

```nix
risk_profiles.owner = {
  allowed_commands = [ "*" ];
  block_high_risk_commands = false;
  workspace_only = false;
};
```

The unit's systemd hardening stays on by decision. The shell cannot write
outside `/var/lib/zeroclaw-<name>`, read `/home`, or gain privileges, and it
has only the unit's default `PATH`. Grant reach explicitly (the read-write
and package options are proposed module additions; see [Always-on host
boundary](issues/19-always-on-host-boundary.md)):

```nix
services.zeroclaw.instances.<name> = {
  # Existing option: read-only binds, target = source.
  bindReadOnlyPaths."/var/lib/zeroclaw-<name>/mnt/notes" = "/path/to/notes";
  # PROPOSED: read-write binds.
  bindPaths."/var/lib/zeroclaw-<name>/mnt/scratch" = "/path/to/scratch";
  # PROPOSED: tools on the shell's PATH.
  extraPackages = [ pkgs.git pkgs.curl pkgs.jq ];
};
```

The service user must be able to read, or write, each source directory
through ordinary permissions, such as a shared group or an ACL. Files owned
by other users appear as `nobody` inside the sandbox. Until the proposed
options land, use `systemd.services."zeroclaw-<name>".path` and
`serviceConfig.BindPaths`. Do not loosen the hardening itself.

Never add `shell` to the partner or household profile.
