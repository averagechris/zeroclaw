//! Durable owner-controlled Telegram membership and invitation state.
//!
//! Membership rows are the canonical authorization source. Agent aliases are
//! derived from a membership at use time and are never stored in this database.

use anyhow::{Context, Result, bail};
use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::time::Duration;
use uuid::Uuid;

const INVITE_TTL_SECONDS: i64 = 24 * 60 * 60;
const MAX_ACTIVE_MEMBERSHIPS: i64 = 512;
const MAX_OUTSTANDING_INVITES: i64 = 128;
const SQLITE_BUSY_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MembershipKind {
    Private,
    Group,
}

impl MembershipKind {
    fn from_db_value(value: &str) -> rusqlite::Result<Self> {
        match value {
            "private" => Ok(Self::Private),
            "group" => Ok(Self::Group),
            _ => Err(rusqlite::Error::InvalidColumnType(
                1,
                "kind".to_string(),
                rusqlite::types::Type::Text,
            )),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Membership {
    pub chat_id: i64,
    pub kind: MembershipKind,
}

impl Membership {
    /// Derive the stable per-chat agent identity from the channel alias.
    pub(crate) fn agent_alias(&self, channel_alias: &str) -> String {
        match self.kind {
            MembershipKind::Private => {
                format!("tg_{channel_alias}_dm_{}", self.chat_id)
            }
            MembershipKind::Group => {
                format!("tg_{channel_alias}_group_{}", self.chat_id.unsigned_abs())
            }
        }
    }
}

/// A handle to the canonical SQLite membership store for one Telegram alias.
/// The connection contains no cached membership state; all reads query SQLite.
pub(crate) struct MembershipStore {
    conn: Mutex<Connection>,
}

impl MembershipStore {
    pub(crate) fn open(data_dir: &Path, alias: &str) -> Result<Self> {
        validate_channel_alias(alias)?;

        let store_dir = data_dir.join("telegram-memberships");
        create_private_dir(&store_dir)?;
        let db_path = store_dir.join(format!("{alias}.sqlite3"));
        prepare_private_db_file(&db_path)?;

        let conn =
            Connection::open(&db_path).context("Failed to open Telegram membership store")?;
        conn.busy_timeout(SQLITE_BUSY_TIMEOUT)
            .context("Failed to configure Telegram membership store busy timeout")?;
        conn.execute_batch(
            "PRAGMA foreign_keys = ON;
             PRAGMA journal_mode = DELETE;
             PRAGMA synchronous = FULL;
             CREATE TABLE IF NOT EXISTS telegram_memberships (
                 chat_id INTEGER PRIMARY KEY,
                 kind TEXT NOT NULL CHECK (kind IN ('private', 'group')),
                 active INTEGER NOT NULL CHECK (active IN (0, 1)),
                 created_at INTEGER NOT NULL,
                 updated_at INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS telegram_invites (
                 token_hash TEXT PRIMARY KEY,
                 expires_at INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS telegram_invites_expiry
                 ON telegram_invites(expires_at);",
        )
        .context("Failed to initialize Telegram membership store")?;

        // SQLite preserves the file's mode when reopening an existing DB. Set
        // it after initialization as well, and fail closed if this is denied.
        set_private_file_mode(&db_path)?;

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Create a single-use 24-hour invitation. The returned token is the only
    /// plaintext copy; only its SHA-256 digest is persisted.
    pub(crate) fn create_invite(&self, now: i64) -> Result<String> {
        let expires_at = now
            .checked_add(INVITE_TTL_SECONDS)
            .context("Invitation expiry timestamp is out of range")?;
        let token = Uuid::new_v4().simple().to_string();
        let token_hash = hash_token(&token);

        let mut conn = self.conn.lock();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .context("Failed to begin invitation creation")?;
        tx.execute("DELETE FROM telegram_invites WHERE expires_at <= ?1", [now])
            .context("Failed to remove expired Telegram invitations")?;
        let outstanding: i64 = tx
            .query_row("SELECT COUNT(*) FROM telegram_invites", [], |row| {
                row.get(0)
            })
            .context("Failed to count Telegram invitations")?;
        if outstanding >= MAX_OUTSTANDING_INVITES {
            bail!("Too many outstanding Telegram invitations");
        }
        tx.execute(
            "INSERT INTO telegram_invites(token_hash, expires_at) VALUES (?1, ?2)",
            params![token_hash, expires_at],
        )
        .context("Failed to store Telegram invitation")?;
        tx.commit()
            .context("Failed to commit Telegram invitation")?;
        Ok(token)
    }

    /// Redeem an invitation for a private chat. Failed, expired, and replayed
    /// tokens deliberately share one generic error and reveal no token data.
    pub(crate) fn redeem(&self, token: &str, user_id: i64, now: i64) -> Result<Membership> {
        validate_private_chat_id(user_id)?;
        let token_hash = hash_token(token);
        let mut conn = self.conn.lock();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .context("Failed to begin Telegram invitation redemption")?;

        let expiry: Option<i64> = tx
            .query_row(
                "SELECT expires_at FROM telegram_invites WHERE token_hash = ?1",
                [&token_hash],
                |row| row.get(0),
            )
            .optional()
            .context("Failed to read Telegram invitation")?;
        let Some(expiry) = expiry.filter(|expires_at| *expires_at > now) else {
            bail!("Invalid or expired Telegram invitation");
        };

        let existing_active: Option<i64> = tx
            .query_row(
                "SELECT active FROM telegram_memberships WHERE chat_id = ?1",
                [user_id],
                |row| row.get(0),
            )
            .optional()
            .context("Failed to read Telegram membership")?;
        if existing_active == Some(1) {
            bail!("Telegram user is already an active member");
        }
        ensure_active_capacity(&tx, existing_active == Some(1))?;

        // All membership changes and token consumption commit together. A
        // constraint or I/O error leaves the invitation available to retry.
        tx.execute(
            "INSERT INTO telegram_memberships(chat_id, kind, active, created_at, updated_at)
             VALUES (?1, 'private', 1, ?2, ?2)
             ON CONFLICT(chat_id) DO UPDATE SET active = 1, updated_at = excluded.updated_at",
            params![user_id, now],
        )
        .context("Failed to activate Telegram membership")?;
        let consumed = tx
            .execute(
                "DELETE FROM telegram_invites WHERE token_hash = ?1 AND expires_at = ?2",
                params![token_hash, expiry],
            )
            .context("Failed to consume Telegram invitation")?;
        if consumed != 1 {
            bail!("Invalid or expired Telegram invitation");
        }
        tx.commit()
            .context("Failed to commit Telegram invitation redemption")?;

        Ok(Membership {
            chat_id: user_id,
            kind: MembershipKind::Private,
        })
    }

    /// Explicitly activate a group membership while preserving any previous
    /// row, so revoking and later re-activating a group retains its identity.
    pub(crate) fn activate_group(&self, chat_id: i64, now: i64) -> Result<Membership> {
        validate_group_chat_id(chat_id)?;
        let mut conn = self.conn.lock();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .context("Failed to begin Telegram group activation")?;
        let existing: Option<i64> = tx
            .query_row(
                "SELECT active FROM telegram_memberships WHERE chat_id = ?1",
                [chat_id],
                |row| row.get(0),
            )
            .optional()
            .context("Failed to read Telegram group membership")?;
        ensure_active_capacity(&tx, existing == Some(1))?;
        tx.execute(
            "INSERT INTO telegram_memberships(chat_id, kind, active, created_at, updated_at)
             VALUES (?1, 'group', 1, ?2, ?2)
             ON CONFLICT(chat_id) DO UPDATE SET kind = 'group', active = 1, updated_at = excluded.updated_at",
            params![chat_id, now],
        )
        .context("Failed to activate Telegram group membership")?;
        tx.commit()
            .context("Failed to commit Telegram group activation")?;
        Ok(Membership {
            chat_id,
            kind: MembershipKind::Group,
        })
    }

    pub(crate) fn membership(&self, chat_id: i64) -> Result<Option<Membership>> {
        self.conn
            .lock()
            .query_row(
                "SELECT chat_id, kind FROM telegram_memberships WHERE chat_id = ?1 AND active = 1",
                [chat_id],
                membership_from_row,
            )
            .optional()
            .context("Failed to read Telegram membership")
    }

    pub(crate) fn list(&self) -> Result<Vec<Membership>> {
        let conn = self.conn.lock();
        let mut statement = conn
            .prepare(
                "SELECT chat_id, kind FROM telegram_memberships WHERE active = 1 ORDER BY chat_id",
            )
            .context("Failed to prepare Telegram membership listing")?;
        let rows = statement
            .query_map([], membership_from_row)
            .context("Failed to list Telegram memberships")?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("Failed to read Telegram membership listing")
    }

    pub(crate) fn revoke(&self, chat_id: i64) -> Result<bool> {
        validate_membership_chat_id(chat_id)?;
        let mut conn = self.conn.lock();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .context("Failed to begin Telegram membership revocation")?;
        let changed = tx
            .execute(
                "UPDATE telegram_memberships SET active = 0 WHERE chat_id = ?1 AND active = 1",
                [chat_id],
            )
            .context("Failed to revoke Telegram membership")?;
        tx.commit()
            .context("Failed to commit Telegram membership revocation")?;
        Ok(changed == 1)
    }
}

fn membership_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Membership> {
    let chat_id = row.get(0)?;
    let kind = MembershipKind::from_db_value(row.get::<_, String>(1)?.as_str())?;
    Ok(Membership { chat_id, kind })
}

fn ensure_active_capacity(tx: &rusqlite::Transaction<'_>, reactivating: bool) -> Result<()> {
    if reactivating {
        return Ok(());
    }
    let active: i64 = tx
        .query_row(
            "SELECT COUNT(*) FROM telegram_memberships WHERE active = 1",
            [],
            |row| row.get(0),
        )
        .context("Failed to count active Telegram memberships")?;
    if active >= MAX_ACTIVE_MEMBERSHIPS {
        bail!("Telegram membership limit reached");
    }
    Ok(())
}

fn hash_token(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        // Writing to a String is infallible.
        let _ = write!(output, "{byte:02x}");
    }
    output
}

fn validate_channel_alias(alias: &str) -> Result<()> {
    if alias.is_empty()
        || !alias
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    {
        bail!("Invalid Telegram channel alias");
    }
    Ok(())
}

fn validate_private_chat_id(chat_id: i64) -> Result<()> {
    if chat_id <= 0 {
        bail!("Telegram private chat ID must be positive");
    }
    Ok(())
}

fn validate_group_chat_id(chat_id: i64) -> Result<()> {
    if chat_id >= 0 || chat_id == i64::MIN {
        bail!("Telegram group chat ID must be negative and representable");
    }
    Ok(())
}

fn validate_membership_chat_id(chat_id: i64) -> Result<()> {
    if chat_id == 0 || chat_id == i64::MIN {
        bail!("Invalid Telegram membership chat ID");
    }
    Ok(())
}

#[cfg(unix)]
fn create_private_dir(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    std::fs::create_dir_all(path).context("Failed to create Telegram membership directory")?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
        .context("Failed to secure Telegram membership directory")
}

#[cfg(not(unix))]
fn create_private_dir(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path).context("Failed to create Telegram membership directory")
}

#[cfg(unix)]
fn prepare_private_db_file(path: &Path) -> Result<()> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    if let Ok(metadata) = std::fs::symlink_metadata(path)
        && (metadata.file_type().is_symlink() || !metadata.file_type().is_file())
    {
        bail!("Telegram membership database path is not a regular file");
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .open(path)
        .context("Failed to create Telegram membership database file")?;
    file.set_permissions(std::fs::Permissions::from_mode(0o600))
        .context("Failed to secure Telegram membership database file")
}

#[cfg(not(unix))]
fn prepare_private_db_file(path: &Path) -> Result<()> {
    std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .open(path)
        .context("Failed to create Telegram membership database file")?;
    Ok(())
}

#[cfg(unix)]
fn set_private_file_mode(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .context("Failed to secure Telegram membership database file")
}

#[cfg(not(unix))]
fn set_private_file_mode(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::{Arc, Barrier};

    fn temp_data_dir() -> PathBuf {
        std::env::temp_dir().join(format!("telegram-memberships-test-{}", Uuid::new_v4()))
    }

    fn open_test_store(path: &Path) -> MembershipStore {
        MembershipStore::open(path, "bot_main").expect("store should open")
    }

    #[test]
    fn membership_and_invite_state_survive_restart() {
        let dir = temp_data_dir();
        let token = {
            let store = open_test_store(&dir);
            store.create_invite(10).expect("invite should be created")
        };
        {
            let store = open_test_store(&dir);
            let membership = store
                .redeem(&token, 1234, 11)
                .expect("invite should redeem");
            assert_eq!(membership.kind, MembershipKind::Private);
        }
        let store = open_test_store(&dir);
        assert_eq!(
            store.membership(1234).expect("lookup should work"),
            Some(Membership {
                chat_id: 1234,
                kind: MembershipKind::Private,
            })
        );
        drop(store);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn invite_is_single_use_and_parallel_redemption_has_one_winner() {
        let dir = temp_data_dir();
        let store = Arc::new(open_test_store(&dir));
        let token = store.create_invite(10).expect("invite should be created");
        let barrier = Arc::new(Barrier::new(3));
        let mut joins = Vec::new();
        for user_id in [1234, 5678] {
            let token = token.clone();
            let barrier = Arc::clone(&barrier);
            let store = Arc::clone(&store);
            joins.push(std::thread::spawn(move || {
                barrier.wait();
                store.redeem(&token, user_id, 11).is_ok()
            }));
        }
        barrier.wait();
        let outcomes = joins
            .into_iter()
            .map(|join| join.join().expect("redeem thread should complete"))
            .collect::<Vec<_>>();
        assert_eq!(outcomes.iter().filter(|result| **result).count(), 1);
        let winner = [1234, 5678]
            .into_iter()
            .find(|user_id| {
                store
                    .membership(*user_id)
                    .expect("lookup should work")
                    .is_some()
            })
            .expect("one user should be active");
        assert!(
            store
                .redeem(&token, if winner == 1234 { 5678 } else { 1234 }, 12)
                .is_err()
        );
        drop(store);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn invitation_expires_at_the_deadline_and_errors_are_generic() {
        let dir = temp_data_dir();
        let store = open_test_store(&dir);
        let token = store.create_invite(10).expect("invite should be created");
        let error = store
            .redeem(&token, 1234, 10 + INVITE_TTL_SECONDS)
            .expect_err("invite should expire at its deadline");
        assert_eq!(error.to_string(), "Invalid or expired Telegram invitation");
        assert!(!error.to_string().contains(&token));
        drop(store);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn revoked_memberships_reactivate_with_the_same_identity() {
        let dir = temp_data_dir();
        let store = open_test_store(&dir);
        let first = store.activate_group(-100123, 10).expect("group activates");
        let alias = first.agent_alias("main");
        assert!(store.revoke(-100123).expect("revoke should work"));
        assert_eq!(store.membership(-100123).expect("lookup works"), None);
        let reactivated = store
            .activate_group(-100123, 12)
            .expect("group reactivates");
        assert_eq!(reactivated.agent_alias("main"), alias);
        assert!(store.revoke(-100123).expect("revoke reactivated group"));
        assert!(!store.revoke(-100123).expect("revoke is idempotent"));
        drop(store);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn database_write_failures_are_returned() {
        let dir = temp_data_dir();
        let store = open_test_store(&dir);
        store
            .conn
            .lock()
            .execute_batch(
                "CREATE TRIGGER reject_invite BEFORE INSERT ON telegram_invites
                 BEGIN SELECT RAISE(ABORT, 'forced write failure'); END;",
            )
            .expect("test trigger should be created");
        let error = store
            .create_invite(10)
            .expect_err("database rejection should propagate");
        assert!(
            error
                .to_string()
                .contains("Failed to store Telegram invitation")
        );
        drop(store);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[test]
    fn database_directory_and_file_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp_data_dir();
        let store = open_test_store(&dir);
        let membership_dir = dir.join("telegram-memberships");
        let db_path = membership_dir.join("bot_main.sqlite3");
        assert_eq!(
            std::fs::metadata(&membership_dir)
                .expect("directory metadata")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(&db_path)
                .expect("database metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        drop(store);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn rejects_invalid_aliases_and_chat_ids() {
        let dir = temp_data_dir();
        assert!(MembershipStore::open(&dir, "../bot").is_err());
        let store = open_test_store(&dir);
        assert!(store.activate_group(123, 1).is_err());
        assert!(store.activate_group(i64::MIN, 1).is_err());
        let token = store.create_invite(1).expect("invite should be created");
        assert!(store.redeem(&token, -123, 2).is_err());
        drop(store);
        let _ = std::fs::remove_dir_all(dir);
    }
}
