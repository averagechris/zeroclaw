use super::*;
use crate::telegram_memberships::MembershipStore;
use zeroclaw_api::channel::ChannelMessage;
use zeroclaw_config::schema::{
    AliasedAgentConfig, ModelProviderConfig, OpenAIModelProviderConfig, RiskProfileConfig,
    TelegramConfig, TelegramInvitationsConfig,
};
use zeroclaw_runtime::observability::NoopObserver;

fn config(root: &Path) -> Config {
    let mut config = Config {
        data_dir: root.join("state"),
        config_path: root.join("config.toml"),
        ..Default::default()
    };
    config.providers.models.openai.insert(
        "test".into(),
        OpenAIModelProviderConfig {
            base: ModelProviderConfig {
                model: Some("test-model".into()),
                api_key: Some("synthetic-test-key".into()),
                ..Default::default()
            },
        },
    );
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
    for alias in ["guest", "group"] {
        config.agents.insert(
            alias.into(),
            AliasedAgentConfig {
                model_provider: "openai.test".into(),
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
    config
}

fn message(chat: i64, sender: i64) -> ChannelMessage {
    let mut msg = ChannelMessage::new(
        "test-message",
        "test-sender",
        chat.to_string(),
        "hello",
        "telegram",
        1,
    );
    msg.channel_alias = Some("home".into());
    msg.platform_sender_id = Some(sender.to_string());
    msg
}

fn router(config: Config, store: Arc<MembershipStore>) -> AgentRouter {
    let runtime = Arc::from(platform::create_runtime(&config.runtime).unwrap());
    let config_arc = Arc::new(RwLock::new(config.clone()));
    AgentRouter::multi(HashMap::new(), HashMap::new(), None, None)
        .with_telegram_routes(&config)
        .with_dynamic_agents(
            config_arc,
            Arc::new(NoopObserver),
            runtime,
            None,
            Arc::new(HashMap::new()),
            None,
            HashMap::from([("home".into(), store)]),
        )
}

#[tokio::test]
async fn telegram_dynamic_router_builds_once_and_revokes_cached_and_queued_contexts() {
    let root = tempfile::TempDir::new().unwrap();
    let cfg = config(root.path());
    let store = Arc::new(MembershipStore::open(&cfg.data_dir, "home").unwrap());
    let invite = store.create_invite(1).unwrap();
    store.redeem(&invite, 2001, 2).unwrap();
    store.activate_group(-3001, 2).unwrap();
    let router = router(cfg, store.clone());
    let msg = message(2001, 2001);
    let ctx = router.resolve_async(&msg).await.unwrap().unwrap();
    let again = router.resolve_async(&msg).await.unwrap().unwrap();
    assert!(Arc::ptr_eq(&ctx, &again));
    let group = router
        .resolve_async(&message(-3001, 2001))
        .await
        .unwrap()
        .unwrap();
    assert_ne!(ctx.agent_alias, group.agent_alias);
    assert!(
        router
            .resolve_async(&message(2001, 2002))
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        router
            .resolve_async(&message(-3002, 2001))
            .await
            .unwrap()
            .is_none()
    );
    assert!(queued_dynamic_membership_is_current(
        router.dynamic_agents.as_deref(),
        &ctx,
        &msg
    ));
    store.revoke(2001).unwrap();
    assert!(router.resolve_async(&msg).await.unwrap().is_none());
    assert!(!queued_dynamic_membership_is_current(
        router.dynamic_agents.as_deref(),
        &ctx,
        &msg
    ));
    assert!(
        router
            .resolve_async(&message(-3001, 2001))
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn telegram_dynamic_reload_restores_identity_and_live_policy_can_close_cached_access() {
    let root = tempfile::TempDir::new().unwrap();
    let cfg = config(root.path());
    let store = Arc::new(MembershipStore::open(&cfg.data_dir, "home").unwrap());
    let member = store
        .redeem(&store.create_invite(1).unwrap(), 2001, 2)
        .unwrap();
    std::fs::write(&cfg.config_path, toml::to_string(&cfg).unwrap()).unwrap();
    let alias = member.agent_alias("home");
    let (restored, defaults) = load_runtime_config_and_defaults_for_data_dir(
        &cfg.config_path,
        &alias,
        Some(&cfg.data_dir),
    )
    .await
    .unwrap();
    assert!(restored.agents.contains_key(&alias));
    assert_eq!(restored.data_dir, cfg.data_dir);
    assert_eq!(defaults.model, "test-model");
    let router = router(cfg, store);
    let msg = message(2001, 2001);
    let ctx = router.resolve_async(&msg).await.unwrap().unwrap();
    let dynamic = router.dynamic_agents.as_ref().unwrap();
    dynamic
        .config_arc
        .write()
        .risk_profiles
        .get_mut("guests")
        .unwrap()
        .allowed_tools
        .retain(|tool| tool != "memory_forget");
    assert!(!queued_dynamic_membership_is_current(
        Some(dynamic),
        &ctx,
        &msg
    ));
    let narrowed = router.resolve_async(&msg).await.unwrap().unwrap();
    assert!(!Arc::ptr_eq(&ctx, &narrowed));
    assert!(queued_dynamic_membership_is_current(
        Some(dynamic),
        &narrowed,
        &msg
    ));
    dynamic
        .config_arc
        .write()
        .risk_profiles
        .get_mut("guests")
        .unwrap()
        .allowed_tools
        .push("shell".into());
    assert!(router.resolve_async(&msg).await.unwrap().is_none());
    assert!(!queued_dynamic_membership_is_current(
        Some(dynamic),
        &ctx,
        &msg
    ));
}
