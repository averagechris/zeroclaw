//! Deterministic Telegram enrollment controls, handled before agent dispatch.
use super::{TelegramChannel, i18n};
use crate::telegram_memberships::MembershipKind;
use zeroclaw_api::channel::{Channel, SendMessage};
use zeroclaw_config::schema::TelegramInvitationsConfig;

impl TelegramChannel {
    fn invitation_policy(&self) -> Option<TelegramInvitationsConfig> {
        self.invitations.as_ref()?;
        let config = self.persist.as_ref()?.read();
        let telegram = config.channels.telegram.get(&self.alias)?;
        if !telegram.enabled
            || config
                .validate_telegram_invitations(&self.alias, telegram)
                .is_err()
        {
            return None;
        }
        telegram.invitations.clone()
    }

    fn static_invitation_route(&self, chat_id: i64) -> bool {
        self.persist.as_ref().is_some_and(|config| {
            config
                .read()
                .channels
                .telegram
                .get(&self.alias)
                .and_then(|telegram| telegram.routes.as_ref())
                .is_some_and(|routes| routes.contains_key(&chat_id.to_string()))
        })
    }

    fn message_peer_policy(&self, message: &serde_json::Value) -> (bool, bool) {
        let identities: Vec<String> = Self::authorization_identities(message)
            .iter()
            .map(|identity| Self::normalize_identity(identity))
            .collect();
        let refs: Vec<&str> = identities.iter().map(String::as_str).collect();
        let peers: Vec<String> = (self.peer_resolver)()
            .iter()
            .map(|peer| Self::normalize_identity(peer))
            .collect();
        (
            crate::allowlist::is_identity_denied_by(&peers, &refs, |a, b| a == b),
            crate::allowlist::is_identity_allowed(
                &peers,
                &refs,
                crate::allowlist::Match::Sensitive,
            ),
        )
    }

    fn message_sender_is_denied(&self, message: &serde_json::Value) -> bool {
        self.message_peer_policy(message).0
    }

    fn invitation_actor(message: &serde_json::Value) -> Option<i64> {
        if message.get("sender_chat").is_some()
            || message
                .pointer("/from/is_bot")
                .and_then(serde_json::Value::as_bool)
                != Some(false)
        {
            return None;
        }
        message
            .pointer("/from/id")
            .and_then(serde_json::Value::as_i64)
            .filter(|id| *id > 0)
    }

    fn invitation_private_chat(message: &serde_json::Value, actor: i64) -> bool {
        message
            .pointer("/chat/type")
            .and_then(serde_json::Value::as_str)
            == Some("private")
            && message
                .pointer("/chat/id")
                .and_then(serde_json::Value::as_i64)
                == Some(actor)
    }

    fn invitation_group_chat(message: &serde_json::Value) -> Option<i64> {
        if !matches!(
            message
                .pointer("/chat/type")
                .and_then(serde_json::Value::as_str),
            Some("group" | "supergroup")
        ) {
            return None;
        }
        message
            .pointer("/chat/id")
            .and_then(serde_json::Value::as_i64)
            .filter(|id| *id < 0)
    }

    pub(super) fn message_sender_is_allowed(&self, message: &serde_json::Value) -> bool {
        let (denied, configured) = self.message_peer_policy(message);
        if denied {
            return false;
        }
        if self.invitations.is_none() {
            return configured;
        }
        let Some(chat_id) = message
            .pointer("/chat/id")
            .and_then(serde_json::Value::as_i64)
        else {
            return false;
        };
        if self.static_invitation_route(chat_id) {
            return configured;
        }
        if self.invitation_policy().is_none() {
            return false;
        }
        let Some(actor) = Self::invitation_actor(message) else {
            return false;
        };
        let chat_id = if Self::invitation_private_chat(message, actor) {
            actor
        } else if let Some(chat_id) = Self::invitation_group_chat(message) {
            chat_id
        } else {
            return false;
        };
        if self.static_invitation_route(chat_id) {
            return false;
        }
        self.invitations.as_ref().is_some_and(|store| {
            store
                .membership(chat_id)
                .ok()
                .flatten()
                .is_some_and(|membership| {
                    matches!(
                        (membership.kind, chat_id > 0),
                        (MembershipKind::Private, true) | (MembershipKind::Group, false)
                    )
                })
        })
    }

    pub(super) fn approval_callback_sender_is_allowed(&self, callback: &serde_json::Value) -> bool {
        let Some(message) = callback.get("message") else {
            return false;
        };
        let Some(from) = callback.get("from") else {
            return false;
        };
        let mut scoped = message.clone();
        scoped["from"] = from.clone();
        if let Some(message) = scoped.as_object_mut() {
            message.remove("sender_chat");
        }
        self.message_sender_is_allowed(&scoped)
    }

    fn telegram_command_parts(text: &str) -> (&str, Option<&str>, &str) {
        let text = text.trim();
        let (head, args) = text
            .split_once(char::is_whitespace)
            .map_or((text, ""), |(head, args)| (head, args.trim()));
        let (command, target) = head
            .split_once('@')
            .map_or((head, None), |(command, target)| (command, Some(target)));
        (command, target, args)
    }

    pub(super) fn invitation_command(text: &str) -> Option<(&str, Option<&str>, &str)> {
        let parts = Self::telegram_command_parts(text);
        matches!(
            parts.0,
            "/invite" | "/start" | "/activate" | "/guests" | "/revoke"
        )
        .then_some(parts)
    }

    pub(super) fn is_dynamic_settings_command(
        &self,
        message: &serde_json::Value,
        text: &str,
    ) -> bool {
        self.invitations.is_some()
            && matches!(
                Self::telegram_command_parts(text).0,
                "/model" | "/models" | "/config"
            )
            && message
                .pointer("/chat/id")
                .and_then(serde_json::Value::as_i64)
                .is_some_and(|id| !self.static_invitation_route(id))
            && self.message_sender_is_allowed(message)
    }

    async fn send_invitation_text(&self, text: String, recipient: &str) {
        let mut message = SendMessage::new(text, recipient);
        message.suppress_voice = true;
        // The notice never includes database errors or credentials. Delivery is
        // best-effort after durable enrollment, so a retry cannot repeat it.
        if !matches!(
            tokio::time::timeout(std::time::Duration::from_secs(10), self.send(&message)).await,
            Ok(Ok(()))
        ) {
            ::zeroclaw_log::record!(
                WARN,
                ::zeroclaw_log::Event::new(module_path!(), ::zeroclaw_log::Action::Fail)
                    .with_outcome(::zeroclaw_log::EventOutcome::Failure),
                "Telegram invitation notice delivery failed"
            );
        }
    }

    pub(super) async fn send_invitation_notice(&self, key: &str, recipient: &str) {
        self.send_invitation_text(i18n::get_required_cli_string(key), recipient)
            .await;
    }

    async fn invitation_bot_username(&self) -> Option<String> {
        tokio::time::timeout(std::time::Duration::from_secs(10), self.get_bot_username())
            .await
            .ok()
            .flatten()
    }

    pub(super) async fn handle_invitation_command(&self, update: &serde_json::Value) -> bool {
        let Some(store) = self.invitations.as_ref() else {
            return false;
        };
        let Some(message) = update
            .get("message")
            .or_else(|| update.get("edited_message"))
        else {
            return false;
        };
        let Some(text) = message.get("text").and_then(serde_json::Value::as_str) else {
            return false;
        };
        if self.is_dynamic_settings_command(message, text) {
            let (_, target, _) = Self::telegram_command_parts(text);
            if let Some(target) = target {
                let Some(username) = self.invitation_bot_username().await else {
                    return true;
                };
                if !target.eq_ignore_ascii_case(&username) {
                    return true;
                }
            }
            if self.message_sender_is_allowed(message)
                && let Some(chat_id) = message
                    .pointer("/chat/id")
                    .and_then(serde_json::Value::as_i64)
            {
                let recipient = match Self::topic_thread_id(message) {
                    Some(thread) => format!("{chat_id}:{thread}"),
                    None => chat_id.to_string(),
                };
                self.send_invitation_notice(
                    "channel-telegram-invitation-settings-restricted",
                    &recipient,
                )
                .await;
            }
            return true;
        }
        let Some((command, target, args)) = Self::invitation_command(text) else {
            return false;
        };
        let Some(chat_id) = message
            .pointer("/chat/id")
            .and_then(serde_json::Value::as_i64)
        else {
            return true;
        };
        if let Some(target) = target {
            let Some(username) = self.invitation_bot_username().await else {
                return true;
            };
            if !target.eq_ignore_ascii_case(&username) {
                return true;
            }
        }
        let recipient = match Self::topic_thread_id(message) {
            Some(thread) => format!("{chat_id}:{thread}"),
            None => chat_id.to_string(),
        };
        // Re-read policy after any getMe await: ownership may have changed.
        let Some(policy) = self.invitation_policy() else {
            return true;
        };
        let forwarded = [
            "forward_origin",
            "forward_from",
            "forward_from_chat",
            "forward_sender_name",
            "forward_date",
        ]
        .iter()
        .any(|field| message.get(field).is_some());
        if update.get("edited_message").is_some()
            || message.get("edit_date").is_some()
            || forwarded
            || self.message_sender_is_denied(message)
        {
            self.send_invitation_notice("channel-telegram-invitation-rejected", &recipient)
                .await;
            return true;
        }
        let Some(actor) = Self::invitation_actor(message) else {
            return true;
        };
        let owner = policy.owner_id.parse::<i64>().ok() == Some(actor);
        let private = Self::invitation_private_chat(message, actor);
        let now = chrono::Utc::now().timestamp();
        let key = match command {
            "/start" if private && !args.is_empty() && !args.contains(char::is_whitespace) => {
                if self.static_invitation_route(chat_id) {
                    "channel-telegram-invitation-static-route"
                } else if store.redeem(args, actor, now).is_ok() {
                    "channel-telegram-invitation-welcome"
                } else {
                    "channel-telegram-invitation-invalid"
                }
            }
            "/invite" if owner && private && args.is_empty() => {
                let Some(username) = self.invitation_bot_username().await else {
                    self.send_invitation_notice(
                        "channel-telegram-invitation-unavailable",
                        &recipient,
                    )
                    .await;
                    return true;
                };
                // Provider I/O above must not retain an old owner grant.
                if self
                    .invitation_policy()
                    .is_none_or(|current| current.owner_id != actor.to_string())
                    || self.message_sender_is_denied(message)
                {
                    return true;
                }
                match store.create_invite(chrono::Utc::now().timestamp()) {
                    Ok(token) => {
                        let link = format!("https://t.me/{username}?start={token}");
                        let text = i18n::get_required_cli_string_with_args(
                            "channel-telegram-invitation-created",
                            &[("link", link.as_str())],
                        );
                        self.send_invitation_text(text, &recipient).await;
                        return true;
                    }
                    Err(_) => "channel-telegram-invitation-unavailable",
                }
            }
            "/activate" if owner && args.is_empty() => {
                if let Some(group_id) = Self::invitation_group_chat(message) {
                    if self.static_invitation_route(group_id) {
                        "channel-telegram-invitation-static-route"
                    } else if store.activate_group(group_id, now).is_ok() {
                        "channel-telegram-invitation-group-active"
                    } else {
                        "channel-telegram-invitation-unavailable"
                    }
                } else {
                    "channel-telegram-invitation-group-only"
                }
            }
            "/guests" if owner && private && args.is_empty() => match store.list() {
                Ok(memberships) if memberships.is_empty() => {
                    "channel-telegram-invitation-no-guests"
                }
                Ok(memberships) => {
                    let entries = memberships
                        .iter()
                        .map(|membership| membership.chat_id.to_string())
                        .collect::<Vec<_>>()
                        .join("\n");
                    let text = i18n::get_required_cli_string_with_args(
                        "channel-telegram-invitation-guests",
                        &[("entries", entries.as_str())],
                    );
                    self.send_invitation_text(text, &recipient).await;
                    return true;
                }
                Err(_) => "channel-telegram-invitation-unavailable",
            },
            "/revoke" if owner && private => {
                match args
                    .parse::<i64>()
                    .ok()
                    .filter(|id| *id != 0 && id.to_string() == args)
                {
                    Some(target) if self.static_invitation_route(target) => {
                        "channel-telegram-invitation-static-route"
                    }
                    Some(target) => match store.revoke(target) {
                        Ok(true) => "channel-telegram-invitation-revoked",
                        Ok(false) => "channel-telegram-invitation-no-membership",
                        Err(_) => "channel-telegram-invitation-unavailable",
                    },
                    None => "channel-telegram-invitation-revoke-usage",
                }
            }
            "/start" => "channel-telegram-invitation-required",
            _ => "channel-telegram-invitation-owner-only",
        };
        self.send_invitation_notice(key, &recipient).await;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::telegram::UpdateOutcome;
    use crate::telegram_memberships::MembershipStore;
    use parking_lot::RwLock;
    use std::sync::Arc;
    use zeroclaw_config::schema::{AliasedAgentConfig, Config, RiskProfileConfig, TelegramConfig};

    type Fixture = (
        tempfile::TempDir,
        Arc<MembershipStore>,
        Arc<RwLock<Config>>,
        TelegramChannel,
    );

    async fn fixture(server: &wiremock::MockServer) -> Fixture {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, ResponseTemplate};
        Mock::given(method("POST"))
            .and(path("/botfake/sendMessage"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"ok": true})))
            .mount(server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(MembershipStore::open(dir.path(), "home").unwrap());
        let mut config = Config::default();
        config.risk_profiles.insert(
            "guests".into(),
            RiskProfileConfig {
                allowed_tools: vec![
                    "memory_recall".into(),
                    "memory_store".into(),
                    "memory_forget".into(),
                ],
                ..Default::default()
            },
        );
        for template in ["guest", "group"] {
            config.agents.insert(
                template.into(),
                AliasedAgentConfig {
                    risk_profile: "guests".into(),
                    ..Default::default()
                },
            );
        }
        config.channels.telegram.insert(
            "home".into(),
            TelegramConfig {
                enabled: true,
                per_user_session: false,
                invitations: Some(TelegramInvitationsConfig {
                    owner_id: "1001".into(),
                    guest_agent: "guest".into(),
                    group_agent: "group".into(),
                }),
                ..Default::default()
            },
        );
        let live = Arc::new(RwLock::new(config));
        let channel = TelegramChannel::new(
            "fake".into(),
            "home",
            Arc::new(|| vec!["1001".into(), "!blocked".into()]),
            false,
        )
        .with_persistence(Arc::clone(&live))
        .with_invitations(Arc::clone(&store))
        .with_api_base(server.uri())
        .with_per_user_session(false)
        .with_ack_reactions(false);
        *channel.bot_username.lock() = Some("testbot".into());
        (dir, store, live, channel)
    }

    fn update(id: i64, actor: i64, chat: i64, text: &str) -> serde_json::Value {
        serde_json::json!({"update_id": id, "message": {
            "message_id": id, "from": {"id": actor, "is_bot": false},
            "chat": {"id": chat, "type": if chat > 0 { "private" } else { "supergroup" }},
            "text": text,
        }})
    }

    async fn process(
        channel: &TelegramChannel,
        update: &serde_json::Value,
        tx: &tokio::sync::mpsc::Sender<zeroclaw_api::channel::ChannelMessage>,
    ) {
        assert!(matches!(
            channel.process_update(update, tx, &mut None).await,
            UpdateOutcome::Advanced
        ));
    }

    #[tokio::test]
    async fn invitation_controls_redeem_and_revoke_without_agent_dispatch() {
        let server = wiremock::MockServer::start().await;
        let (_dir, store, _live, channel) = fixture(&server).await;
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        process(&channel, &update(1, 1001, 1001, "/invite"), &tx).await;
        assert!(rx.try_recv().is_err());
        let requests = server.received_requests().await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
        let token = body["text"]
            .as_str()
            .unwrap()
            .split("?start=")
            .nth(1)
            .unwrap()
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
            .collect::<String>();
        assert!(!token.is_empty());
        process(
            &channel,
            &update(2, 23, 23, &format!("/start {token}")),
            &tx,
        )
        .await;
        assert!(store.membership(23).unwrap().is_some());
        assert!(rx.try_recv().is_err());
        process(&channel, &update(3, 23, 23, "hello"), &tx).await;
        assert_eq!(rx.try_recv().unwrap().content, "hello");
        process(
            &channel,
            &update(4, 24, 24, &format!("/start {token}")),
            &tx,
        )
        .await;
        assert!(store.membership(24).unwrap().is_none());
        process(&channel, &update(5, 1001, 1001, "/guests"), &tx).await;
        process(&channel, &update(6, 1001, 1001, "/revoke 23"), &tx).await;
        assert!(store.membership(23).unwrap().is_none());
        process(&channel, &update(7, 23, 23, "after revoke"), &tx).await;
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn group_activation_is_owner_only_and_admission_stays_chat_scoped() {
        let server = wiremock::MockServer::start().await;
        let (_dir, store, _live, channel) = fixture(&server).await;
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        process(&channel, &update(1, 23, -42, "/activate"), &tx).await;
        assert!(store.membership(-42).unwrap().is_none());
        process(&channel, &update(2, 1001, -42, "/activate@testbot"), &tx).await;
        assert!(store.membership(-42).unwrap().is_some());
        process(&channel, &update(3, 23, -42, "group hello"), &tx).await;
        let message = rx.try_recv().unwrap();
        assert_eq!(message.reply_target, "-42");
        assert_eq!(
            message.conversation_scope,
            zeroclaw_api::channel::ChannelConversationScope::ReplyTarget
        );
        assert!(
            channel
                .parse_update_message(&update(4, 23, 23, "DM"))
                .is_none()
        );
        assert!(
            channel
                .parse_update_message(&update(5, 23, -43, "other group"))
                .is_none()
        );
        let mut denied = update(6, 23, -42, "denied");
        denied["message"]["from"]["username"] = "blocked".into();
        assert!(channel.parse_update_message(&denied).is_none());
        let mut anonymous = update(7, 1001, -42, "anonymous");
        anonymous["message"]["from"]["id"] = 1087968824_i64.into();
        anonymous["message"]["sender_chat"] = serde_json::json!({"id": -42});
        assert!(channel.parse_update_message(&anonymous).is_none());
        let mut bot = update(8, 23, -42, "bot");
        bot["message"]["from"]["is_bot"] = true.into();
        assert!(channel.parse_update_message(&bot).is_none());
        let callback = serde_json::json!({"from": {"id": 23, "is_bot": false},
            "message": {"chat": {"id": -42, "type": "supergroup"}}});
        assert!(channel.approval_callback_sender_is_allowed(&callback));
        let mut other = callback.clone();
        other["message"]["chat"]["id"] = (-43).into();
        assert!(!channel.approval_callback_sender_is_allowed(&other));
        store.revoke(-42).unwrap();
        assert!(!channel.approval_callback_sender_is_allowed(&callback));
    }

    #[tokio::test]
    async fn queued_controls_reject_forwarded_edited_anonymous_and_wrong_bot() {
        let server = wiremock::MockServer::start().await;
        let (_dir, store, _live, channel) = fixture(&server).await;
        let token = store.create_invite(chrono::Utc::now().timestamp()).unwrap();
        let mut forwarded = update(1, 23, 23, &format!("/start {token}"));
        forwarded["message"]["forward_origin"] = serde_json::json!({"type": "user"});
        let mut edited = update(2, 1001, -42, "/activate");
        edited["edited_message"] = edited["message"].clone();
        edited.as_object_mut().unwrap().remove("message");
        let mut anonymous = update(3, 1001, -42, "/activate");
        anonymous["message"]["sender_chat"] = serde_json::json!({"id": -42});
        let mut queue = std::collections::VecDeque::new();
        let mut pending = std::collections::HashMap::new();
        let now = std::time::Instant::now();
        TelegramChannel::enqueue_update_batch(
            &mut queue,
            &mut pending,
            &[
                forwarded,
                edited,
                anonymous,
                update(4, 1001, -42, "/activate@otherbot"),
            ],
            now,
            1,
        );
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        let mut offset = 0;
        assert!(matches!(
            channel
                .process_queued_updates(
                    &tx,
                    &mut queue,
                    &mut pending,
                    &mut offset,
                    &mut None,
                    now,
                    1
                )
                .await,
            UpdateOutcome::Advanced
        ));
        assert_eq!(offset, 5);
        assert!(rx.try_recv().is_err());
        assert!(store.list().unwrap().is_empty());
        store
            .redeem(&token, 23, chrono::Utc::now().timestamp())
            .unwrap();
    }

    #[tokio::test]
    async fn live_owner_and_disabled_policy_apply_to_existing_channel() {
        let server = wiremock::MockServer::start().await;
        let (_dir, store, live, channel) = fixture(&server).await;
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        live.write()
            .channels
            .telegram
            .get_mut("home")
            .unwrap()
            .invitations
            .as_mut()
            .unwrap()
            .owner_id = "2002".into();
        process(&channel, &update(1, 1001, -42, "/activate"), &tx).await;
        assert!(store.membership(-42).unwrap().is_none());
        process(&channel, &update(2, 2002, -42, "/activate"), &tx).await;
        assert!(store.membership(-42).unwrap().is_some());
        live.write()
            .channels
            .telegram
            .get_mut("home")
            .unwrap()
            .invitations = None;
        assert!(
            channel
                .parse_update_message(&update(3, 23, -42, "hello"))
                .is_none()
        );
        process(&channel, &update(4, 2002, 2002, "/invite"), &tx).await;
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn static_route_conflicts_leave_invitation_and_membership_unchanged() {
        let server = wiremock::MockServer::start().await;
        let (_dir, store, live, channel) = fixture(&server).await;
        live.write()
            .channels
            .telegram
            .get_mut("home")
            .unwrap()
            .routes = Some(
            [
                ("23".into(), "owner".into()),
                ("-42".into(), "owner".into()),
            ]
            .into(),
        );
        let token = store.create_invite(chrono::Utc::now().timestamp()).unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        process(
            &channel,
            &update(1, 23, 23, &format!("/start {token}")),
            &tx,
        )
        .await;
        process(&channel, &update(2, 1001, -42, "/activate"), &tx).await;
        process(&channel, &update(3, 1001, 1001, "/revoke 23"), &tx).await;
        assert!(store.list().unwrap().is_empty());
        assert!(rx.try_recv().is_err());
        store
            .redeem(&token, 24, chrono::Utc::now().timestamp())
            .unwrap();
        assert!(
            live.read().channels.telegram["home"]
                .routes
                .as_ref()
                .unwrap()
                .contains_key("23")
        );
    }

    #[tokio::test]
    async fn failed_membership_reads_deny_access_and_do_not_dispatch_controls() {
        let server = wiremock::MockServer::start().await;
        let (dir, store, _live, channel) = fixture(&server).await;
        store
            .activate_group(-42, chrono::Utc::now().timestamp())
            .unwrap();
        let database =
            rusqlite::Connection::open(dir.path().join("telegram-memberships/home.sqlite3"))
                .unwrap();
        database
            .execute_batch("DROP TABLE telegram_memberships;")
            .unwrap();
        assert!(
            channel
                .parse_update_message(&update(1, 23, -42, "hello"))
                .is_none()
        );
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        process(&channel, &update(2, 1001, 1001, "/guests"), &tx).await;
        assert!(rx.try_recv().is_err());
        let requests = server.received_requests().await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
        assert!(
            !body["text"]
                .as_str()
                .unwrap()
                .contains("telegram_memberships")
        );
    }
    #[tokio::test]
    async fn group_approval_callback_cannot_resolve_another_groups_card() {
        use wiremock::matchers::{method, path_regex};
        use wiremock::{Mock, ResponseTemplate};
        let server = wiremock::MockServer::start().await;
        let (_dir, store, _live, channel) = fixture(&server).await;
        Mock::given(method("POST"))
            .and(path_regex(
                r"/botfake/(answerCallbackQuery|editMessageText|editMessageReplyMarkup)$",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"ok": true})))
            .mount(&server)
            .await;
        for group in [-42, -43] {
            store
                .activate_group(group, chrono::Utc::now().timestamp())
                .unwrap();
        }
        let (approval_tx, mut approval_rx) = tokio::sync::oneshot::channel();
        channel.pending_approvals.lock().await.insert(
            "card".into(),
            crate::util::PendingApproval {
                sender: approval_tx,
                destination: "-42".into(),
                tool_name: "memory_store".into(),
            },
        );
        let callback = |chat_id| {
            serde_json::json!({"update_id": 1, "callback_query": {
                "id": "tap", "from": {"id": 23, "is_bot": false},
                "message": {"message_id": 55, "chat": {"id": chat_id, "type": "supergroup"}},
                "data": "approval:card:approve",
            }})
        };
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        process(&channel, &callback(-43), &tx).await;
        assert!(approval_rx.try_recv().is_err());
        assert!(channel.pending_approvals.lock().await.contains_key("card"));
        process(&channel, &callback(-42), &tx).await;
        assert_eq!(
            approval_rx.await.unwrap(),
            zeroclaw_api::channel::ChannelApprovalResponse::Approve
        );
        assert!(rx.try_recv().is_err());
    }
    #[tokio::test]
    async fn broad_configured_peers_cannot_bypass_dynamic_identity_or_settings_policy() {
        let server = wiremock::MockServer::start().await;
        let (_dir, store, live, mut channel) = fixture(&server).await;
        channel.peer_resolver = Arc::new(|| vec!["*".into()]);
        store
            .activate_group(-42, chrono::Utc::now().timestamp())
            .unwrap();
        let token = store.create_invite(chrono::Utc::now().timestamp()).unwrap();
        store
            .redeem(&token, 23, chrono::Utc::now().timestamp())
            .unwrap();
        assert!(
            channel
                .parse_update_message(&update(1, 24, 24, "uninvited"))
                .is_none()
        );
        assert!(
            channel
                .parse_update_message(&update(2, 24, 23, "wrong DM sender"))
                .is_none()
        );
        let mut bot = update(3, 23, -42, "bot");
        bot["message"]["from"]["is_bot"] = true.into();
        assert!(channel.parse_update_message(&bot).is_none());
        let mut anonymous = update(4, 23, -42, "anonymous");
        anonymous["message"]["sender_chat"] = serde_json::json!({"id": -42});
        assert!(channel.parse_update_message(&anonymous).is_none());
        let mut wrong_type = update(5, 23, -42, "wrong chat type");
        wrong_type["message"]["chat"]["type"] = "channel".into();
        assert!(channel.parse_update_message(&wrong_type).is_none());
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        for (id, command) in [
            (6, "/model expensive"),
            (7, "/models provider"),
            (8, "/config"),
            (9, "/model@testbot --agent expensive"),
            (10, "/model@otherbot expensive"),
        ] {
            process(&channel, &update(id, 23, 23, command), &tx).await;
            assert!(rx.try_recv().is_err());
        }
        process(
            &channel,
            &update(11, 23, 23, "/modeling is interesting"),
            &tx,
        )
        .await;
        assert_eq!(rx.try_recv().unwrap().content, "/modeling is interesting");
        live.write()
            .channels
            .telegram
            .get_mut("home")
            .unwrap()
            .routes = Some([("1001".into(), "owner".into())].into());
        process(&channel, &update(12, 1001, 1001, "/model configured"), &tx).await;
        assert_eq!(rx.try_recv().unwrap().content, "/model configured");
        store.revoke(-42).unwrap();
        assert!(
            channel
                .parse_update_message(&update(13, 23, -42, "revoked"))
                .is_none()
        );
    }
}
