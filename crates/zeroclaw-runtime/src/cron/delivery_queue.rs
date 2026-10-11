//! Durable transport obligations for completed cron runs.
//!
//! `cron_jobs` remains the schedule authority. This table records only the
//! immutable notification that a completed run still needs to deliver.

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use zeroclaw_config::schema::Config;

use super::scheduler::announce_delivery_decision;
use super::store::with_initialized_connection;
use super::{CronJob, DeliveryConfig};

const MAX_ATTEMPTS: i64 = 8;
const BASE_BACKOFF_SECONDS: i64 = 2;
const MAX_BACKOFF_SECONDS: i64 = 15 * 60;
const CLAIM_LEASE_SECONDS: i64 = 5 * 60;
const MAX_ERROR_BYTES: usize = 2048;
const MAX_TERMINAL_HISTORY_PER_JOB: i64 = 50;
const MAX_TERMINAL_HISTORY_TOTAL: i64 = 500;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PendingDelivery {
    pub id: i64,
    pub delivery: DeliveryConfig,
    pub output: String,
    pub agent_alias: String,
    pub attempts: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct DeliveryStatus {
    pub id: i64,
    pub status: String,
    pub attempts: u32,
    pub last_error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub delivered_at: Option<DateTime<Utc>>,
}

/// Insert the notification obligation in the same transaction as its run.
/// The caller supplies a stable key derived from job, run start, and run mode.
pub(crate) fn enqueue_on(
    conn: &Connection,
    job: &CronJob,
    event_key: &str,
    output: &str,
) -> Result<()> {
    if !job.delivery.mode.eq_ignore_ascii_case("announce")
        || !announce_delivery_decision(output).should_deliver()
    {
        return Ok(());
    }

    let delivery =
        serde_json::to_string(&job.delivery).context("serialize cron delivery config")?;
    conn.execute(
        "INSERT OR IGNORE INTO cron_delivery_queue
            (event_key, job_id, agent_alias, delivery, output, status, attempts,
             available_at, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, 'pending', 0, ?6, ?6)",
        params![
            event_key,
            job.id,
            job.agent_alias,
            delivery,
            output,
            Utc::now().to_rfc3339(),
        ],
    )?;
    prune_terminal_history_on(conn, &job.id)
}

fn prune_terminal_history_on(conn: &Connection, job_id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM cron_delivery_queue WHERE id IN (
            SELECT id FROM cron_delivery_queue
            WHERE job_id = ?1 AND status IN ('sent', 'cancelled')
            ORDER BY created_at DESC, id DESC LIMIT -1 OFFSET ?2
         )",
        params![job_id, MAX_TERMINAL_HISTORY_PER_JOB],
    )?;
    conn.execute(
        "DELETE FROM cron_delivery_queue WHERE id IN (
            SELECT id FROM cron_delivery_queue
            WHERE status IN ('sent', 'cancelled')
            ORDER BY created_at DESC, id DESC LIMIT -1 OFFSET ?1
         )",
        params![MAX_TERMINAL_HISTORY_TOTAL],
    )?;
    Ok(())
}

/// Claim due notifications for transport. Claimed rows become eligible again
/// after a short lease if the process exits before acknowledging them.
pub(crate) fn pending(
    config: &Config,
    now: DateTime<Utc>,
    limit: usize,
) -> Result<Vec<PendingDelivery>> {
    with_initialized_connection(config, |conn| {
        let tx = conn.unchecked_transaction()?;
        let now_text = now.to_rfc3339();
        let lease_until = (now + Duration::seconds(CLAIM_LEASE_SECONDS)).to_rfc3339();
        let bounded_limit = i64::try_from(limit.max(1)).context("delivery queue limit overflow")?;
        let ids = {
            let mut stmt = tx.prepare(
                "SELECT id FROM cron_delivery_queue
                 WHERE (status = 'pending' AND available_at <= ?1)
                    OR (status = 'sending' AND lease_until <= ?1)
                 ORDER BY available_at ASC, id ASC LIMIT ?2",
            )?;
            let rows =
                stmt.query_map(params![now_text, bounded_limit], |row| row.get::<_, i64>(0))?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        };
        let mut deliveries = Vec::with_capacity(ids.len());
        for id in ids {
            tx.execute(
                "UPDATE cron_delivery_queue SET status = 'sending', lease_until = ?2
                 WHERE id = ?1 AND ((status = 'pending' AND available_at <= ?3)
                    OR (status = 'sending' AND lease_until <= ?3))",
                params![id, lease_until, now_text],
            )?;
            if let Some(row_data) = tx
                .query_row(
                    "SELECT delivery, output, agent_alias, attempts
                     FROM cron_delivery_queue WHERE id = ?1 AND status = 'sending' AND lease_until = ?2",
                    params![id, lease_until],
                    |row| {
                        let delivery_raw: String = row.get(0)?;
                        let attempts: i64 = row.get(3)?;
                        Ok((delivery_raw, row.get::<_, String>(1)?, row.get::<_, String>(2)?, attempts))
                    },
                )
                .optional()?
            {
                let delivery_config = serde_json::from_str(&row_data.0)
                    .context("decode queued cron delivery config")?;
                deliveries.push(PendingDelivery {
                    id,
                    delivery: delivery_config,
                    output: row_data.1,
                    agent_alias: row_data.2,
                    attempts: u32::try_from(row_data.3)
                        .context("invalid delivery attempt count")?,
                });
            }
        }
        tx.commit().context("commit cron delivery claims")?;
        Ok(deliveries)
    })
}

pub(crate) fn acknowledge(config: &Config, id: i64) -> Result<bool> {
    with_initialized_connection(config, |conn| {
        let tx = conn.unchecked_transaction()?;
        let job_id: Option<String> = tx
            .query_row(
                "SELECT job_id FROM cron_delivery_queue WHERE id=?1 AND status='sending'",
                [id],
                |row| row.get(0),
            )
            .optional()?;
        let Some(job_id) = job_id else {
            return Ok(false);
        };
        tx.execute(
            "UPDATE cron_delivery_queue SET status = 'sent', delivered_at = ?2,
                    lease_until = NULL, last_error = NULL
             WHERE id = ?1 AND status = 'sending'",
            params![id, Utc::now().to_rfc3339()],
        )?;
        prune_terminal_history_on(&tx, &job_id)?;
        tx.commit()?;
        Ok(true)
    })
}

/// Check that a previously claimed item is still active immediately before
/// sending it. Cancellation can happen while an earlier network send blocks.
pub(crate) fn is_claim_active(config: &Config, id: i64) -> Result<bool> {
    with_initialized_connection(config, |conn| {
        let active = conn
            .query_row(
                "SELECT 1 FROM cron_delivery_queue
                 WHERE id = ?1 AND status = 'sending'",
                [id],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        Ok(active)
    })
}

pub(crate) fn retry(config: &Config, id: i64, now: DateTime<Utc>, error: &str) -> Result<bool> {
    with_initialized_connection(config, |conn| {
        let tx = conn.unchecked_transaction()?;
        let attempts: Option<i64> = tx
            .query_row(
                "SELECT attempts FROM cron_delivery_queue WHERE id = ?1 AND status = 'sending'",
                params![id],
                |row| row.get(0),
            )
            .optional()?;
        let Some(attempts) = attempts else {
            tx.commit()?;
            return Ok(false);
        };
        let next_attempt = attempts + 1;
        let status = if next_attempt >= MAX_ATTEMPTS {
            "failed"
        } else {
            "pending"
        };
        let exponent = u32::try_from(next_attempt.saturating_sub(1))
            .unwrap_or(u32::MAX)
            .min(30);
        let backoff = BASE_BACKOFF_SECONDS
            .saturating_mul(2_i64.saturating_pow(exponent))
            .min(MAX_BACKOFF_SECONDS);
        let error = truncate_error(error);
        tx.execute(
            "UPDATE cron_delivery_queue SET status = ?2, attempts = ?3,
                    available_at = ?4, lease_until = NULL, last_error = ?5
             WHERE id = ?1 AND status = 'sending'",
            params![
                id,
                status,
                next_attempt,
                (now + Duration::seconds(backoff)).to_rfc3339(),
                error
            ],
        )?;
        tx.commit().context("commit cron delivery retry")?;
        Ok(true)
    })
}

pub(crate) fn statuses_for_job(
    config: &Config,
    job_id: &str,
    agent_alias: &str,
) -> Result<Vec<DeliveryStatus>> {
    with_initialized_connection(config, |conn| {
        let mut stmt = conn.prepare(
            "SELECT id, status, attempts, last_error, created_at, delivered_at
             FROM cron_delivery_queue WHERE job_id = ?1 AND agent_alias = ?2
             ORDER BY created_at DESC, id DESC LIMIT 20",
        )?;
        let rows = stmt.query_map(params![job_id, agent_alias], |row| {
            let attempts: i64 = row.get(2)?;
            let created_at: String = row.get(4)?;
            let delivered_at: Option<String> = row.get(5)?;
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                attempts,
                row.get::<_, Option<String>>(3)?,
                created_at,
                delivered_at,
            ))
        })?;
        rows.map(|row| {
            let (id, status, attempts, last_error, created_at, delivered_at) = row?;
            Ok(DeliveryStatus {
                id,
                status,
                attempts: u32::try_from(attempts).context("invalid delivery attempt count")?,
                last_error,
                created_at: DateTime::parse_from_rfc3339(&created_at)?.with_timezone(&Utc),
                delivered_at: delivered_at
                    .map(|value| {
                        DateTime::parse_from_rfc3339(&value).map(|time| time.with_timezone(&Utc))
                    })
                    .transpose()?,
            })
        })
        .collect()
    })
}

pub(super) fn initialize_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS cron_delivery_queue (
            id           INTEGER PRIMARY KEY AUTOINCREMENT,
            event_key    TEXT NOT NULL UNIQUE,
            job_id       TEXT NOT NULL,
            agent_alias  TEXT NOT NULL,
            delivery     TEXT NOT NULL,
            output       TEXT NOT NULL,
            status       TEXT NOT NULL CHECK (status IN ('pending', 'sending', 'sent', 'failed', 'cancelled')),
            attempts     INTEGER NOT NULL DEFAULT 0,
            available_at TEXT NOT NULL,
            lease_until  TEXT,
            last_error   TEXT,
            created_at   TEXT NOT NULL,
            delivered_at TEXT
         );
         CREATE INDEX IF NOT EXISTS idx_cron_delivery_due
            ON cron_delivery_queue(status, available_at);
         CREATE INDEX IF NOT EXISTS idx_cron_delivery_job
            ON cron_delivery_queue(job_id, agent_alias, created_at);",
    )
    .context("Failed to initialize cron delivery queue schema")
}

pub(super) fn cancel_unsent_on(
    conn: &Connection,
    job_id: &str,
    agent_alias: Option<&str>,
) -> Result<()> {
    if let Some(agent_alias) = agent_alias {
        conn.execute(
            "UPDATE cron_delivery_queue SET status = 'cancelled', lease_until = NULL
             WHERE job_id = ?1 AND agent_alias = ?2 AND status IN ('pending', 'sending')",
            params![job_id, agent_alias],
        )?;
    } else {
        conn.execute(
            "UPDATE cron_delivery_queue SET status = 'cancelled', lease_until = NULL
             WHERE job_id = ?1 AND status IN ('pending', 'sending')",
            params![job_id],
        )?;
    }
    prune_terminal_history_on(conn, job_id)
}

fn truncate_error(error: &str) -> String {
    if error.len() <= MAX_ERROR_BYTES {
        return error.to_string();
    }
    let mut end = MAX_ERROR_BYTES;
    while !error.is_char_boundary(end) {
        end -= 1;
    }
    error[..end].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cron::Schedule;
    use crate::cron::store::{
        RunCompletionAction, add_shell_job_with_format, persist_run_result, remove_job_for_agent,
    };
    use tempfile::TempDir;

    fn test_config(tmp: &TempDir) -> Config {
        let mut config = Config {
            data_dir: tmp.path().join("data"),
            config_path: tmp.path().join("config.toml"),
            ..Config::default()
        };
        config.scheduler.missed_run_policy = zeroclaw_config::schema::MissedRunPolicy::Review;
        std::fs::create_dir_all(&config.data_dir).unwrap();
        config
    }

    fn announced_job(config: &Config, agent_alias: &str) -> CronJob {
        add_shell_job_with_format(
            config,
            agent_alias,
            None,
            Schedule::At {
                at: Utc::now() + Duration::hours(1),
            },
            "echo ok",
            Some(DeliveryConfig {
                mode: "announce".to_string(),
                channel: Some("telegram".to_string()),
                to: Some("chat-1".to_string()),
                thread_id: None,
                best_effort: true,
            }),
            zeroclaw_config::schema::CronShellOutputFormat::default(),
        )
        .unwrap()
    }

    #[test]
    fn result_queue_survives_one_shot_job_deletion_and_restart() {
        let tmp = TempDir::new().unwrap();
        let config = test_config(&tmp);
        let job = announced_job(&config, "owner-agent");
        let now = Utc::now();

        persist_run_result(
            &config,
            &job,
            now,
            now,
            now,
            "ok",
            Some("finished"),
            0,
            RunCompletionAction::Delete,
        )
        .unwrap();
        assert!(crate::cron::store::get_job(&config, &job.id).is_err());

        let queued = pending(&config, now + Duration::seconds(1), 10).unwrap();
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].agent_alias, "owner-agent");
        assert_eq!(queued[0].output, "finished");
        assert_eq!(queued[0].attempts, 0);

        // A new Config handle represents the same on-disk state after restart.
        let restarted_config = Config {
            data_dir: config.data_dir.clone(),
            config_path: config.config_path.clone(),
            ..Config::default()
        };
        assert!(
            pending(&restarted_config, now + Duration::seconds(2), 10)
                .unwrap()
                .is_empty()
        );
        assert!(acknowledge(&restarted_config, queued[0].id).unwrap());
        assert!(
            statuses_for_job(&restarted_config, &job.id, "owner-agent").unwrap()[0].status
                == "sent"
        );
    }

    #[test]
    fn retries_back_off_then_stop_and_status_reads_are_owner_scoped() {
        let tmp = TempDir::new().unwrap();
        let config = test_config(&tmp);
        let job = announced_job(&config, "owner-agent");
        let conn = crate::cron::store::with_initialized_connection(&config, |conn| {
            enqueue_on(conn, &job, "retry-test", "finished")
        });
        conn.unwrap();

        let mut current = Utc::now();
        let mut id = 0;
        for attempt in 1..=MAX_ATTEMPTS {
            let due = pending(&config, current, 1).unwrap();
            assert_eq!(due.len(), 1);
            id = due[0].id;
            assert!(retry(&config, id, current, "temporary failure").unwrap());
            current += Duration::seconds(MAX_BACKOFF_SECONDS + 1);
            assert_eq!(
                statuses_for_job(&config, &job.id, "wrong-agent")
                    .unwrap()
                    .len(),
                0
            );
            if attempt < MAX_ATTEMPTS {
                assert_eq!(
                    statuses_for_job(&config, &job.id, "owner-agent").unwrap()[0].attempts,
                    attempt as u32
                );
            }
        }

        assert!(
            pending(&config, current + Duration::hours(1), 1)
                .unwrap()
                .is_empty()
        );
        let failed = statuses_for_job(&config, &job.id, "owner-agent").unwrap();
        assert_eq!(failed[0].id, id);
        assert_eq!(failed[0].status, "failed");
        assert_eq!(failed[0].attempts, MAX_ATTEMPTS as u32);
    }

    #[test]
    fn explicit_job_removal_cancels_unsent_notification() {
        let tmp = TempDir::new().unwrap();
        let config = test_config(&tmp);
        let job = announced_job(&config, "owner-agent");
        crate::cron::store::with_initialized_connection(&config, |conn| {
            enqueue_on(conn, &job, "remove-test", "finished")
        })
        .unwrap();

        remove_job_for_agent(&config, &job.id, "owner-agent").unwrap();
        assert!(
            pending(&config, Utc::now() + Duration::hours(1), 10)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            statuses_for_job(&config, &job.id, "owner-agent").unwrap()[0].status,
            "cancelled"
        );
    }

    #[test]
    fn removed_job_invalidates_a_claimed_delivery_before_send() {
        let tmp = TempDir::new().unwrap();
        let config = test_config(&tmp);
        let job = announced_job(&config, "owner-agent");
        crate::cron::store::with_initialized_connection(&config, |conn| {
            enqueue_on(conn, &job, "claimed-remove-test", "finished")
        })
        .unwrap();

        let claimed = pending(&config, Utc::now() + Duration::hours(1), 10).unwrap();
        assert_eq!(claimed.len(), 1);
        assert!(is_claim_active(&config, claimed[0].id).unwrap());

        remove_job_for_agent(&config, &job.id, "owner-agent").unwrap();
        assert!(!is_claim_active(&config, claimed[0].id).unwrap());
        assert!(
            pending(&config, Utc::now() + Duration::hours(2), 10)
                .unwrap()
                .is_empty()
        );
    }
}
