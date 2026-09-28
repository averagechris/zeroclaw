//! Isolation proof for one Telegram alias routed to several agents by exact
//! numeric chat ID (`channels.telegram.<alias>.routes`).
//!
//! These tests drive the production pieces at their boundary: messages come
//! from the real Telegram parser, the router is built from validated config,
//! and each agent's context uses the production memory factory, security
//! policy, and tool filter.

use super::*;

const OWNER: i64 = 1001;
const PARTNER: i64 = 1002;
const STRANGER: i64 = 1009;
const GROUP: i64 = -1003;
const OTHER_GROUP: i64 = -1004;
const MEMORY_TOOLS: [&str; 3] = ["memory_recall", "memory_store", "memory_forget"];
const OWNER_FACT: &str = "zebra fact 7741";
const OWNER_SESSION_NOTE: &str = "zebra note 5523";

/// The stage 1 layout: owner, partner, and household agents behind one routed
/// alias `home`, plus an unrelated agent `ops` that owns `telegram.work`, so
/// the bare `telegram` owner key exists and could leak through a fallback.
fn stage_one_config(root: &std::path::Path) -> Config {
    use zeroclaw_config::schema::{
        AliasedAgentConfig, AnthropicModelProviderConfig, RiskProfileConfig, TelegramConfig,
    };
    let mut config = Config {
        config_path: root.join("config.toml"),
        data_dir: root.join("data"),
        ..Config::default()
    };
    config.agents.clear();
    config.providers.models.anthropic.insert(
        "default".to_string(),
        AnthropicModelProviderConfig::default(),
    );
    let memory_tools: Vec<String> = MEMORY_TOOLS.iter().map(ToString::to_string).collect();
    for alias in ["owner", "partner", "household", "ops"] {
        let excluded_tools = if matches!(alias, "partner" | "household") {
            vec!["shell".to_string()]
        } else {
            Vec::new()
        };
        config.risk_profiles.insert(
            alias.to_string(),
            RiskProfileConfig {
                allowed_tools: memory_tools.clone(),
                excluded_tools,
                auto_approve: vec!["memory_recall".to_string(), "memory_store".to_string()],
                ..RiskProfileConfig::default()
            },
        );
        config.agents.insert(
            alias.to_string(),
            AliasedAgentConfig {
                enabled: true,
                model_provider: "anthropic.default".into(),
                risk_profile: alias.into(),
                channels: if alias == "ops" {
                    vec!["telegram.work".into()]
                } else {
                    Vec::new()
                },
                ..AliasedAgentConfig::default()
            },
        );
    }
    config.channels.telegram.insert(
        "home".to_string(),
        TelegramConfig {
            enabled: true,
            bot_token: "home-token".into(),
            per_user_session: false,
            routes: Some(HashMap::from([
                (OWNER.to_string(), "owner".to_string()),
                (PARTNER.to_string(), "partner".to_string()),
                (GROUP.to_string(), "household".to_string()),
            ])),
            ..TelegramConfig::default()
        },
    );
    config.channels.telegram.insert(
        "work".to_string(),
        TelegramConfig {
            enabled: true,
            bot_token: "work-token".into(),
            ..TelegramConfig::default()
        },
    );
    config.validate().expect("stage 1 config is valid");
    config
}

/// The real Telegram parser for alias `home`, admitting both humans and a
/// stranger so routing, not the allowlist, is what these tests exercise.
fn home_parser() -> crate::telegram::TelegramChannel {
    crate::telegram::TelegramChannel::new(
        "home-token".into(),
        "home",
        Arc::new(|| vec![OWNER.to_string(), PARTNER.to_string(), STRANGER.to_string()]),
        false,
    )
    .with_per_user_session(false)
    .with_text_only(true)
}

fn telegram_update(chat_id: i64, from_id: i64, text: &str) -> serde_json::Value {
    let chat_type = if chat_id > 0 { "private" } else { "supergroup" };
    serde_json::json!({
        "update_id": 1,
        "message": {
            "message_id": 7,
            "chat": { "id": chat_id, "type": chat_type },
            "from": { "id": from_id, "username": format!("user{from_id}") },
            "text": text,
        }
    })
}

fn parse(chat_id: i64, from_id: i64, text: &str) -> ChannelMessage {
    home_parser()
        .parse_update_message(&telegram_update(chat_id, from_id, text))
        .expect("authorized text parses")
}

fn named_ctx(alias: &str) -> Arc<ChannelRuntimeContext> {
    Arc::new(ChannelRuntimeContext {
        agent_alias: Arc::new(alias.to_string()),
        ..(*router_test_ctx()).clone()
    })
}

fn routed_router(config: &Config, ctxs: &[(&str, Arc<ChannelRuntimeContext>)]) -> AgentRouter {
    let enabled_agents = enabled_agent_aliases(config);
    let collected = vec!["telegram.home".to_string(), "telegram.work".to_string()];
    let owners = build_owner_by_channel_key(config, &enabled_agents, &collected);
    AgentRouter::multi(
        ctxs.iter()
            .map(|(alias, ctx)| ((*alias).to_string(), Arc::clone(ctx)))
            .collect(),
        owners,
        None,
        None,
    )
    .with_telegram_routes(config)
}

fn resolved_alias(router: &AgentRouter, msg: &ChannelMessage) -> Option<String> {
    router.resolve(msg).map(|ctx| ctx.agent_alias.to_string())
}

#[test]
fn routed_alias_dispatches_by_exact_numeric_ids_with_no_fallback() {
    let tmp = tempfile::TempDir::new().unwrap();
    let config = stage_one_config(tmp.path());
    let ctxs: Vec<(&str, Arc<ChannelRuntimeContext>)> = ["owner", "partner", "household", "ops"]
        .into_iter()
        .map(|alias| (alias, named_ctx(alias)))
        .collect();
    let router = routed_router(&config, &ctxs);
    let route = |msg: &ChannelMessage| resolved_alias(&router, msg);

    // Mapped chats reach exactly their agent.
    assert_eq!(route(&parse(OWNER, OWNER, "hi")).as_deref(), Some("owner"));
    assert_eq!(
        route(&parse(PARTNER, PARTNER, "hi")).as_deref(),
        Some("partner")
    );
    assert_eq!(
        route(&parse(GROUP, OWNER, "hi")).as_deref(),
        Some("household")
    );
    assert_eq!(
        route(&parse(GROUP, PARTNER, "hi")).as_deref(),
        Some("household")
    );

    // Unknown DMs and groups reach nobody, not the owner and not `ops`,
    // which owns the bare `telegram` key through `telegram.work`.
    assert_eq!(route(&parse(STRANGER, STRANGER, "hi")), None);
    assert_eq!(route(&parse(OTHER_GROUP, OWNER, "hi")), None);

    // Mixed or missing IDs never match a private route.
    let mut mixed = parse(OWNER, OWNER, "hi");
    mixed.platform_sender_id = Some(PARTNER.to_string());
    assert_eq!(route(&mixed), None);
    let mut missing = parse(OWNER, OWNER, "hi");
    missing.platform_sender_id = None;
    assert_eq!(route(&missing), None);
    let mut username_only = parse(OWNER, OWNER, "hi");
    username_only.platform_sender_id = Some(format!("user{OWNER}"));
    assert_eq!(route(&username_only), None);
    let mut user_id_as_group = parse(GROUP, OWNER, "hi");
    user_id_as_group.reply_target = (-OWNER).to_string();
    assert_eq!(route(&user_id_as_group), None);

    // A route to an agent without a live context is dropped, not redirected.
    let without_partner: Vec<_> = ctxs
        .iter()
        .filter(|(alias, _)| *alias != "partner")
        .cloned()
        .collect();
    let router = routed_router(&config, &without_partner);
    assert_eq!(
        resolved_alias(&router, &parse(PARTNER, PARTNER, "hi")),
        None
    );

    // Ordinary aliases keep their conventional binding.
    let mut work = parse(OWNER, OWNER, "hi");
    work.channel_alias = Some("work".to_string());
    assert_eq!(resolved_alias(&router, &work).as_deref(), Some("ops"));
}

#[test]
fn routed_alias_never_enters_single_owner_lookups() {
    let tmp = tempfile::TempDir::new().unwrap();
    let config = stage_one_config(tmp.path());
    let enabled_agents = enabled_agent_aliases(&config);
    let collected = vec!["telegram.home".to_string(), "telegram.work".to_string()];

    // Channel startup: the route table keeps the alias live.
    let active = ActiveChannelAliases::compute(&config);
    assert!(active.contains("telegram.home"));
    assert!(active.contains("telegram.work"));

    // The owner map, transcription, and TTS owner lookups have no owner.
    let owners = build_owner_by_channel_key(&config, &enabled_agents, &collected);
    assert!(!owners.contains_key("telegram.home"), "{owners:?}");
    assert_eq!(
        resolve_agent_transcription_provider(&config, "telegram.home"),
        ""
    );
    assert_eq!(config.agent_for_channel("telegram.home"), None);

    // Session hydration skips routed sessions even though the bare
    // `telegram` fallback names `ops`.
    assert_eq!(owners.get("telegram").map(String::as_str), Some("ops"));
    let routed_keys = config.telegram_routed_channel_keys();
    assert_eq!(
        startup_hydration_owner(&owners, &routed_keys, Some("telegram.home")),
        None
    );
    assert_eq!(
        startup_hydration_owner(&owners, &routed_keys, Some("telegram.work")).as_deref(),
        Some("ops")
    );

    // With no conventional bindings anywhere, the legacy "first enabled
    // agent owns every channel" fallback must not claim the routed alias.
    let mut legacy = config.clone();
    legacy.agents.remove("ops");
    legacy.channels.telegram.remove("work");
    let enabled_agents = enabled_agent_aliases(&legacy);
    let owners = build_owner_by_channel_key(&legacy, &enabled_agents, &collected);
    assert!(owners.is_empty(), "legacy fallback assigned {owners:?}");
    let active = ActiveChannelAliases::compute(&legacy);
    assert!(active.contains("telegram.home"));
    assert!(!active.contains("discord.unbound"));
}

#[test]
fn routed_alias_session_keys_are_distinct_and_the_group_is_shared() {
    let owner_dm = parse(OWNER, OWNER, "hi");
    let partner_dm = parse(PARTNER, PARTNER, "hi");
    let owner_in_group = parse(GROUP, OWNER, "hi");
    let partner_in_group = parse(GROUP, PARTNER, "hi");
    let ctx = router_test_ctx();
    let key = |msg: &ChannelMessage| runtime_conversation_history_key(ctx.as_ref(), msg);

    assert_eq!(key(&owner_in_group), key(&partner_in_group));
    let keys: HashSet<String> = [&owner_dm, &partner_dm, &owner_in_group]
        .into_iter()
        .map(key)
        .collect();
    assert_eq!(keys.len(), 3, "{keys:?}");
    // Scheduling stays personal inside the shared group session.
    assert_ne!(
        interruption_scope_key(&owner_in_group),
        interruption_scope_key(&partner_in_group)
    );
}

/// Scripted actions a [`RouteScriptProvider`] takes at the start of a turn.
#[derive(Clone, Copy)]
enum TurnAction {
    Reply,
    Store,
    Recall,
    Shell,
}

/// Records every model call and plays one scripted action per turn.
#[derive(Default)]
struct RouteScriptProvider {
    script: std::sync::Mutex<std::collections::VecDeque<TurnAction>>,
    calls: std::sync::Mutex<Vec<String>>,
}

impl RouteScriptProvider {
    fn with_script(script: &[TurnAction]) -> Arc<Self> {
        Arc::new(Self {
            script: std::sync::Mutex::new(script.iter().copied().collect()),
            calls: std::sync::Mutex::new(Vec::new()),
        })
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

fn tool_call(name: &str, arguments: serde_json::Value) -> String {
    format!(
        "<tool_call>\n{}\n</tool_call>",
        serde_json::json!({ "name": name, "arguments": arguments })
    )
}

#[async_trait::async_trait]
impl ModelProvider for RouteScriptProvider {
    async fn chat_with_system(
        &self,
        system_prompt: Option<&str>,
        message: &str,
        _model: &str,
        _temperature: Option<f64>,
    ) -> anyhow::Result<String> {
        self.calls
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(format!("{}\n{message}", system_prompt.unwrap_or_default()));
        Ok("ok".to_string())
    }

    async fn chat_with_history(
        &self,
        messages: &[ChatMessage],
        _model: &str,
        _temperature: Option<f64>,
    ) -> anyhow::Result<String> {
        let transcript = messages
            .iter()
            .map(|m| format!("{}: {}", m.role, m.content))
            .collect::<Vec<_>>()
            .join("\n");
        self.calls
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(transcript);
        let after_tools = messages
            .iter()
            .rev()
            .find(|m| m.role == "user")
            .is_some_and(|m| m.content.contains("[Tool results]"));
        if after_tools {
            return Ok("done".to_string());
        }
        let action = self
            .script
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .pop_front()
            .unwrap_or(TurnAction::Reply);
        Ok(match action {
            TurnAction::Reply => "reply".to_string(),
            TurnAction::Store => tool_call(
                "memory_store",
                serde_json::json!({ "key": "owner_zebra", "content": OWNER_FACT, "category": "core" }),
            ),
            TurnAction::Recall => {
                tool_call("memory_recall", serde_json::json!({ "query": "zebra" }))
            }
            TurnAction::Shell => tool_call("shell", serde_json::json!({ "command": "id" })),
        })
    }
}

impl ::zeroclaw_api::attribution::Attributable for RouteScriptProvider {
    fn role(&self) -> ::zeroclaw_api::attribution::Role {
        ::zeroclaw_api::attribution::Role::Provider(
            ::zeroclaw_api::attribution::ProviderKind::Model(
                ::zeroclaw_api::attribution::ModelProviderKind::Custom,
            ),
        )
    }
    fn alias(&self) -> &str {
        "RouteScriptProvider"
    }
}

/// A `shell` stand-in that counts executions. It is offered to every agent
/// so that only the agent's own policy can keep it out.
struct CountingShellTool(Arc<AtomicUsize>);

impl ::zeroclaw_api::attribution::Attributable for CountingShellTool {
    fn role(&self) -> ::zeroclaw_api::attribution::Role {
        ::zeroclaw_api::attribution::Role::Tool(::zeroclaw_api::attribution::ToolKind::Plugin)
    }
    fn alias(&self) -> &str {
        "shell"
    }
}

#[async_trait::async_trait]
impl Tool for CountingShellTool {
    fn name(&self) -> &str {
        "shell"
    }

    fn description(&self) -> &str {
        "counting shell"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({ "type": "object", "properties": { "command": { "type": "string" } } })
    }

    async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<ToolResult> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(ToolResult {
            success: true,
            output: ToolOutput::default(),
            error: None,
        })
    }
}

struct RoutedAgent {
    ctx: Arc<ChannelRuntimeContext>,
    provider: Arc<RouteScriptProvider>,
    shell_runs: Arc<AtomicUsize>,
    tool_names: Vec<String>,
}

/// An agent context assembled like `start_channels` does: production memory
/// factory, `SecurityPolicy::for_agent`, and the production tool filter over
/// the built-in memory tools plus a counting `shell`.
async fn routed_agent(
    config: &Config,
    alias: &str,
    channel: Arc<dyn Channel>,
    script: &[TurnAction],
) -> RoutedAgent {
    let risk_profile = config
        .risk_profile_for_agent(alias)
        .expect("agent has a risk profile")
        .clone();
    let security = Arc::new(SecurityPolicy::for_agent(config, alias).unwrap());
    let memory = zeroclaw_memory::create_memory_for_agent(config, alias, None)
        .await
        .unwrap();
    let shell_runs = Arc::new(AtomicUsize::new(0));
    let mut tools: Vec<Box<dyn Tool>> = vec![
        Box::new(zeroclaw_tools::memory_store::MemoryStoreTool::new(
            Arc::clone(&memory),
            Arc::clone(&security),
        )),
        Box::new(zeroclaw_tools::memory_recall::MemoryRecallTool::new(
            Arc::clone(&memory),
        )),
        Box::new(zeroclaw_tools::memory_forget::MemoryForgetTool::new(
            Arc::clone(&memory),
            Arc::clone(&security),
        )),
        Box::new(CountingShellTool(Arc::clone(&shell_runs))),
    ];
    apply_policy_tool_filter(&mut tools, Some(security.as_ref()), None);
    let tool_names = tools.iter().map(|t| t.name().to_string()).collect();
    let provider = RouteScriptProvider::with_script(script);
    let ctx = Arc::new(ChannelRuntimeContext {
        channels_by_name: Arc::new(HashMap::from([("telegram.home".to_string(), channel)])),
        model_provider: provider.clone(),
        agent_alias: Arc::new(alias.to_string()),
        agent_cfg: Arc::new(config.agents[alias].clone()),
        prompt_config: Arc::new(config.clone()),
        memory: Arc::clone(&memory),
        memory_strategy: Arc::new(
            zeroclaw_runtime::agent::memory_strategy::DefaultMemoryStrategy::with_config(
                Arc::clone(&memory),
                config.memory.clone(),
                config.data_dir.clone(),
            ),
        ),
        tools_registry: Arc::new(
            zeroclaw_runtime::tools::scoped::ScopedToolRegistry::from_raw_for_test(tools),
        ),
        auto_save_memory: config.memory.auto_save,
        workspace_dir: Arc::new(config.agent_workspace_dir(alias)),
        non_cli_excluded_tools: Arc::new(risk_profile.excluded_tools.clone()),
        autonomy_level: risk_profile.level,
        approval_manager: Arc::new(ApprovalManager::for_non_interactive(&risk_profile)),
        ..(*router_test_ctx()).clone()
    });
    RoutedAgent {
        ctx,
        provider,
        shell_runs,
        tool_names,
    }
}

async fn dispatch(router: &AgentRouter, chat_id: i64, from_id: i64, text: &str) {
    let msg = parse(chat_id, from_id, text);
    let ctx = router.resolve(&msg).expect("mapped chat resolves");
    Box::pin(process_channel_message(ctx, msg, CancellationToken::new())).await;
}

fn history_len(agent: &RoutedAgent, key: &str) -> usize {
    agent
        .ctx
        .conversation_histories
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .peek(key)
        .map_or(0, Vec::len)
}

/// Memory, shell, `/new`, and `/model` across the three routed agents.
#[tokio::test]
async fn routed_agents_keep_memory_tools_and_session_controls_apart() {
    use TurnAction::{Recall, Reply, Shell, Store};
    let tmp = tempfile::TempDir::new().unwrap();
    let config = stage_one_config(tmp.path());
    let channel: Arc<dyn Channel> = Arc::new(TelegramRecordingChannel::default());
    let owner = routed_agent(&config, "owner", Arc::clone(&channel), &[Store, Recall]).await;
    let partner = routed_agent(&config, "partner", Arc::clone(&channel), &[Recall, Shell]).await;
    let household = routed_agent(
        &config,
        "household",
        Arc::clone(&channel),
        &[Recall, Shell, Reply],
    )
    .await;
    let router = routed_router(
        &config,
        &[
            ("owner", Arc::clone(&owner.ctx)),
            ("partner", Arc::clone(&partner.ctx)),
            ("household", Arc::clone(&household.ctx)),
        ],
    );

    // Every stage 1 profile exposes exactly the memory tools.
    for agent in [&owner, &partner, &household] {
        let mut names = agent.tool_names.clone();
        names.sort();
        assert_eq!(names, ["memory_forget", "memory_recall", "memory_store"]);
    }

    // The owner stores a fact with `memory_store`. A second owner-scope entry
    // is bound to the owner's sender session, which both the owner's DM turns
    // and the owner's group turns recall against at turn start, so only the
    // agent scope can keep it out of the household agent's injection. (On
    // the default BM25-only recall path, session-scoped injection does not
    // include unsessioned `core` rows, so the tool-stored fact is checked
    // through `memory_recall`.)
    dispatch(&router, OWNER, OWNER, "remember the zebra").await;
    let owner_sender_session = sanitize_session_key(&parse(GROUP, OWNER, "x").sender);
    owner
        .ctx
        .memory
        .store(
            "owner_sender_note",
            OWNER_SESSION_NOTE,
            zeroclaw_memory::MemoryCategory::Core,
            Some(&owner_sender_session),
        )
        .await
        .unwrap();
    dispatch(&router, OWNER, OWNER, "zebra").await;
    let owner_calls = owner.provider.calls();
    let recall_turn_start = &owner_calls[owner_calls.len() - 2];
    assert!(
        recall_turn_start.contains("[Memory context]") && recall_turn_start.contains("5523"),
        "turn-start injection must surface the owner's own note: {recall_turn_start}"
    );
    assert!(
        owner_calls.last().unwrap().contains("7741"),
        "memory_recall must surface the owner's own fact"
    );

    // Neither other agent sees either entry, by injection or `memory_recall`.
    dispatch(&router, PARTNER, PARTNER, "zebra").await;
    dispatch(&router, GROUP, OWNER, "zebra").await;
    for (alias, agent) in [("partner", &partner), ("household", &household)] {
        let calls = agent.provider.calls();
        assert!(
            calls.iter().any(|c| c.contains("[Tool results]")),
            "{alias} ran memory_recall"
        );
        for call in &calls {
            assert!(
                !call.contains("7741"),
                "{alias} saw the owner's fact: {call}"
            );
            assert!(
                !call.contains("5523"),
                "{alias} saw the owner's note: {call}"
            );
            assert!(
                !call.contains("remember the zebra"),
                "{alias} saw the owner's message: {call}"
            );
        }
    }

    // Partner and household dispatches that call `shell` are denied.
    dispatch(&router, PARTNER, PARTNER, "run id").await;
    dispatch(&router, GROUP, PARTNER, "run id").await;
    for agent in [&owner, &partner, &household] {
        assert_eq!(agent.shell_runs.load(Ordering::SeqCst), 0);
    }
    // The shell call was attempted after the model saw the turn, and no
    // shell result ever reached any agent.
    for agent in [&partner, &household] {
        let calls = agent.provider.calls();
        assert!(calls.iter().any(|c| c.contains("run id")));
        assert!(
            calls
                .iter()
                .all(|c| !c.contains("<tool_result name=\"shell\""))
        );
    }
    // Control: the same call through a profile that grants `shell` runs it,
    // so the denial above comes from the partner and household profiles.
    let mut shell_config = config.clone();
    shell_config
        .risk_profiles
        .get_mut("owner")
        .unwrap()
        .allowed_tools
        .push("shell".to_string());
    let shell_owner = routed_agent(&shell_config, "owner", Arc::clone(&channel), &[Shell]).await;
    let shell_router = routed_router(&shell_config, &[("owner", Arc::clone(&shell_owner.ctx))]);
    dispatch(&shell_router, OWNER, OWNER, "run id").await;
    assert_eq!(shell_owner.shell_runs.load(Ordering::SeqCst), 1);

    // `/model` in the group applies to the group session only.
    let owner_dm_key =
        runtime_conversation_history_key(owner.ctx.as_ref(), &parse(OWNER, OWNER, "x"));
    let group_key =
        runtime_conversation_history_key(household.ctx.as_ref(), &parse(GROUP, OWNER, "x"));
    dispatch(&router, GROUP, PARTNER, "/model group-model").await;
    let group_route = household
        .ctx
        .route_overrides
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&group_key)
        .cloned()
        .expect("group model override stored on the group session");
    assert_eq!(group_route.model, "group-model");
    for agent in [&owner, &partner] {
        assert!(
            agent
                .ctx
                .route_overrides
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_empty(),
            "a group /model must not touch a DM agent"
        );
    }

    // `/new` in the group resets the shared group session only.
    assert!(history_len(&household, &group_key) > 0);
    let owner_dm_before = history_len(&owner, &owner_dm_key);
    assert!(owner_dm_before > 0);
    dispatch(&router, GROUP, OWNER, "/new").await;
    assert_eq!(history_len(&household, &group_key), 0);
    assert_eq!(history_len(&owner, &owner_dm_key), owner_dm_before);
}

/// Gate answers and model-picker selections from unmapped identities are
/// consumed or dropped before any agent runs.
#[tokio::test]
async fn unmapped_gate_and_picker_inputs_never_reach_an_agent() {
    let tmp = tempfile::TempDir::new().unwrap();
    let config = stage_one_config(tmp.path());
    let channel: Arc<dyn Channel> = Arc::new(TelegramRecordingChannel::default());
    let agents = [
        routed_agent(&config, "owner", Arc::clone(&channel), &[]).await,
        routed_agent(&config, "partner", Arc::clone(&channel), &[]).await,
        routed_agent(&config, "household", Arc::clone(&channel), &[]).await,
    ];
    let router = routed_router(
        &config,
        &[
            ("owner", Arc::clone(&agents[0].ctx)),
            ("partner", Arc::clone(&agents[1].ctx)),
            ("household", Arc::clone(&agents[2].ctx)),
        ],
    );

    let mut inputs = vec![
        parse(STRANGER, STRANGER, "approve run-1"),
        parse(OTHER_GROUP, OWNER, "approve run-1"),
        parse(STRANGER, STRANGER, "/model fast"),
        parse(STRANGER, STRANGER, "hello"),
    ];
    let mut marker = parse(STRANGER, STRANGER, "");
    marker.internal_sop_event = Some("sop.gate:approve:run-1".to_string());
    inputs.push(marker);
    // A picker selection re-enters as `/model <hint>` with the requesting
    // user's ID; mixed chat and sender IDs must not resolve.
    let mut picker = parse(OWNER, OWNER, "/model fast");
    picker.id = "telegram_model_picker_test".to_string();
    picker.platform_sender_id = Some(STRANGER.to_string());
    inputs.push(picker);

    let (tx, rx) = tokio::sync::mpsc::channel::<ChannelMessage>(16);
    let loop_task = zeroclaw_spawn::spawn!(run_message_dispatch_loop(rx, router, 4));
    for msg in inputs {
        tx.send(msg).await.unwrap();
    }
    // Positive control: a mapped message still reaches its agent.
    tx.send(parse(PARTNER, PARTNER, "hello")).await.unwrap();
    drop(tx);
    tokio::time::timeout(Duration::from_secs(10), loop_task)
        .await
        .expect("dispatch loop drains")
        .unwrap();

    assert!(agents[0].provider.calls().is_empty(), "owner ran");
    assert!(agents[2].provider.calls().is_empty(), "household ran");
    let partner_calls = agents[1].provider.calls();
    assert!(!partner_calls.is_empty(), "control message must run");
    assert!(partner_calls.iter().all(|c| !c.contains("approve run-1")));
    for agent in &agents {
        assert!(
            agent
                .ctx
                .route_overrides
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_empty()
        );
    }
}

/// The daemon boots with validation errors as warnings, so every invalid
/// route table must still fail closed: the alias reaches no agent, and an
/// agent that also lists the alias does not become its owner.
#[test]
fn invalid_route_tables_fail_closed_at_runtime() {
    let tmp = tempfile::TempDir::new().unwrap();
    let base = stage_one_config(tmp.path());
    let ctxs: Vec<(&str, Arc<ChannelRuntimeContext>)> = ["owner", "partner", "household", "ops"]
        .into_iter()
        .map(|alias| (alias, named_ctx(alias)))
        .collect();
    let set_routes = |config: &mut Config, routes: &[(&str, &str)]| {
        config.channels.telegram.get_mut("home").unwrap().routes = Some(
            routes
                .iter()
                .map(|(id, agent)| ((*id).to_string(), (*agent).to_string()))
                .collect(),
        );
    };
    let owner_id = OWNER.to_string();
    let partner_id = PARTNER.to_string();
    let group_id = GROUP.to_string();

    let mut empty = base.clone();
    set_routes(&mut empty, &[]);
    let mut malformed = base.clone();
    set_routes(
        &mut malformed,
        &[(owner_id.as_str(), "owner"), ("01002", "partner")],
    );
    let mut missing_agent = base.clone();
    set_routes(
        &mut missing_agent,
        &[(owner_id.as_str(), "owner"), (partner_id.as_str(), "ghost")],
    );
    let mut shared_agent = base.clone();
    set_routes(
        &mut shared_agent,
        &[(owner_id.as_str(), "owner"), (group_id.as_str(), "owner")],
    );
    let mut also_bound = base.clone();
    also_bound
        .agents
        .get_mut("ops")
        .unwrap()
        .channels
        .push("telegram.home".into());

    for (case, config) in [
        ("empty", &empty),
        ("malformed", &malformed),
        ("missing agent", &missing_agent),
        ("group shares a DM agent", &shared_agent),
        ("alias also in agents.ops.channels", &also_bound),
    ] {
        assert!(config.validate().is_err(), "{case}: validation must fail");
        assert_eq!(config.agent_for_channel("telegram.home"), None, "{case}");
        let router = routed_router(config, &ctxs);
        for (chat, from) in [(OWNER, OWNER), (PARTNER, PARTNER), (GROUP, OWNER)] {
            assert_eq!(
                resolved_alias(&router, &parse(chat, from, "hi")),
                None,
                "{case}: chat {chat} must reach no agent"
            );
        }
    }
}
