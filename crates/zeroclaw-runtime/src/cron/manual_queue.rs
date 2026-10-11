use anyhow::{Context, Result, bail};
use rusqlite::{OptionalExtension, params};
use zeroclaw_config::schema::Config;

use super::store::with_initialized_connection;

pub(super) fn initialize_schema(conn: &rusqlite::Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS cron_manual_requests (
            id TEXT PRIMARY KEY,
            job_id TEXT NOT NULL UNIQUE,
            agent_alias TEXT NOT NULL,
            approved INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL,
            started_at TEXT
        );",
    )
    .context("Failed to initialize manual cron request queue")
}

/// A durable request to run one existing cron job from its owning agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManualRequest {
    pub id: String,
    pub job_id: String,
    pub agent_alias: String,
    pub approved: bool,
}

/// Queue one manual run. Repeating the same request is idempotent while it is
/// pending; a different agent or approval value cannot take over that request.
pub fn enqueue(config: &Config, job_id: &str, agent_alias: &str, approved: bool) -> Result<String> {
    let request_id = uuid::Uuid::new_v4().to_string();
    with_initialized_connection(config, |conn| {
        let tx = conn
            .unchecked_transaction()
            .context("Failed to begin manual cron enqueue transaction")?;
        let owns_job = tx
            .query_row(
                "SELECT 1 FROM cron_jobs WHERE id = ?1 AND agent_alias = ?2",
                params![job_id, agent_alias],
                |_| Ok(()),
            )
            .optional()
            .context("Failed to validate manual cron job ownership")?
            .is_some();
        if !owns_job {
            bail!("cron job '{job_id}' not found for agent '{agent_alias}'");
        }
        tx.execute(
            "INSERT INTO cron_manual_requests (id, job_id, agent_alias, approved, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(job_id) DO NOTHING",
            params![
                request_id,
                job_id,
                agent_alias,
                if approved { 1_i64 } else { 0_i64 },
                chrono::Utc::now().to_rfc3339(),
            ],
        )
        .context("Failed to enqueue manual cron request")?;

        let (existing_id, existing_agent, existing_approved, started_at): (
            String,
            String,
            bool,
            Option<String>,
        ) = tx
            .query_row(
                "SELECT id, agent_alias, approved, started_at
                 FROM cron_manual_requests WHERE job_id = ?1",
                [job_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .context("Manual cron request disappeared after enqueue")?;
        if existing_agent != agent_alias {
            bail!("a manual run for this job is already queued by another agent");
        }
        if existing_approved != approved {
            bail!("a manual run for this job is already queued with different approval");
        }
        if started_at.is_some() {
            bail!("a manual run for this job has already started");
        }
        tx.commit()
            .context("Failed to commit manual cron enqueue transaction")?;
        Ok(existing_id)
    })
}

/// Read queued requests without claiming their jobs. The scheduler owns claims.
pub fn pending(config: &Config, limit: usize) -> Result<Vec<ManualRequest>> {
    with_initialized_connection(config, |conn| {
        let mut statement = conn
            .prepare(
                "SELECT id, job_id, agent_alias, approved
                 FROM cron_manual_requests
                 WHERE started_at IS NULL
                 ORDER BY created_at, id
                 LIMIT ?1",
            )
            .context("Failed to prepare manual cron request query")?;
        let rows = statement
            .query_map([i64::try_from(limit).unwrap_or(i64::MAX)], |row| {
                Ok(ManualRequest {
                    id: row.get(0)?,
                    job_id: row.get(1)?,
                    agent_alias: row.get(2)?,
                    approved: row.get(3)?,
                })
            })
            .context("Failed to read manual cron requests")?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("Failed to decode manual cron requests")
    })
}

/// Claim the owning job and mark its manual request started in one transaction.
/// A failed admission leaves both the request and job available for retry.
pub fn mark_started(config: &Config, id: &str) -> Result<bool> {
    with_initialized_connection(config, |conn| {
        let tx = conn.unchecked_transaction()?;
        let request: Option<(String, String)> = tx.query_row(
            "SELECT job_id, agent_alias FROM cron_manual_requests WHERE id=?1 AND started_at IS NULL",
            [id], |row| Ok((row.get(0)?, row.get(1)?))).optional()?;
        let Some((job_id, agent_alias)) = request else {
            return Ok(false);
        };
        let now = chrono::Utc::now().to_rfc3339();
        let claimed = tx.execute(
            "UPDATE cron_jobs SET locked_at=?1 WHERE id=?2 AND agent_alias=?3 AND locked_at IS NULL",
            params![now, job_id, agent_alias])?;
        if claimed == 0 {
            return Ok(false);
        }
        tx.execute(
            "UPDATE cron_manual_requests SET started_at=?2 WHERE id=?1",
            params![id, now],
        )?;
        tx.commit().context("Failed to admit background cron run")?;
        Ok(true)
    })
}

/// Settle a captured result's request and release its job claim atomically.
/// Retrying a committed settlement cannot release a newer claim.
pub(crate) fn settle(config: &Config, request: &ManualRequest) -> Result<()> {
    with_initialized_connection(config, |conn| {
        let tx = conn.unchecked_transaction()?;
        let removed = tx.execute(
            "DELETE FROM cron_manual_requests WHERE id=?1 AND job_id=?2 AND agent_alias=?3",
            params![request.id, request.job_id, request.agent_alias],
        )?;
        if removed == 1 {
            tx.execute("UPDATE cron_jobs SET locked_at=NULL, lock_token=NULL WHERE id=?1 AND agent_alias=?2",
                params![request.job_id, request.agent_alias])?;
        }
        tx.commit().context("Failed to settle background cron run")
    })
}

/// Requests started before the previous process exited. The caller should
/// review these as missed work and settle them without rerunning the action.
pub fn interrupted(config: &Config, limit: usize) -> Result<Vec<ManualRequest>> {
    with_initialized_connection(config, |conn| {
        let mut statement = conn
            .prepare(
                "SELECT id, job_id, agent_alias, approved
                 FROM cron_manual_requests
                 WHERE started_at IS NOT NULL
                 ORDER BY started_at, id
                 LIMIT ?1",
            )
            .context("Failed to prepare interrupted manual cron request query")?;
        let rows = statement
            .query_map([i64::try_from(limit).unwrap_or(i64::MAX)], |row| {
                Ok(ManualRequest {
                    id: row.get(0)?,
                    job_id: row.get(1)?,
                    agent_alias: row.get(2)?,
                    approved: row.get(3)?,
                })
            })
            .context("Failed to read interrupted manual cron requests")?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("Failed to decode interrupted manual cron requests")
    })
}

/// Whether the canonical cron run record shows this request's run completed.
/// Compare timestamps inside SQLite so RFC3339 offsets and fractional seconds
/// are normalized consistently across the two stored values.
pub fn settled(config: &Config, request: &ManualRequest) -> Result<bool> {
    with_initialized_connection(config, |conn| {
        let started_at: Option<String> = conn
            .query_row(
                "SELECT started_at FROM cron_manual_requests WHERE id = ?1 AND job_id = ?2",
                params![request.id, request.job_id],
                |row| row.get(0),
            )
            .optional()
            .context("Failed to read manual cron request start time")?
            .flatten();
        let Some(started_at) = started_at else {
            return Ok(false);
        };
        let completed: Option<bool> = conn
            .query_row(
                "SELECT julianday(last_run) >= julianday(?2)
                 FROM cron_jobs WHERE id = ?1 AND last_run IS NOT NULL",
                params![request.job_id, started_at],
                |row| row.get(0),
            )
            .optional()
            .context("Failed to compare manual cron request with last run")?;
        Ok(completed.unwrap_or(false))
    })
}

/// Cancel queued runs when a job is paused or removed. A run whose start has
/// been recorded may already have side effects and is left for normal settling.
pub(super) fn cancel_pending_on(
    conn: &rusqlite::Connection,
    job_id: &str,
    agent_alias: Option<&str>,
) -> Result<()> {
    if let Some(agent_alias) = agent_alias {
        conn.execute(
            "DELETE FROM cron_manual_requests
             WHERE job_id = ?1 AND agent_alias = ?2 AND started_at IS NULL",
            params![job_id, agent_alias],
        )?;
    } else {
        conn.execute(
            "DELETE FROM cron_manual_requests WHERE job_id = ?1 AND started_at IS NULL",
            [job_id],
        )?;
    }
    Ok(())
}

/// Remove a request only after the scheduler has settled its run.
pub fn complete(config: &Config, id: &str) -> Result<()> {
    with_initialized_connection(config, |conn| {
        conn.execute("DELETE FROM cron_manual_requests WHERE id = ?1", [id])
            .context("Failed to complete manual cron request")?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn test_config(tmp: &TempDir) -> Config {
        Config {
            data_dir: tmp.path().join("data"),
            config_path: tmp.path().join("config.toml"),
            ..Config::default()
        }
    }

    fn add_test_job(config: &Config, agent_alias: &str) -> String {
        super::super::add_agent_job(
            config,
            agent_alias,
            None,
            super::super::Schedule::Cron {
                expr: "0 8 * * *".into(),
                tz: None,
            },
            "do the work",
            super::super::SessionTarget::Isolated,
            None,
            None,
            false,
            None,
            true,
        )
        .unwrap()
        .id
    }

    #[test]
    fn requests_are_durable_idempotent_and_completed_by_id() {
        let tmp = TempDir::new().unwrap();
        let config = test_config(&tmp);
        let job_id = add_test_job(&config, "agent-a");
        let id = enqueue(&config, &job_id, "agent-a", true).unwrap();

        assert_eq!(enqueue(&config, &job_id, "agent-a", true).unwrap(), id);
        let reopened = test_config(&tmp);
        assert_eq!(
            pending(&reopened, 10).unwrap(),
            vec![ManualRequest {
                id: id.clone(),
                job_id,
                agent_alias: "agent-a".into(),
                approved: true,
            }]
        );

        complete(&reopened, &id).unwrap();
        assert!(pending(&config, 10).unwrap().is_empty());
    }

    #[test]
    fn enqueue_requires_job_ownership_and_started_requests_are_not_pending() {
        let tmp = TempDir::new().unwrap();
        let config = test_config(&tmp);
        let job_id = add_test_job(&config, "agent-a");

        assert!(enqueue(&config, &job_id, "agent-b", false).is_err());
        let id = enqueue(&config, &job_id, "agent-a", false).unwrap();
        assert!(mark_started(&config, &id).unwrap());
        assert!(!mark_started(&config, &id).unwrap());
        assert!(pending(&config, 10).unwrap().is_empty());
        assert_eq!(interrupted(&config, 10).unwrap()[0].id, id);
    }

    #[test]
    fn pending_request_cannot_be_reassigned_or_change_approval() {
        let tmp = TempDir::new().unwrap();
        let config = test_config(&tmp);
        let job_id = add_test_job(&config, "agent-a");
        enqueue(&config, &job_id, "agent-a", false).unwrap();

        assert!(enqueue(&config, &job_id, "agent-b", false).is_err());
        assert!(enqueue(&config, &job_id, "agent-a", true).is_err());
        assert_eq!(pending(&config, 10).unwrap()[0].agent_alias, "agent-a");
    }

    #[test]
    fn settled_ignores_an_older_run_and_recognizes_the_requested_run() {
        let tmp = TempDir::new().unwrap();
        let config = test_config(&tmp);
        let job_id = add_test_job(&config, "agent-a");
        let id = enqueue(&config, &job_id, "agent-a", false).unwrap();
        assert!(mark_started(&config, &id).unwrap());
        let request = interrupted(&config, 10).unwrap().remove(0);

        with_initialized_connection(&config, |conn| {
            let started_at: String = conn.query_row(
                "SELECT started_at FROM cron_manual_requests WHERE id = ?1",
                [&id],
                |row| row.get(0),
            )?;
            conn.execute(
                "UPDATE cron_jobs SET last_run = datetime(julianday(?2) - 1 / 86400.0)
                 WHERE id = ?1",
                params![job_id, started_at],
            )?;
            Ok(())
        })
        .unwrap();
        assert!(!settled(&config, &request).unwrap());

        with_initialized_connection(&config, |conn| {
            let started_at: String = conn.query_row(
                "SELECT started_at FROM cron_manual_requests WHERE id = ?1",
                [&id],
                |row| row.get(0),
            )?;
            conn.execute(
                "UPDATE cron_jobs SET last_run = ?2 WHERE id = ?1",
                params![job_id, started_at],
            )?;
            Ok(())
        })
        .unwrap();
        assert!(settled(&config, &request).unwrap());
    }
    #[test]
    fn failed_admission_does_not_leave_job_claimed() {
        let tmp = tempfile::TempDir::new().unwrap();
        let config = test_config(&tmp);
        let job = crate::cron::store::add_job(&config, "owner-agent", "*/5 * * * *", "echo test")
            .unwrap();
        let request = enqueue(&config, &job.id, "owner-agent", false).unwrap();
        with_initialized_connection(&config, |conn| {
            conn.execute_batch("CREATE TRIGGER block_start BEFORE UPDATE ON cron_manual_requests BEGIN SELECT RAISE(ABORT, 'start unavailable'); END;")?;
            Ok(())
        }).unwrap();
        assert!(mark_started(&config, &request).is_err());
        assert_eq!(pending(&config, 10).unwrap().len(), 1);
        assert!(crate::cron::claim_job(&config, &job.id, chrono::Utc::now()).unwrap());
        crate::cron::release_job(&config, &job.id).unwrap();
        with_initialized_connection(&config, |conn| {
            conn.execute_batch("DROP TRIGGER block_start;")?;
            Ok(())
        })
        .unwrap();
        assert!(mark_started(&config, &request).unwrap());
        assert!(pending(&config, 10).unwrap().is_empty());
    }
    #[test]
    fn settlement_rolls_back_on_failure_and_cannot_unlock_a_new_run() {
        let tmp = TempDir::new().unwrap();
        let config = test_config(&tmp);
        let job = crate::cron::store::add_job(&config, "owner-agent", "*/5 * * * *", "echo test")
            .unwrap();
        enqueue(&config, &job.id, "owner-agent", false).unwrap();
        let request = pending(&config, 10).unwrap().remove(0);
        assert!(mark_started(&config, &request.id).unwrap());
        with_initialized_connection(&config, |conn| {
            conn.execute_batch("CREATE TRIGGER block_settle BEFORE UPDATE OF locked_at ON cron_jobs BEGIN SELECT RAISE(ABORT, 'settlement unavailable'); END;")?;
            Ok(())
        }).unwrap();
        assert!(settle(&config, &request).is_err());
        assert_eq!(interrupted(&config, 10).unwrap().len(), 1);
        assert!(!crate::cron::claim_job(&config, &job.id, chrono::Utc::now()).unwrap());
        with_initialized_connection(&config, |conn| {
            conn.execute_batch("DROP TRIGGER block_settle;")?;
            Ok(())
        })
        .unwrap();
        settle(&config, &request).unwrap();
        assert!(interrupted(&config, 10).unwrap().is_empty());
        assert!(crate::cron::claim_job(&config, &job.id, chrono::Utc::now()).unwrap());
        settle(&config, &request).unwrap();
        assert!(!crate::cron::claim_job(&config, &job.id, chrono::Utc::now()).unwrap());
        crate::cron::release_job(&config, &job.id).unwrap();
    }
}
