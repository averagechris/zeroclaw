//! Channel delivery-default injection for cron_add and interactive tools.

const AUTO_DELIVERY_DEFAULT_CHANNELS: &[&str] = &[
    "telegram",
    "discord",
    "slack",
    "mattermost",
    "matrix",
    "dingtalk",
    "lark",
    "feishu",
];

/// Tools that take an optional `channel` arg and otherwise pick
/// `channels.iter().next()` (HashMap order). Without an originating-channel
/// default they can route an interactive prompt to a random external channel
/// while the user is sitting in the RPC/ACP/WS session that issued the turn —
/// which looks like a hang until timeout/cancel.
const ORIGINATING_CHANNEL_TOOLS: &[&str] = &["ask_user", "poll", "escalate_to_human"];

/// When `tool_args` omits `channel`, fill it from the turn's originating
/// channel name so interactive tools prefer the conversation back-channel
/// (`rpc` / `acp` / `ws` / …) over an arbitrary map entry.
pub(crate) fn maybe_inject_originating_channel(
    tool_name: &str,
    tool_args: &mut serde_json::Value,
    channel_name: &str,
) {
    if !ORIGINATING_CHANNEL_TOOLS.contains(&tool_name) {
        return;
    }
    let channel_name = channel_name.trim();
    if channel_name.is_empty() || channel_name == "agent" || channel_name == "daemon" {
        // Configless / non-channel turns have no conversation back-channel to
        // prefer; leave selection to the tool's existing map fallback.
        return;
    }
    let Some(args) = tool_args.as_object_mut() else {
        return;
    };
    let needs_channel = args
        .get("channel")
        .and_then(serde_json::Value::as_str)
        .is_none_or(|value| value.trim().is_empty());
    if needs_channel {
        args.insert(
            "channel".to_string(),
            serde_json::Value::String(channel_name.to_string()),
        );
    }
}

pub(crate) fn maybe_inject_channel_delivery_defaults(
    tool_name: &str,
    tool_args: &mut serde_json::Value,
    channel_name: &str,
    channel_alias: Option<&str>,
    channel_reply_target: Option<&str>,
) {
    // Interactive tools first — independent of cron delivery defaults.
    maybe_inject_originating_channel(tool_name, tool_args, channel_name);

    if tool_name != "cron_add" {
        return;
    }

    if !AUTO_DELIVERY_DEFAULT_CHANNELS.contains(&channel_name) {
        return;
    }

    let Some(reply_target) = channel_reply_target
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return;
    };

    let Some(args) = tool_args.as_object_mut() else {
        return;
    };

    let is_agent_job = args
        .get("job_type")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|job_type| job_type.eq_ignore_ascii_case("agent"))
        || args
            .get("prompt")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|prompt| !prompt.trim().is_empty());
    if !is_agent_job {
        return;
    }

    // Delivery resolves `<type>.<alias>`; the bare type is registered only
    // while that type has one configured instance (`configured_channel_map`).
    let delivery_channel = match channel_alias
        .map(str::trim)
        .filter(|alias| !alias.is_empty())
    {
        Some(alias) => format!("{channel_name}.{alias}"),
        None => channel_name.to_string(),
    };

    let default_delivery = || {
        serde_json::json!({
            "mode": "announce",
            "channel": delivery_channel,
            "to": reply_target,
        })
    };

    match args.get_mut("delivery") {
        None => {
            args.insert("delivery".to_string(), default_delivery());
        }
        Some(delivery) if delivery.is_null() => {
            *delivery = default_delivery();
        }
        Some(serde_json::Value::Object(delivery)) => {
            if delivery
                .get("mode")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|mode| mode.eq_ignore_ascii_case("none"))
            {
                return;
            }

            delivery
                .entry("mode".to_string())
                .or_insert_with(|| serde_json::Value::String("announce".to_string()));

            // Unset, or the bare type the turn-context block recommends: both
            // name this turn's instance. Anything else is a chosen destination.
            let needs_resolved_channel = delivery
                .get("channel")
                .and_then(serde_json::Value::as_str)
                .is_none_or(|value| {
                    let value = value.trim();
                    value.is_empty() || value.eq_ignore_ascii_case(channel_name)
                });
            if needs_resolved_channel {
                delivery.insert(
                    "channel".to_string(),
                    serde_json::Value::String(delivery_channel.clone()),
                );
            }

            let needs_target = delivery
                .get("to")
                .and_then(serde_json::Value::as_str)
                .is_none_or(|value| value.trim().is_empty());
            if needs_target {
                delivery.insert(
                    "to".to_string(),
                    serde_json::Value::String(reply_target.to_string()),
                );
            }
        }
        Some(_) => {}
    }
}

/// Keep scheduled task results in the conversation that created or updated
/// the task. The model can choose to suppress an add announcement, but it
/// cannot redirect an announcement to another configured channel or target.
pub(crate) fn pin_cron_delivery_to_origin(
    tool_name: &str,
    tool_args: &mut serde_json::Value,
    channel_name: &str,
    channel_alias: Option<&str>,
    channel_reply_target: Option<&str>,
) {
    if tool_name == "cron_add"
        && let Some(args) = tool_args.as_object_mut()
    {
        args.entry("delete_after_run")
            .or_insert(serde_json::Value::Bool(false));
    }
    if !matches!(tool_name, "cron_add" | "cron_update")
        || !AUTO_DELIVERY_DEFAULT_CHANNELS.contains(&channel_name)
    {
        return;
    }
    let Some(reply_target) = channel_reply_target
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return;
    };

    let delivery_channel = match channel_alias
        .map(str::trim)
        .filter(|alias| !alias.is_empty())
    {
        Some(alias) => format!("{channel_name}.{alias}"),
        None => channel_name.to_string(),
    };

    let Some(args) = tool_args.as_object_mut() else {
        return;
    };
    let delivery = if tool_name == "cron_add" {
        Some(
            args.entry("delivery")
                .or_insert_with(|| serde_json::json!({"mode": "announce"})),
        )
    } else {
        args.get_mut("patch").and_then(|patch| {
            // cron_update accepts a JSON object encoded as a string. Normalize
            // that supported form before pinning so the tool cannot persist a
            // model supplied destination that bypasses review-mode routing.
            if let Some(encoded) = patch.as_str()
                && let Ok(parsed) = serde_json::from_str::<serde_json::Value>(encoded)
                && parsed.is_object()
            {
                *patch = parsed;
            }
            patch
                .as_object_mut()
                .and_then(|patch| patch.get_mut("delivery"))
        })
    };
    let Some(delivery) = delivery else {
        return;
    };
    if delivery.is_null() && tool_name == "cron_add" {
        *delivery = serde_json::json!({"mode": "announce"});
    }
    let Some(delivery) = delivery.as_object_mut() else {
        return;
    };
    if delivery
        .get("mode")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|mode| mode.eq_ignore_ascii_case("none"))
    {
        return;
    }

    delivery.insert(
        "channel".to_string(),
        serde_json::Value::String(delivery_channel),
    );
    delivery.insert(
        "to".to_string(),
        serde_json::Value::String(reply_target.to_string()),
    );
    // Telegram carries topic identity in reply_target (`chat_id:thread_id`).
    // Drop a separate model-supplied thread so it cannot redirect delivery.
    if channel_name == "telegram" {
        delivery.remove("thread_id");
    }
}

/// Bind a model-requested reaction to the active inbound conversation.
///
/// The current turn's channel object, channel name, and reply target are the
/// canonical scope. The model may select a message and emoji, but cannot
/// redirect the reaction to another channel or recipient.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReactionScopeError {
    MissingConversation,
    InvalidArguments,
}

impl ReactionScopeError {
    pub(crate) fn message_key(self) -> &'static str {
        match self {
            Self::MissingConversation => "turn-reaction-missing-conversation",
            Self::InvalidArguments => "turn-reaction-invalid-arguments",
        }
    }
}

pub(crate) fn scope_reaction_to_current_conversation(
    tool_name: &str,
    tool_args: &mut serde_json::Value,
    channel_name: &str,
    channel_alias: Option<&str>,
    channel_reply_target: Option<&str>,
) -> Result<(), ReactionScopeError> {
    if tool_name != "reaction" {
        return Ok(());
    }

    let channel_name = channel_name.trim();
    if channel_name.is_empty() || matches!(channel_name, "agent" | "daemon") {
        return Err(ReactionScopeError::MissingConversation);
    }

    // `channel_alias` is Some even for an unaliased channel when a live
    // Channel object is present. None therefore means the turn has no
    // authoritative channel object to bind against.
    let Some(channel_alias) = channel_alias else {
        return Err(ReactionScopeError::MissingConversation);
    };
    let channel_alias = channel_alias.trim();
    let channel_key = if channel_alias.is_empty() {
        channel_name.to_string()
    } else {
        format!("{channel_name}.{channel_alias}")
    };

    let Some(reply_target) = channel_reply_target
        .map(str::trim)
        .filter(|target| !target.is_empty())
    else {
        return Err(ReactionScopeError::MissingConversation);
    };

    let Some(args) = tool_args.as_object_mut() else {
        return Err(ReactionScopeError::InvalidArguments);
    };
    args.insert(
        "channel".to_string(),
        serde_json::Value::String(channel_key),
    );
    args.insert(
        "channel_id".to_string(),
        serde_json::Value::String(reply_target.to_string()),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        maybe_inject_channel_delivery_defaults, maybe_inject_originating_channel,
        pin_cron_delivery_to_origin,
    };

    #[test]
    fn null_delivery_is_replaced_with_channel_defaults() {
        let mut args = serde_json::json!({
            "job_type": "agent",
            "delivery": null,
        });

        maybe_inject_channel_delivery_defaults(
            "cron_add",
            &mut args,
            "discord",
            None,
            Some("channel-42"),
        );

        assert_eq!(
            args["delivery"],
            serde_json::json!({
                "mode": "announce",
                "channel": "discord",
                "to": "channel-42",
            })
        );
    }

    #[test]
    fn cron_add_preserves_explicit_delete_after_run_choice() {
        let mut args = serde_json::json!({
            "job_type": "agent",
            "delete_after_run": true,
            "delivery": { "mode": "announce" }
        });

        pin_cron_delivery_to_origin("cron_add", &mut args, "discord", None, Some("channel-42"));

        assert_eq!(args["delete_after_run"], true);
    }

    #[test]
    fn absent_delivery_defaults_to_the_originating_composite_channel() {
        let mut args = serde_json::json!({
            "job_type": "agent",
            "prompt": "check the inbox",
        });

        maybe_inject_channel_delivery_defaults(
            "cron_add",
            &mut args,
            "telegram",
            Some("work"),
            Some("chat-42"),
        );

        assert_eq!(
            args["delivery"],
            serde_json::json!({
                "mode": "announce",
                "channel": "telegram.work",
                "to": "chat-42",
            })
        );
    }

    #[test]
    fn omitted_channel_key_is_filled_with_the_composite_channel() {
        // Observed shape: the model emits mode and to, and omits channel.
        let mut args = serde_json::json!({
            "job_type": "agent",
            "delivery": { "mode": "announce", "to": "chat-42" },
        });

        maybe_inject_channel_delivery_defaults(
            "cron_add",
            &mut args,
            "telegram",
            Some("work"),
            Some("chat-42"),
        );

        assert_eq!(args["delivery"]["channel"], "telegram.work");
    }

    #[test]
    fn an_explicit_destination_is_never_overridden() {
        // Includes this turn's own instance, which must not gain a second suffix.
        for chosen in ["telegram.work", "telegram.other", "discord", "discord.ops"] {
            let mut args = serde_json::json!({
                "job_type": "agent",
                "delivery": { "mode": "announce", "channel": chosen, "to": "chat-42" },
            });

            maybe_inject_channel_delivery_defaults(
                "cron_add",
                &mut args,
                "telegram",
                Some("work"),
                Some("chat-42"),
            );

            assert_eq!(args["delivery"]["channel"], chosen, "chosen={chosen}");
        }
    }

    #[test]
    fn bare_originating_type_is_upgraded_to_the_composite_channel() {
        // The turn-context block recommends this spelling to the model.
        for spelling in ["telegram", "TELEGRAM", "  telegram  "] {
            let mut args = serde_json::json!({
                "job_type": "agent",
                "delivery": { "mode": "announce", "channel": spelling, "to": "chat-42" },
            });

            maybe_inject_channel_delivery_defaults(
                "cron_add",
                &mut args,
                "telegram",
                Some("work"),
                Some("chat-42"),
            );

            assert_eq!(
                args["delivery"]["channel"], "telegram.work",
                "spelling={spelling}"
            );
        }
    }

    #[test]
    fn mode_none_still_suppresses_every_injection() {
        let mut args = serde_json::json!({
            "job_type": "agent",
            "delivery": { "mode": "none" },
        });

        maybe_inject_channel_delivery_defaults(
            "cron_add",
            &mut args,
            "telegram",
            Some("work"),
            Some("chat-42"),
        );

        assert_eq!(args["delivery"], serde_json::json!({ "mode": "none" }));
    }

    #[test]
    fn cron_add_pins_explicit_shell_and_agent_delivery_to_origin() {
        for job_type in ["shell", "agent"] {
            let mut args = serde_json::json!({
                "job_type": job_type,
                "delivery": {
                    "mode": "announce",
                    "channel": "discord.ops",
                    "to": "foreign-channel",
                    "thread_id": "foreign-thread"
                }
            });

            pin_cron_delivery_to_origin(
                "cron_add",
                &mut args,
                "telegram",
                Some("work"),
                Some("chat-42:topic-7"),
            );

            assert_eq!(args["delivery"]["channel"], "telegram.work");
            assert_eq!(args["delivery"]["to"], "chat-42:topic-7");
            assert!(args["delivery"].get("thread_id").is_none());
        }
    }

    #[test]
    fn cron_add_shell_without_delivery_defaults_to_origin_announcement() {
        let mut args = serde_json::json!({
            "job_type": "shell",
            "command": "echo done"
        });

        pin_cron_delivery_to_origin("cron_add", &mut args, "discord", None, Some("channel-42"));

        assert_eq!(
            args["delivery"],
            serde_json::json!({
                "mode": "announce",
                "channel": "discord",
                "to": "channel-42"
            })
        );
        assert_eq!(args["delete_after_run"], false);
    }

    #[test]
    fn cron_update_pins_only_present_delivery_patch() {
        let mut args = serde_json::json!({
            "job_id": "task-1",
            "patch": {
                "delivery": {
                    "mode": "announce",
                    "channel": "discord.ops",
                    "to": "foreign-channel",
                    "thread_id": "foreign-thread"
                }
            }
        });

        pin_cron_delivery_to_origin(
            "cron_update",
            &mut args,
            "telegram",
            Some("work"),
            Some("chat-42:topic-7"),
        );

        assert_eq!(args["patch"]["delivery"]["channel"], "telegram.work");
        assert_eq!(args["patch"]["delivery"]["to"], "chat-42:topic-7");
        assert!(args["patch"]["delivery"].get("thread_id").is_none());

        let mut omitted = serde_json::json!({ "job_id": "task-1", "patch": { "enabled": false } });
        pin_cron_delivery_to_origin(
            "cron_update",
            &mut omitted,
            "telegram",
            Some("work"),
            Some("chat-42"),
        );
        assert_eq!(omitted["patch"], serde_json::json!({ "enabled": false }));
    }

    #[test]
    fn cron_add_keeps_explicit_silent_delivery_silent() {
        let mut args = serde_json::json!({
            "job_type": "shell",
            "delivery": { "mode": "none", "channel": "discord.ops", "to": "foreign" }
        });

        pin_cron_delivery_to_origin(
            "cron_add",
            &mut args,
            "telegram",
            Some("work"),
            Some("chat-42"),
        );

        assert_eq!(
            args["delivery"],
            serde_json::json!({ "mode": "none", "channel": "discord.ops", "to": "foreign" })
        );
    }

    #[test]
    fn unaliased_channel_still_defaults_to_the_bare_type() {
        // CLI and daemon turns have no instance behind them.
        let mut args = serde_json::json!({
            "job_type": "agent",
            "prompt": "check the inbox",
        });

        maybe_inject_channel_delivery_defaults(
            "cron_add",
            &mut args,
            "telegram",
            Some("   "),
            Some("chat-42"),
        );

        assert_eq!(args["delivery"]["channel"], "telegram");
    }

    #[test]
    fn ask_user_gets_originating_rpc_channel_when_omitted() {
        // Regression: without this, ask_user picks channels.iter().next()
        // (HashMap order) and can hang on an external channel while the user
        // is waiting in the RPC/Code session that issued the turn.
        let mut args = serde_json::json!({
            "question": "Pick one",
            "choices": ["A", "B"],
        });
        maybe_inject_originating_channel("ask_user", &mut args, "rpc");
        assert_eq!(args["channel"], "rpc");
    }

    #[test]
    fn ask_user_does_not_override_explicit_channel() {
        let mut args = serde_json::json!({
            "question": "Pick one",
            "channel": "telegram.default",
        });
        maybe_inject_originating_channel("ask_user", &mut args, "rpc");
        assert_eq!(args["channel"], "telegram.default");
    }

    #[test]
    fn ask_user_skips_generic_agent_channel_name() {
        let mut args = serde_json::json!({ "question": "hi" });
        maybe_inject_originating_channel("ask_user", &mut args, "agent");
        assert!(args.get("channel").is_none());
    }

    #[test]
    fn poll_and_escalate_also_get_originating_channel() {
        for tool in ["poll", "escalate_to_human"] {
            let mut args = serde_json::json!({ "question": "q" });
            maybe_inject_originating_channel(tool, &mut args, "acp");
            assert_eq!(args["channel"], "acp", "tool={tool}");
        }
    }

    #[test]
    fn injected_channel_matches_escalate_tool_schema_parameter() {
        // Guard against the injection being a no-op: `escalate_to_human` must
        // actually declare the `channel` parameter we inject, otherwise the
        // helper "passes" while the tool ignores the key and falls back to
        // channels.iter().next(). Behavioural coverage of the routing itself
        // lives in zeroclaw-tools::escalate tests.
        let tool = zeroclaw_tools::escalate::EscalateToHumanTool::new(
            std::sync::Arc::new(zeroclaw_config::policy::SecurityPolicy::default()),
            vec![],
            std::sync::Arc::new(parking_lot::RwLock::new(std::collections::HashMap::new())),
        );
        let schema = zeroclaw_api::tool::Tool::parameters_schema(&tool);
        assert!(
            schema["properties"]["channel"].is_object(),
            "escalate_to_human must declare the injected `channel` parameter; schema: {schema}"
        );
    }
}
