//! Memberships own enrollment; the declarative config owns template policy.
//! Derived agent entries are transient views and are never saved to TOML.
use anyhow::{Context, Result};
use zeroclaw_config::schema::Config;

use crate::telegram_memberships::{Membership, MembershipKind, MembershipStore};

pub(crate) fn materialize(
    config: &mut Config,
    channel_alias: &str,
    membership: &Membership,
) -> Result<String> {
    let telegram = config
        .channels
        .telegram
        .get(channel_alias)
        .context("Telegram enrollment channel is missing")?;
    config.validate_telegram_invitations(channel_alias, telegram)?;
    anyhow::ensure!(
        !telegram
            .routes
            .as_ref()
            .is_some_and(|routes| routes.contains_key(&membership.chat_id.to_string())),
        "Telegram enrollment chat is reserved by a static route"
    );
    let invitations = telegram
        .invitations
        .as_ref()
        .context("Telegram invitations are disabled")?;
    let template_alias = match membership.kind {
        MembershipKind::Private => &invitations.guest_agent,
        MembershipKind::Group => &invitations.group_agent,
    };
    let mut agent = config
        .agents
        .get(template_alias)
        .context("Telegram enrollment template is missing")?
        .clone();
    let alias = membership.agent_alias(channel_alias);
    anyhow::ensure!(
        !config.agents.contains_key(&alias),
        "Telegram enrollment identity collides with a configured agent"
    );
    // The validated template has no sibling data grants or custom workspace.
    agent.channels.clear();
    agent.delegate_same_risk_profile = false;
    agent.delegates.clear();
    config.agents.insert(alias.clone(), agent);
    Ok(alias)
}

/// Reconstruct a generated identity after rereading Nix-owned configuration.
pub(crate) fn materialize_for_alias(config: &mut Config, agent_alias: &str) -> Result<()> {
    if config.agents.contains_key(agent_alias) {
        return Ok(());
    }
    let candidates: Vec<String> = config
        .channels
        .telegram
        .iter()
        .filter(|(channel, tg)| {
            tg.invitations.is_some() && agent_alias.starts_with(&format!("tg_{channel}_"))
        })
        .map(|(channel, _)| channel.clone())
        .collect();
    // Leave ordinary aliases to the existing static agent resolver.
    if candidates.is_empty() {
        return Ok(());
    }
    for channel in candidates {
        let store = MembershipStore::open(&config.data_dir, &channel)?;
        for membership in store.list()? {
            if membership.agent_alias(&channel) == agent_alias {
                materialize(config, &channel, &membership)?;
                return Ok(());
            }
        }
    }
    anyhow::bail!("Telegram enrollment identity is inactive or missing")
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeroclaw_config::schema::{
        AliasedAgentConfig, RiskProfileConfig, TelegramConfig, TelegramInvitationsConfig,
    };

    fn config(root: &std::path::Path) -> Config {
        let mut config = Config {
            data_dir: root.join("data"),
            config_path: root.join("config.toml"),
            ..Default::default()
        };
        config.risk_profiles.insert(
            "guests".into(),
            RiskProfileConfig {
                allowed_tools: vec![
                    "memory_recall".into(),
                    "memory_store".into(),
                    "memory_forget".into(),
                    "reaction".into(),
                ],
                ..Default::default()
            },
        );
        for name in ["guest", "group"] {
            config.agents.insert(
                name.into(),
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

    #[tokio::test]
    async fn independent_memberships_isolate_real_memory_and_survive_config_regeneration() {
        let root = tempfile::TempDir::new().unwrap();
        let base = config(root.path());
        let store = MembershipStore::open(&base.data_dir, "home").unwrap();
        let first = store
            .redeem(&store.create_invite(100).unwrap(), 2001, 101)
            .unwrap();
        let second = store
            .redeem(&store.create_invite(100).unwrap(), 2002, 101)
            .unwrap();
        let group = store.activate_group(-3001, 101).unwrap();
        let other_group = store.activate_group(-3002, 101).unwrap();
        let mut memories = Vec::new();
        for membership in [&first, &second, &group, &other_group] {
            let mut effective = base.clone();
            let alias = materialize(&mut effective, "home", membership).unwrap();
            let memory = zeroclaw_memory::create_memory_for_agent(&effective, &alias, None)
                .await
                .unwrap();
            memories.push(memory);
        }
        memories[0]
            .store(
                "private_fact",
                "cobalt hedgehog",
                zeroclaw_memory::MemoryCategory::Core,
                None,
            )
            .await
            .unwrap();
        assert!(
            !memories[0]
                .recall("cobalt hedgehog", 10, None, None, None)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            memories[1]
                .recall("cobalt hedgehog", 10, None, None, None)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            memories[2]
                .recall("cobalt hedgehog", 10, None, None, None)
                .await
                .unwrap()
                .is_empty()
        );
        memories[2]
            .store(
                "private_fact",
                "marigold otter",
                zeroclaw_memory::MemoryCategory::Core,
                None,
            )
            .await
            .unwrap();
        assert!(
            memories[3]
                .recall("marigold otter", 10, None, None, None)
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            memories[0]
                .get("private_fact")
                .await
                .unwrap()
                .unwrap()
                .content,
            "cobalt hedgehog"
        );
        assert_eq!(
            memories[2]
                .get("private_fact")
                .await
                .unwrap()
                .unwrap()
                .content,
            "marigold otter"
        );
        let alias = first.agent_alias("home");
        let mut regenerated = base.clone();
        materialize_for_alias(&mut regenerated, &alias).unwrap();
        let restored = zeroclaw_memory::create_memory_for_agent(&regenerated, &alias, None)
            .await
            .unwrap();
        assert!(
            !restored
                .recall("cobalt hedgehog", 10, None, None, None)
                .await
                .unwrap()
                .is_empty()
        );
        store.revoke(2001).unwrap();
        assert!(materialize_for_alias(&mut base.clone(), &alias).is_err());
    }

    #[test]
    fn templates_cannot_reuse_owner_memory_or_grant_shell() {
        let root = tempfile::TempDir::new().unwrap();
        let mut cfg = config(root.path());
        let validate =
            |c: &Config| c.validate_telegram_invitations("home", &c.channels.telegram["home"]);
        assert!(validate(&cfg).is_ok());
        cfg.risk_profiles
            .get_mut("guests")
            .unwrap()
            .allowed_tools
            .push("shell".into());
        assert!(validate(&cfg).is_err());
        cfg.risk_profiles
            .get_mut("guests")
            .unwrap()
            .allowed_tools
            .pop();
        assert!(validate(&cfg).is_ok());
        cfg.risk_profiles
            .get_mut("guests")
            .unwrap()
            .allowed_tools
            .push("send_via".into());
        assert!(validate(&cfg).is_err());
        cfg.risk_profiles
            .get_mut("guests")
            .unwrap()
            .allowed_tools
            .pop();
        cfg.agents.get_mut("guest").unwrap().workspace.path = Some(root.path().join("shared"));
        assert!(validate(&cfg).is_err());
    }
}
