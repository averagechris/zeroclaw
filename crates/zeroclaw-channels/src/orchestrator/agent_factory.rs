//! Per-agent setup shared by static channel startup and dynamic agent bindings.

use super::*;

pub(super) struct PreparedChannelAgent {
    agent_alias: String,
    config: Config,
    agent: zeroclaw_config::schema::AliasedAgentConfig,
    risk_profile: zeroclaw_config::schema::RiskProfileConfig,
    model_provider: Arc<dyn ModelProvider>,
    provider_name: String,
    model: String,
    temperature: Option<f64>,
    provider_runtime_options: zeroclaw_providers::ModelProviderRuntimeOptions,
    mem: Arc<dyn Memory>,
    tools_registry: Arc<zeroclaw_runtime::tools::scoped::ScopedToolRegistry>,
    observer: Arc<dyn Observer>,
    system_prompt: String,
    workspace: PathBuf,
    skill_names: Vec<String>,
    ask_user_handle_ch: Option<tools::PerToolChannelHandle>,
    reaction_handle_ch: tools::PerToolChannelHandle,
    poll_handle_ch: Option<tools::PerToolChannelHandle>,
    escalate_handle_ch: Option<tools::PerToolChannelHandle>,
    channel_room_handle_ch: Option<tools::PerToolChannelHandle>,
    ch_activated_handle: Option<Arc<std::sync::Mutex<tools::ActivatedToolSet>>>,
    sop_engine: Option<Arc<std::sync::Mutex<zeroclaw_runtime::sop::SopEngine>>>,
    sop_audit: Option<Arc<zeroclaw_runtime::sop::SopAuditLogger>>,
    pub(super) tool_specs: Vec<(String, String)>,
}

impl PreparedChannelAgent {
    pub(super) async fn prepare(
        config: &Config,
        config_arc: Arc<RwLock<Config>>,
        agent_alias: &str,
        observer: Arc<dyn Observer>,
        runtime: Arc<dyn platform::RuntimeAdapter>,
        canvas_store: Option<zeroclaw_runtime::tools::CanvasStore>,
        sop_engine: Option<Arc<std::sync::Mutex<zeroclaw_runtime::sop::SopEngine>>>,
        sop_audit: Option<Arc<zeroclaw_runtime::sop::SopAuditLogger>>,
        warmup: bool,
    ) -> Result<Self> {
        let agent = config
            .resolved_agent_config(agent_alias)
            .with_context(|| format!("agents.{agent_alias} is not configured"))?;
        let risk_profile = config
            .risk_profile_for_agent(agent_alias)
            .with_context(|| {
                format!(
                    "agents.{agent_alias}.risk_profile does not name a configured risk_profiles entry"
                )
            })?
            .clone();

        // Resolve the agent's model provider strictly from its mandatory
        // `<type>.<alias>` reference. No fallback to a first/default provider:
        // an agent whose ref does not resolve to a configured entry with a
        // `model` is rejected here.
        let runtime_defaults = runtime_defaults_from_config(config, agent.model_provider.as_str())
            .with_context(|| format!("agents.{agent_alias}.model_provider"))?;
        let provider_name = runtime_defaults.default_model_provider.clone();
        let model = runtime_defaults.model.clone();
        let temperature = runtime_defaults.temperature;
        let provider_api_key = runtime_defaults.api_key.clone();
        let provider_api_url = runtime_defaults.api_url.clone();
        let provider_reliability = runtime_defaults.reliability.clone();
        let provider_runtime_options =
            zeroclaw_providers::provider_runtime_options_for_agent(config, agent_alias);
        let model_provider: Arc<dyn ModelProvider> = Arc::from(
            create_resilient_model_provider_nonblocking(
                Arc::new(config.clone()),
                &provider_name,
                provider_api_key.clone(),
                provider_api_url.clone(),
                provider_reliability.clone(),
                provider_runtime_options.clone(),
            )
            .await?,
        );

        if warmup && let Err(e) = ProviderDispatch::from_ref(&*model_provider).warmup().await {
            ::zeroclaw_log::record!(
                WARN,
                ::zeroclaw_log::Event::new(module_path!(), ::zeroclaw_log::Action::Note)
                    .with_outcome(::zeroclaw_log::EventOutcome::Unknown)
                    .with_attrs(
                        ::serde_json::json!({"error": format!("{}", e), "agent": agent_alias})
                    ),
                "ModelProvider warmup failed (non-fatal)"
            );
        }

        let security = Arc::new(SecurityPolicy::for_agent(config, agent_alias)?);
        let mem: Arc<dyn Memory> = zeroclaw_memory::create_memory_for_agent(
            config,
            agent_alias,
            provider_api_key.as_deref(),
        )
        .await?;
        let (composio_key, composio_entity_id) = if config.composio.enabled {
            (
                config.composio.api_key.as_deref(),
                Some(config.composio.entity_id.as_str()),
            )
        } else {
            (None, None)
        };

        let workspace = config.agent_workspace_dir(agent_alias);
        // Per-agent skills: install-wide workspace + open_skills set,
        // unioned with this agent's declared `skill_bundles`.
        let skills =
            zeroclaw_runtime::skills::load_skills_for_agent(&workspace, config, agent_alias);

        let all_tools_result_ch = tools::all_tools_with_runtime(
            Arc::new(config.clone()),
            &security,
            &risk_profile,
            agent_alias,
            Arc::clone(&runtime),
            Arc::clone(&mem),
            composio_key,
            composio_entity_id,
            &config.browser,
            &config.http_request,
            &config.web_fetch,
            &workspace,
            &config.agents,
            provider_api_key.as_deref(),
            config,
            canvas_store.clone(),
            false,
            None,
            sop_engine.clone(),
            sop_audit.clone(),
            Some(Arc::clone(&config_arc)),
        )?;
        // Route the per-agent tool registry through the one gated seam - see
        // `assemble_channel_agent_tools` for the knobs and why. `mut` because the
        // text-tool prompt policy below may clear `deferred_section` for a
        // non-native strict-tool-parsing target.
        let ChannelAssembledTools {
            tools: built_tools,
            mut deferred_section,
            pinned_section,
            ask_user_handle: ask_user_handle_ch,
            reaction_handle: reaction_handle_ch,
            poll_handle: poll_handle_ch,
            escalate_handle: escalate_handle_ch,
            channel_room_handle: channel_room_handle_ch,
            activated_handle: ch_activated_handle,
        } = assemble_channel_agent_tools(
            config,
            agent_alias,
            provider_name.as_str(),
            model.as_str(),
            &security,
            all_tools_result_ch,
            &skills,
            Arc::clone(&runtime),
        )
        .await;

        let tool_specs: Vec<(String, String)> = built_tools
            .iter()
            .map(|t| (t.name().to_string(), t.description().to_string()))
            .collect();

        let tools_registry = Arc::new(built_tools);

        let mut tool_descs: Vec<(&str, &str)> = vec![
            (
                "shell",
                "Execute terminal commands. Use when: running local checks, build/test commands, diagnostics. Don't use when: a safer dedicated tool exists, or command is destructive without approval.",
            ),
            (
                "file_read",
                "Read file contents. Use when: inspecting project files, configs, logs. Don't use when: a targeted search is enough.",
            ),
            (
                "file_write",
                "Write file contents. Use when: applying focused edits, scaffolding files, updating docs/code. Don't use when: side effects are unclear or file ownership is uncertain.",
            ),
            (
                "memory_store",
                "Save to memory. Use when: preserving durable preferences, decisions, key context. Don't use when: information is transient/noisy/sensitive without need.",
            ),
            (
                "memory_recall",
                "Search memory. Use when: retrieving prior decisions, user preferences, historical context. Don't use when: answer is already in current context.",
            ),
            (
                "memory_forget",
                "Delete a memory entry. Use when: memory is incorrect/stale or explicitly requested for removal. Don't use when: impact is uncertain.",
            ),
        ];

        if matches!(
            config.effective_skills_prompt_mode(agent_alias),
            zeroclaw_config::schema::SkillsPromptInjectionMode::Compact
        ) {
            tool_descs.push((
                "read_skill",
                "Load the full source for an available skill by name. Use when: compact mode only shows a summary and you need the complete skill instructions.",
            ));
        }
        if config.browser.enabled {
            tool_descs.push((
                "browser_open",
                "Open approved HTTPS URLs in system browser (allowlist-only, no scraping)",
            ));
        }
        if config.composio.enabled {
            tool_descs.push((
                "composio",
                "Execute actions on 1000+ apps via Composio (Gmail, Notion, GitHub, Slack, etc.). Use action='list' to discover actions, 'list_accounts' to retrieve connected account IDs, 'execute' to run (optionally with connected_account_id), and 'connect' for OAuth.",
            ));
        }
        tool_descs.push((
            "schedule",
            "Manage scheduled tasks (create/list/get/cancel/pause/resume). Supports recurring cron and one-shot delays.",
        ));
        tool_descs.push((
            "pushover",
            "Send a Pushover notification to your device. Requires PUSHOVER_TOKEN and PUSHOVER_USER_KEY in .env file.",
        ));
        tool_descs.push((
            "channel_room",
            "Create channel rooms and invite users through active channels. Use with Matrix channel keys such as matrix.default.",
        ));
        if !config.agents.is_empty() {
            tool_descs.push((
                "delegate",
                "Delegate a subtask to a specialized agent. Use when: a task benefits from a different model (e.g. fast summarization, deep reasoning, code generation). The sub-agent runs a single prompt and returns its response.",
            ));
        }
        if config.channels.email.values().any(|c| c.enabled) {
            tool_descs.push((
                "email_search",
                "Search the IMAP inbox by sender, subject, or date. Returns a list of matching emails with UID, sender, subject, and date. Use when asked about email. Follow up with email_read to fetch the full body.",
            ));
            tool_descs.push((
                "email_read",
                "Fetch the full content of an email by its UID (from email_search). Returns sender, to, date, subject, body text, and attachments.",
            ));
        }

        // Filter out tools excluded for non-CLI channels so this agent's
        // system prompt does not advertise them for channel-driven runs.
        {
            let active_profile = &risk_profile;
            let excluded = &active_profile.excluded_tools;
            if !excluded.is_empty() && active_profile.level != AutonomyLevel::Full {
                tool_descs.retain(|(name, _)| !excluded.iter().any(|ex| ex == name));
            }
        }
        let effective_tool_names =
            effective_non_cli_tool_names(tools_registry.as_ref(), &risk_profile);
        tool_descs.retain(|(name, _)| effective_tool_names.contains(name));

        let bootstrap_max_chars = if agent.resolved.compact_context {
            Some(6000)
        } else {
            None
        };
        let startup_excluded_tools: &[String] = if risk_profile.level == AutonomyLevel::Full {
            &[]
        } else {
            &risk_profile.excluded_tools
        };
        let native_tools = ::zeroclaw_runtime::agent::loop_::native_tool_specs_present_for_turn(
            model_provider.as_ref(),
            model.as_str(),
            tools_registry.as_ref(),
            startup_excluded_tools,
            ch_activated_handle.as_ref(),
        )?;
        let expose_text_tool_protocol = compose_channel_mcp_prompt_sections(
            native_tools,
            agent.resolved.strict_tool_parsing,
            &mut tool_descs,
            &mut deferred_section,
            &pinned_section,
        );
        let callable_protocol_exposed = native_tools || expose_text_tool_protocol;
        let mut system_prompt = build_system_prompt_with_mode_and_effective_tools(
            &workspace,
            &model,
            &tool_descs,
            |name| callable_protocol_exposed && effective_tool_names.contains(name),
            &skills,
            Some(&agent.identity),
            bootstrap_max_chars,
            Some(&risk_profile),
            native_tools,
            config.effective_skills_prompt_mode(agent_alias),
            agent.resolved.compact_context,
            agent.resolved.max_system_prompt_chars,
            true,
            config.channels.show_tool_calls,
            runtime.shell_profile().as_ref(),
        );
        if expose_text_tool_protocol {
            system_prompt.push_str(&build_tool_instructions_for_names(
                tools_registry.as_ref(),
                &effective_tool_names,
            ));
        }
        if !deferred_section.is_empty() {
            system_prompt.push('\n');
            system_prompt.push_str(&deferred_section);
        }
        if agent.resolved.tool_receipts.enabled && agent.resolved.tool_receipts.inject_system_prompt
        {
            system_prompt.push_str(zeroclaw_runtime::agent::tool_receipts::SYSTEM_PROMPT_ADDENDUM);
        }

        let skill_names = skills.iter().map(|skill| skill.name.clone()).collect();
        Ok(Self {
            agent_alias: agent_alias.to_string(),
            config: config.clone(),
            agent,
            risk_profile,
            model_provider,
            provider_name,
            model,
            temperature,
            provider_runtime_options,
            mem,
            tools_registry,
            observer,
            system_prompt,
            workspace,
            skill_names,
            ask_user_handle_ch,
            reaction_handle_ch,
            poll_handle_ch,
            escalate_handle_ch,
            channel_room_handle_ch,
            ch_activated_handle,
            sop_engine,
            sop_audit,
            tool_specs,
        })
    }

    pub(super) fn model(&self) -> &str {
        &self.model
    }

    pub(super) fn skill_names(&self) -> &[String] {
        &self.skill_names
    }

    pub(super) fn finish(
        self,
        channels_by_name: Arc<HashMap<String, Arc<dyn Channel>>>,
        session_store: Option<Arc<dyn SessionBackend>>,
    ) -> Arc<ChannelRuntimeContext> {
        let Self {
            agent_alias,
            config,
            agent,
            risk_profile,
            model_provider,
            provider_name,
            model,
            temperature,
            provider_runtime_options,
            mem,
            tools_registry,
            observer,
            system_prompt,
            workspace,
            ask_user_handle_ch,
            reaction_handle_ch,
            poll_handle_ch,
            escalate_handle_ch,
            channel_room_handle_ch,
            ch_activated_handle,
            sop_engine,
            sop_audit,
            ..
        } = self;

        // Wire this agent's reaction / ask_user / channel room / escalate tool handles
        // into the shared `channels_by_name` map.
        {
            let mut map = reaction_handle_ch.write();
            for (name, ch) in channels_by_name.as_ref() {
                map.insert(name.clone(), Arc::clone(ch));
            }
        }
        if let Some(ref handle) = ask_user_handle_ch {
            let mut map = handle.write();
            for (name, ch) in channels_by_name.as_ref() {
                map.insert(name.clone(), Arc::clone(ch));
            }
        }
        if let Some(ref handle) = channel_room_handle_ch {
            let mut map = handle.write();
            for (name, ch) in channels_by_name.as_ref() {
                map.insert(name.clone(), Arc::clone(ch));
            }
        }
        if let Some(ref handle) = poll_handle_ch {
            let mut map = handle.write();
            for (name, ch) in channels_by_name.as_ref() {
                map.insert(name.clone(), Arc::clone(ch));
            }
        }
        if let Some(ref handle) = escalate_handle_ch {
            let mut map = handle.write();
            for (name, ch) in channels_by_name.as_ref() {
                map.insert(name.clone(), Arc::clone(ch));
            }
        }

        let mut provider_cache_seed: HashMap<String, Arc<dyn ModelProvider>> = HashMap::new();
        provider_cache_seed.insert(provider_name.clone(), Arc::clone(&model_provider));
        let message_timeout_secs =
            effective_channel_message_timeout_secs(config.channels.message_timeout_secs);
        let interrupt_on_new_message = interrupt_on_new_message_config(&config.channels);
        let memory_strategy: Arc<dyn MemoryStrategy> = Arc::new(
            zeroclaw_runtime::agent::memory_strategy::DefaultMemoryStrategy::with_config(
                Arc::clone(&mem),
                config.memory.clone(),
                config.data_dir.clone(),
            ),
        );

        Arc::new(ChannelRuntimeContext {
            channels_by_name,
            model_provider: Arc::clone(&model_provider),
            model_provider_ref: Arc::new(provider_name.clone()),
            agent_alias: Arc::new(agent_alias.clone()),
            agent_cfg: Arc::new(agent.clone()),
            prompt_config: Arc::new(config.clone()),
            memory: Arc::clone(&mem),
            memory_strategy,
            tools_registry: Arc::clone(&tools_registry),
            observer: Arc::clone(&observer),
            system_prompt: Arc::new(system_prompt),
            model: Arc::new(model.clone()),
            temperature,
            auto_save_memory: config.memory.auto_save,
            max_tool_iterations: config.effective_max_tool_iterations(agent_alias.as_str()),
            min_relevance_score: config.memory.min_relevance_score,
            conversation_histories: Arc::new(Mutex::new(lru::LruCache::new(
                std::num::NonZeroUsize::new(MAX_CONVERSATION_SENDERS)
                    .expect("MAX_CONVERSATION_SENDERS must be positive"),
            ))),
            history_crumb_flags: Arc::new(Mutex::new(lru::LruCache::new(
                std::num::NonZeroUsize::new(MAX_CONVERSATION_SENDERS)
                    .expect("MAX_CONVERSATION_SENDERS must be positive"),
            ))),
            pending_new_sessions: Arc::new(Mutex::new(HashSet::new())),
            provider_cache: Arc::new(Mutex::new(provider_cache_seed)),
            route_overrides: Arc::new(Mutex::new(HashMap::new())),
            thinking_overrides: Arc::new(Mutex::new(HashMap::new())),
            scope_overrides: Arc::new(Mutex::new(HashMap::new())),
            reliability: Arc::new(config.reliability.clone()),
            provider_runtime_options,
            workspace_dir: Arc::new(workspace.clone()),
            message_timeout_secs,
            interrupt_on_new_message,
            multimodal: config.multimodal.clone(),
            media_pipeline: config.media_pipeline.clone(),
            transcription_config: config.transcription.clone(),
            agent_transcription_provider: agent.transcription_provider.as_str().to_string(),
            hooks: if config.hooks.enabled {
                Some(Arc::new(zeroclaw_runtime::hooks::HookRunner::from_config(
                    &config.hooks,
                )))
            } else {
                None
            },
            non_cli_excluded_tools: Arc::new(risk_profile.excluded_tools.clone()),
            autonomy_level: risk_profile.level,
            tool_call_dedup_exempt: Arc::new(agent.resolved.tool_call_dedup_exempt.clone()),
            model_routes: Arc::new(config.model_routes.clone()),
            query_classification: config.query_classification.clone(),
            ack_reactions: config.channels.ack_reactions,
            show_tool_calls: config.channels.show_tool_calls,
            session_store,
            approval_manager: Arc::new(ApprovalManager::for_non_interactive(&risk_profile)),
            activated_tools: ch_activated_handle,
            cost_tracking: zeroclaw_runtime::cost::CostTracker::get_or_init_global(
                config.cost.clone(),
                &config.data_dir,
            )
            .map(|tracker| {
                let by_type =
                    zeroclaw_runtime::agent::cost::build_type_level_model_provider_pricing(&config);
                ChannelCostTrackingState {
                    tracker,
                    model_provider_pricing: Arc::new(by_type),
                    agent_alias: Arc::new(agent_alias.clone()),
                }
            }),
            pacing: config.pacing.clone(),
            max_tool_result_chars: agent.resolved.max_tool_result_chars,
            context_token_budget: agent.resolved.effective_context_budget(),
            debouncer: Arc::new(zeroclaw_infra::debounce::MessageDebouncer::new(
                Duration::from_millis(config.channels.debounce_ms),
            )),
            receipt_generator: if agent.resolved.tool_receipts.enabled {
                Some(zeroclaw_runtime::agent::tool_receipts::ReceiptGenerator::new())
            } else {
                None
            },
            show_receipts_in_response: agent.resolved.tool_receipts.show_in_response,
            last_applied_config_stamp: Arc::new(Mutex::new(None)),
            runtime_defaults_override: Arc::new(Mutex::new(None)),
            persist_locks: Arc::new(std::sync::Mutex::new(HashMap::new())),
            sop_engine,
            sop_audit,
        })
    }
}
