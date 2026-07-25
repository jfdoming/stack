use std::path::Path;

use anyhow::{Context, Result, anyhow};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

const SCHEMA_VERSION: i64 = 7;

#[derive(Debug, Clone)]
pub struct BranchRecord {
    pub id: i64,
    pub name: String,
    pub parent_branch_id: Option<i64>,
    pub last_synced_head_sha: Option<String>,
    pub cached_pr_number: Option<i64>,
    pub cached_pr_state: Option<String>,
    pub cached_pr_head_oid: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RepoMeta {
    pub base_branch: String,
    pub base_branch_source: BaseBranchSource,
    pub base_remote: Option<String>,
    pub push_target: Option<String>,
    pub canonical_repo: Option<String>,
    pub fork_repo: Option<String>,
    pub push_permission: Option<String>,
    pub permission_checked_at: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BaseBranchSource {
    RemoteHead,
    LocalConvention,
    CurrentBranch,
    FirstLocal,
    Default,
    Legacy,
}

impl BaseBranchSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RemoteHead => "remote_head",
            Self::LocalConvention => "local_convention",
            Self::CurrentBranch => "current_branch",
            Self::FirstLocal => "first_local",
            Self::Default => "default",
            Self::Legacy => "legacy",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "remote_head" => Some(Self::RemoteHead),
            "local_convention" => Some(Self::LocalConvention),
            "current_branch" => Some(Self::CurrentBranch),
            "first_local" => Some(Self::FirstLocal),
            "default" => Some(Self::Default),
            "legacy" => Some(Self::Legacy),
            _ => None,
        }
    }

    fn can_yield_to_remote_head(self) -> bool {
        matches!(
            self,
            Self::LocalConvention
                | Self::CurrentBranch
                | Self::FirstLocal
                | Self::Default
                | Self::Legacy
        )
    }
}

#[derive(Debug, Clone)]
pub struct ParentUpdate {
    pub child_name: String,
    pub parent_name: Option<String>,
}

#[derive(Debug, Clone)]
pub struct MoveTopologyChange {
    target_id: i64,
    target_name: String,
    previous_parent_id: Option<i64>,
    previous_parent_name: Option<String>,
    target_was_inserted: bool,
    parent_id: i64,
    parent_name: String,
    parent_was_inserted: bool,
    target_topology_revision: i64,
    parent_topology_revision: i64,
    previous_parent_topology_revision: Option<i64>,
}

impl MoveTopologyChange {
    pub fn previous_parent_name(&self) -> Option<&str> {
        self.previous_parent_name.as_deref()
    }
}

struct MoveRollbackBranchState {
    name: String,
    parent_branch_id: Option<i64>,
    last_synced_head_sha: Option<String>,
    cached_pr_number: Option<i64>,
    cached_pr_state: Option<String>,
    cached_pr_head_oid: Option<String>,
    topology_revision: i64,
}

impl MoveRollbackBranchState {
    fn has_sync_or_pr_metadata(&self) -> bool {
        self.last_synced_head_sha.is_some()
            || self.cached_pr_number.is_some()
            || self.cached_pr_state.is_some()
            || self.cached_pr_head_oid.is_some()
    }
}

pub struct Database {
    conn: Connection,
}

impl Database {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)
            .with_context(|| format!("failed to open sqlite at {}", path.display()))?;
        let mut db = Self { conn };
        db.migrate()?;
        Ok(db)
    }

    fn migrate(&mut self) -> Result<()> {
        self.conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS branches (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL UNIQUE,
                parent_branch_id INTEGER NULL,
                last_synced_head_sha TEXT NULL,
                cached_pr_number INTEGER NULL,
                cached_pr_state TEXT NULL,
                cached_pr_head_oid TEXT NULL,
                topology_revision INTEGER NOT NULL DEFAULT 0
                    CHECK (topology_revision >= 0 AND typeof(topology_revision) = 'integer'),
                created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                FOREIGN KEY(parent_branch_id) REFERENCES branches(id) ON DELETE SET NULL
            );
            CREATE TABLE IF NOT EXISTS repo_meta (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                base_branch TEXT NOT NULL,
                base_branch_source TEXT NOT NULL DEFAULT 'legacy',
                base_remote TEXT NULL,
                schema_version INTEGER NOT NULL,
                push_target TEXT NULL,
                canonical_repo TEXT NULL,
                fork_repo TEXT NULL,
                push_permission TEXT NULL,
                permission_checked_at INTEGER NULL
            );
            CREATE TABLE IF NOT EXISTS sync_runs (
                id INTEGER PRIMARY KEY,
                started_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                finished_at TEXT NULL,
                status TEXT NOT NULL,
                summary_json TEXT NULL
            );
            CREATE TABLE IF NOT EXISTS topology_state (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                revision INTEGER NOT NULL
                    CHECK (revision >= 0 AND typeof(revision) = 'integer')
            );
            INSERT INTO topology_state(id, revision)
            VALUES (1, 0)
            ON CONFLICT(id) DO NOTHING;
            ",
        )?;
        if !Self::branches_has_column(&tx, "cached_pr_head_oid")? {
            tx.execute(
                "ALTER TABLE branches ADD COLUMN cached_pr_head_oid TEXT NULL",
                [],
            )?;
        }
        if !Self::branches_has_column(&tx, "topology_revision")? {
            tx.execute(
                "ALTER TABLE branches
                 ADD COLUMN topology_revision INTEGER NOT NULL DEFAULT 0
                     CHECK (topology_revision >= 0 AND typeof(topology_revision) = 'integer')",
                [],
            )?;
        }
        tx.execute_batch(
            "DROP TRIGGER IF EXISTS branches_topology_revision_after_insert;
             DROP TRIGGER IF EXISTS branches_topology_revision_after_delete;
             DROP TRIGGER IF EXISTS branches_topology_revision_after_update;
             CREATE TRIGGER branches_topology_revision_after_insert
             AFTER INSERT ON branches
             BEGIN
                 UPDATE topology_state SET revision = revision + 1 WHERE id = 1;
                 UPDATE branches
                 SET topology_revision = (
                     SELECT revision FROM topology_state WHERE id = 1
                 )
                 WHERE id = NEW.id;
             END;
             CREATE TRIGGER branches_topology_revision_after_delete
             AFTER DELETE ON branches
             BEGIN
                 UPDATE topology_state SET revision = revision + 1 WHERE id = 1;
             END;
             CREATE TRIGGER branches_topology_revision_after_update
             AFTER UPDATE OF name, parent_branch_id ON branches
             WHEN OLD.name IS NOT NEW.name
                  OR OLD.parent_branch_id IS NOT NEW.parent_branch_id
             BEGIN
                 UPDATE topology_state SET revision = revision + 1 WHERE id = 1;
                 UPDATE branches
                 SET topology_revision = (
                     SELECT revision FROM topology_state WHERE id = 1
                 )
                 WHERE id = NEW.id;
             END;",
        )?;
        for (name, ty) in [
            ("base_branch_source", "TEXT NOT NULL DEFAULT 'legacy'"),
            ("base_remote", "TEXT NULL"),
            ("push_target", "TEXT NULL"),
            ("canonical_repo", "TEXT NULL"),
            ("fork_repo", "TEXT NULL"),
            ("push_permission", "TEXT NULL"),
            ("permission_checked_at", "INTEGER NULL"),
        ] {
            if !Self::repo_meta_has_column(&tx, name)? {
                tx.execute(&format!("ALTER TABLE repo_meta ADD COLUMN {name} {ty}"), [])?;
            }
        }
        tx.execute(
            "UPDATE repo_meta
             SET base_remote = 'origin'
             WHERE id = 1 AND base_branch_source = 'remote_head' AND base_remote IS NULL",
            [],
        )?;
        tx.execute(
            "UPDATE repo_meta SET schema_version = ?1 WHERE id = 1",
            params![SCHEMA_VERSION],
        )?;
        tx.commit()?;
        Ok(())
    }

    fn repo_meta_has_column(conn: &Connection, name: &str) -> Result<bool> {
        let mut stmt = conn.prepare("PRAGMA table_info(repo_meta)")?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let column: String = row.get(1)?;
            if column == name {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn branches_has_column(conn: &Connection, name: &str) -> Result<bool> {
        let mut stmt = conn.prepare("PRAGMA table_info(branches)")?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let column: String = row.get(1)?;
            if column == name {
                return Ok(true);
            }
        }
        Ok(false)
    }

    #[cfg(test)]
    pub fn set_base_branch_if_missing(
        &self,
        base_branch: &str,
        source: BaseBranchSource,
    ) -> Result<()> {
        self.set_base_branch_if_missing_with_remote(base_branch, source, None)
    }

    pub fn set_base_branch_if_missing_with_remote(
        &self,
        base_branch: &str,
        source: BaseBranchSource,
        base_remote: Option<&str>,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO repo_meta(id, base_branch, base_branch_source, base_remote, schema_version)
             VALUES (1, ?1, ?2, ?3, ?4)
             ON CONFLICT(id) DO NOTHING",
            params![base_branch, source.as_str(), base_remote, SCHEMA_VERSION],
        )?;
        Ok(())
    }

    #[cfg(test)]
    pub fn reconcile_base_branch(
        &self,
        observed: &RepoMeta,
        candidate: &str,
        candidate_source: BaseBranchSource,
        observed_branch_exists: bool,
    ) -> Result<()> {
        self.reconcile_base_branch_with_remote(
            observed,
            candidate,
            candidate_source,
            None,
            observed_branch_exists,
        )
    }

    pub fn reconcile_base_branch_with_remote(
        &self,
        observed: &RepoMeta,
        candidate: &str,
        candidate_source: BaseBranchSource,
        candidate_remote: Option<&str>,
        observed_branch_exists: bool,
    ) -> Result<()> {
        let should_replace = !observed_branch_exists
            || (observed.base_branch_source.can_yield_to_remote_head()
                && candidate_source == BaseBranchSource::RemoteHead);
        if should_replace {
            let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
            let updated = tx.execute(
                "UPDATE repo_meta
                 SET base_branch = ?1, base_branch_source = ?2, base_remote = ?3
                 WHERE id = 1 AND base_branch = ?4 AND base_branch_source = ?5
                   AND base_remote IS ?6",
                params![
                    candidate,
                    candidate_source.as_str(),
                    candidate_remote,
                    observed.base_branch,
                    observed.base_branch_source.as_str(),
                    observed.base_remote.as_deref()
                ],
            )?;
            if updated == 0 {
                tx.commit()?;
                return Ok(());
            }

            let candidate_id = upsert_branch_on(&tx, candidate)?;
            tx.execute(
                "UPDATE branches
                 SET parent_branch_id = NULL, updated_at = CURRENT_TIMESTAMP
                 WHERE id = ?1",
                params![candidate_id],
            )?;
            if observed.base_branch != candidate
                && let Some(old_base_id) = tx
                    .query_row(
                        "SELECT id FROM branches WHERE name = ?1",
                        params![observed.base_branch],
                        |row| row.get::<_, i64>(0),
                    )
                    .optional()?
            {
                tx.execute(
                    "UPDATE branches
                     SET parent_branch_id = ?1, updated_at = CURRENT_TIMESTAMP
                     WHERE parent_branch_id = ?2 AND id != ?1",
                    params![candidate_id, old_base_id],
                )?;
                tx.execute("DELETE FROM branches WHERE id = ?1", params![old_base_id])?;
            }
            tx.commit()?;
        }
        Ok(())
    }

    pub fn repo_meta(&self) -> Result<RepoMeta> {
        self.repo_meta_optional()?
            .ok_or_else(|| anyhow!("repo metadata missing"))
    }

    pub fn repo_meta_optional(&self) -> Result<Option<RepoMeta>> {
        let raw = self
            .conn
            .query_row(
                "SELECT base_branch, base_branch_source, base_remote, push_target, canonical_repo,
                        fork_repo, push_permission, permission_checked_at
                 FROM repo_meta WHERE id = 1",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                    ))
                },
            )
            .optional()?;
        let Some(raw) = raw else {
            return Ok(None);
        };
        let source = BaseBranchSource::parse(&raw.1)
            .ok_or_else(|| anyhow!("invalid cached base branch source '{}'", raw.1))?;
        Ok(Some(RepoMeta {
            base_branch: raw.0,
            base_branch_source: source,
            base_remote: raw.2,
            push_target: raw.3,
            canonical_repo: raw.4,
            fork_repo: raw.5,
            push_permission: raw.6,
            permission_checked_at: raw.7,
        }))
    }

    pub fn set_push_target(&self, target: &str) -> Result<()> {
        if !matches!(target, "auto" | "upstream" | "fork") {
            return Err(anyhow!("invalid push target '{target}'"));
        }
        if target == "auto" {
            self.conn.execute(
                "UPDATE repo_meta
                 SET push_target = ?1, push_permission = NULL, permission_checked_at = NULL
                 WHERE id = 1",
                params![target],
            )?;
        } else {
            self.conn.execute(
                "UPDATE repo_meta SET push_target = ?1 WHERE id = 1",
                params![target],
            )?;
        }
        Ok(())
    }

    pub fn set_placement_cache(
        &self,
        canonical_repo: &str,
        fork_repo: Option<&str>,
        push_permission: Option<&str>,
        checked_at: i64,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE repo_meta
             SET canonical_repo = ?1, fork_repo = ?2, push_permission = ?3,
                 permission_checked_at = ?4
             WHERE id = 1",
            params![canonical_repo, fork_repo, push_permission, checked_at],
        )?;
        Ok(())
    }

    pub fn clear_placement_cache(&self) -> Result<()> {
        self.conn.execute(
            "UPDATE repo_meta
             SET push_permission = NULL, permission_checked_at = NULL
             WHERE id = 1",
            [],
        )?;
        Ok(())
    }

    pub fn branch_by_name(&self, name: &str) -> Result<Option<BranchRecord>> {
        self.conn
            .query_row(
                "SELECT id, name, parent_branch_id, last_synced_head_sha, cached_pr_number, cached_pr_state, cached_pr_head_oid
                 FROM branches WHERE name = ?1",
                params![name],
                |row| {
                    Ok(BranchRecord {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        parent_branch_id: row.get(2)?,
                        last_synced_head_sha: row.get(3)?,
                        cached_pr_number: row.get(4)?,
                        cached_pr_state: row.get(5)?,
                        cached_pr_head_oid: row.get(6)?,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn list_branches(&self) -> Result<Vec<BranchRecord>> {
        list_branches_on(&self.conn)
    }

    pub fn set_parent(&self, child_name: &str, parent_name: Option<&str>) -> Result<()> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let child_id = upsert_branch_on(&tx, child_name)?;
        let parent_id = if let Some(p) = parent_name {
            Some(upsert_branch_on(&tx, p)?)
        } else {
            None
        };
        if let Some(pid) = parent_id {
            ensure_no_cycle_on(&tx, child_id, pid)?;
        }
        tx.execute(
            "UPDATE branches SET parent_branch_id = ?1, updated_at = CURRENT_TIMESTAMP WHERE id = ?2",
            params![parent_id, child_id],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn apply_move_topology(
        &self,
        target_name: &str,
        parent_name: &str,
    ) -> Result<MoveTopologyChange> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let previous_target: Option<(i64, Option<i64>)> = tx
            .query_row(
                "SELECT id, parent_branch_id FROM branches WHERE name = ?1",
                params![target_name],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let previous_parent: Option<i64> = tx
            .query_row(
                "SELECT id FROM branches WHERE name = ?1",
                params![parent_name],
                |row| row.get(0),
            )
            .optional()?;

        let target_was_inserted = previous_target.is_none();
        let parent_was_inserted = previous_parent.is_none();
        let target_id = match previous_target {
            Some((id, _)) => id,
            None => upsert_branch_on(&tx, target_name)?,
        };
        let parent_id = match previous_parent {
            Some(id) => id,
            None => upsert_branch_on(&tx, parent_name)?,
        };
        ensure_no_cycle_on(&tx, target_id, parent_id)?;

        let previous_parent_id = previous_target.and_then(|(_, parent_id)| parent_id);
        let previous_parent_name = previous_parent_id
            .map(|id| {
                tx.query_row(
                    "SELECT name FROM branches WHERE id = ?1",
                    params![id],
                    |row| row.get(0),
                )
                .optional()?
                .ok_or_else(|| anyhow!("previous move parent record is missing"))
            })
            .transpose()?;

        tx.execute(
            "UPDATE branches
             SET parent_branch_id = ?1, updated_at = CURRENT_TIMESTAMP
             WHERE id = ?2",
            params![parent_id, target_id],
        )?;
        let target_topology_revision = branch_topology_revision_on(&tx, target_id)?;
        let parent_topology_revision = branch_topology_revision_on(&tx, parent_id)?;
        let previous_parent_topology_revision = previous_parent_id
            .map(|id| branch_topology_revision_on(&tx, id))
            .transpose()?;
        tx.commit()?;

        Ok(MoveTopologyChange {
            target_id,
            target_name: target_name.to_string(),
            previous_parent_id,
            previous_parent_name,
            target_was_inserted,
            parent_id,
            parent_name: parent_name.to_string(),
            parent_was_inserted,
            target_topology_revision,
            parent_topology_revision,
            previous_parent_topology_revision,
        })
    }

    pub fn rollback_move_topology(&self, change: &MoveTopologyChange) -> Result<()> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let Some(current_target) = move_rollback_branch_state_on(&tx, change.target_id)? else {
            return Err(anyhow!(
                "move topology changed before rollback: target '{}' no longer has attempted parent '{}'",
                change.target_name,
                change.parent_name
            ));
        };
        if current_target.name != change.target_name
            || current_target.parent_branch_id != Some(change.parent_id)
        {
            return Err(anyhow!(
                "move topology changed before rollback: target '{}' no longer has attempted parent '{}'",
                change.target_name,
                change.parent_name
            ));
        }
        if current_target.topology_revision != change.target_topology_revision {
            return Err(anyhow!(
                "move topology changed before rollback: target '{}' revision advanced from {} to {}",
                change.target_name,
                change.target_topology_revision,
                current_target.topology_revision
            ));
        }
        if change.target_was_inserted && current_target.has_sync_or_pr_metadata() {
            return Err(anyhow!(
                "move-inserted target '{}' gained sync or PR metadata before rollback",
                change.target_name
            ));
        }

        let Some(current_parent) = move_rollback_branch_state_on(&tx, change.parent_id)? else {
            return Err(anyhow!(
                "move topology changed before rollback: attempted parent '{}' is missing",
                change.parent_name
            ));
        };
        if current_parent.name != change.parent_name
            || (change.parent_was_inserted && current_parent.parent_branch_id.is_some())
        {
            return Err(anyhow!(
                "move topology changed before rollback: attempted parent '{}' changed",
                change.parent_name
            ));
        }
        if current_parent.topology_revision != change.parent_topology_revision {
            return Err(anyhow!(
                "move topology changed before rollback: attempted parent '{}' revision advanced from {} to {}",
                change.parent_name,
                change.parent_topology_revision,
                current_parent.topology_revision
            ));
        }
        if change.parent_was_inserted && current_parent.has_sync_or_pr_metadata() {
            return Err(anyhow!(
                "move-inserted parent '{}' gained sync or PR metadata before rollback",
                change.parent_name
            ));
        }

        if let (Some(previous_parent_id), Some(previous_parent_name)) = (
            change.previous_parent_id,
            change.previous_parent_name.as_deref(),
        ) {
            let previous_parent = move_rollback_branch_state_on(&tx, previous_parent_id)?;
            let expected_revision = change
                .previous_parent_topology_revision
                .ok_or_else(|| anyhow!("previous move parent revision is missing"))?;
            if previous_parent.as_ref().is_none_or(|parent| {
                parent.name != previous_parent_name || parent.topology_revision != expected_revision
            }) {
                return Err(anyhow!(
                    "move topology changed before rollback: previous parent '{}' changed",
                    previous_parent_name
                ));
            }
            ensure_no_cycle_on(&tx, change.target_id, previous_parent_id)?;
        }

        if change.target_was_inserted {
            let child_count: i64 = tx.query_row(
                "SELECT COUNT(*) FROM branches WHERE parent_branch_id = ?1",
                params![change.target_id],
                |row| row.get(0),
            )?;
            if child_count != 0 {
                return Err(anyhow!(
                    "move-inserted target '{}' gained children before rollback",
                    change.target_name
                ));
            }
        }
        if change.parent_was_inserted {
            let other_child_count: i64 = tx.query_row(
                "SELECT COUNT(*) FROM branches
                 WHERE parent_branch_id = ?1 AND id != ?2",
                params![change.parent_id, change.target_id],
                |row| row.get(0),
            )?;
            if other_child_count != 0 {
                return Err(anyhow!(
                    "move-inserted parent '{}' gained children before rollback",
                    change.parent_name
                ));
            }
        }

        if change.target_was_inserted {
            let deleted = tx.execute(
                "DELETE FROM branches
                 WHERE id = ?1 AND name = ?2 AND parent_branch_id = ?3",
                params![change.target_id, change.target_name, change.parent_id],
            )?;
            if deleted != 1 {
                return Err(anyhow!(
                    "move topology changed before rollback: target '{}' changed",
                    change.target_name
                ));
            }
        } else {
            let restored = tx.execute(
                "UPDATE branches
                 SET parent_branch_id = ?1, updated_at = CURRENT_TIMESTAMP
                 WHERE id = ?2 AND name = ?3 AND parent_branch_id = ?4",
                params![
                    change.previous_parent_id,
                    change.target_id,
                    change.target_name,
                    change.parent_id
                ],
            )?;
            if restored != 1 {
                return Err(anyhow!(
                    "move topology changed before rollback: target '{}' changed",
                    change.target_name
                ));
            }
        }

        if change.parent_was_inserted {
            let deleted = tx.execute(
                "DELETE FROM branches
                 WHERE id = ?1 AND name = ?2 AND parent_branch_id IS NULL",
                params![change.parent_id, change.parent_name],
            )?;
            if deleted != 1 {
                return Err(anyhow!(
                    "move topology changed before rollback: inserted parent '{}' changed",
                    change.parent_name
                ));
            }
        }

        tx.commit()?;
        Ok(())
    }

    pub fn set_parents_batch(&self, updates: &[ParentUpdate]) -> Result<()> {
        if updates.is_empty() {
            return Ok(());
        }

        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let existing = list_branches_on(&tx)?;
        let mut id_by_name: std::collections::HashMap<String, i64> =
            existing.iter().map(|b| (b.name.clone(), b.id)).collect();
        let mut parent_by_id: std::collections::HashMap<i64, Option<i64>> = existing
            .iter()
            .map(|b| (b.id, b.parent_branch_id))
            .collect();
        let mut next_id = existing.iter().map(|b| b.id).max().unwrap_or(0) + 1;

        for update in updates {
            let child_id = ensure_temp_id(
                &mut id_by_name,
                &mut parent_by_id,
                &mut next_id,
                &update.child_name,
            );
            let parent_id = update
                .parent_name
                .as_deref()
                .map(|name| ensure_temp_id(&mut id_by_name, &mut parent_by_id, &mut next_id, name));
            parent_by_id.insert(child_id, parent_id);
        }

        for id in parent_by_id.keys().copied() {
            let mut seen = std::collections::HashSet::new();
            let mut cursor = Some(id);
            while let Some(current) = cursor {
                if !seen.insert(current) {
                    return Err(anyhow!("link would create a cycle"));
                }
                cursor = parent_by_id.get(&current).copied().flatten();
            }
        }

        for update in updates {
            tx.execute(
                "INSERT INTO branches(name) VALUES (?1)
                 ON CONFLICT(name) DO UPDATE SET updated_at = CURRENT_TIMESTAMP",
                params![update.child_name],
            )?;
            if let Some(parent) = &update.parent_name {
                tx.execute(
                    "INSERT INTO branches(name) VALUES (?1)
                     ON CONFLICT(name) DO UPDATE SET updated_at = CURRENT_TIMESTAMP",
                    params![parent],
                )?;
            }
        }

        for update in updates {
            if let Some(parent) = &update.parent_name {
                tx.execute(
                    "UPDATE branches
                     SET parent_branch_id = (SELECT id FROM branches WHERE name = ?1),
                         updated_at = CURRENT_TIMESTAMP
                     WHERE name = ?2",
                    params![parent, update.child_name],
                )?;
            } else {
                tx.execute(
                    "UPDATE branches SET parent_branch_id = NULL, updated_at = CURRENT_TIMESTAMP WHERE name = ?1",
                    params![update.child_name],
                )?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn set_sync_sha(&self, branch_name: &str, sha: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE branches SET last_synced_head_sha = ?1, updated_at = CURRENT_TIMESTAMP WHERE name = ?2",
            params![sha, branch_name],
        )?;
        Ok(())
    }

    pub fn set_sync_shas(&self, updates: &[(String, String)]) -> Result<()> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        for (branch_name, sha) in updates {
            let updated = tx.execute(
                "UPDATE branches
                 SET last_synced_head_sha = ?1, updated_at = CURRENT_TIMESTAMP
                 WHERE name = ?2",
                params![sha, branch_name],
            )?;
            if updated != 1 {
                return Err(anyhow!(
                    "cannot persist sync SHA: tracked branch '{branch_name}' is missing"
                ));
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn set_pr_cache(
        &self,
        branch_name: &str,
        number: Option<i64>,
        state: Option<&str>,
        head_oid: Option<&str>,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE branches
             SET cached_pr_number = ?1, cached_pr_state = ?2, cached_pr_head_oid = ?3,
                 updated_at = CURRENT_TIMESTAMP
             WHERE name = ?4",
            params![number, state, head_oid, branch_name],
        )?;
        Ok(())
    }

    pub fn clear_parent(&self, branch_name: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE branches SET parent_branch_id = NULL, updated_at = CURRENT_TIMESTAMP WHERE name = ?1",
            params![branch_name],
        )?;
        Ok(())
    }

    pub fn rename_branch(&self, old_name: &str, new_name: &str) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        let old_id: Option<i64> = tx
            .query_row(
                "SELECT id FROM branches WHERE name = ?1",
                params![old_name],
                |row| row.get(0),
            )
            .optional()?;
        if old_id.is_none() {
            return Err(anyhow!("branch '{old_name}' is not tracked"));
        }

        let new_exists: Option<i64> = tx
            .query_row(
                "SELECT id FROM branches WHERE name = ?1",
                params![new_name],
                |row| row.get(0),
            )
            .optional()?;
        if new_exists.is_some() {
            return Err(anyhow!("branch '{new_name}' is already tracked"));
        }

        tx.execute(
            "UPDATE branches SET name = ?1, updated_at = CURRENT_TIMESTAMP WHERE name = ?2",
            params![new_name, old_name],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn delete_branch(&self, branch_name: &str) -> Result<()> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        tx.execute(
            "UPDATE branches SET parent_branch_id = NULL
             WHERE parent_branch_id = (SELECT id FROM branches WHERE name = ?1)",
            params![branch_name],
        )?;
        tx.execute("DELETE FROM branches WHERE name = ?1", params![branch_name])?;
        tx.commit()?;
        Ok(())
    }

    pub fn splice_out_branch(&self, branch_name: &str) -> Result<()> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let branch: Option<(i64, Option<i64>)> = tx
            .query_row(
                "SELECT id, parent_branch_id FROM branches WHERE name = ?1",
                params![branch_name],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let (branch_id, parent_branch_id) =
            branch.ok_or_else(|| anyhow!("branch '{branch_name}' is not tracked"))?;
        tx.execute(
            "UPDATE branches SET parent_branch_id = ?1 WHERE parent_branch_id = ?2",
            params![parent_branch_id, branch_id],
        )?;
        tx.execute("DELETE FROM branches WHERE id = ?1", params![branch_id])?;
        tx.commit()?;
        Ok(())
    }

    pub fn splice_out_branch_if_unchanged(&self, expected: &BranchRecord) -> Result<()> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let current: Option<(i64, Option<i64>)> = tx
            .query_row(
                "SELECT id, parent_branch_id FROM branches WHERE name = ?1",
                params![expected.name],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let Some((branch_id, parent_branch_id)) = current else {
            return Err(anyhow!("branch '{}' is no longer tracked", expected.name));
        };
        if branch_id != expected.id || parent_branch_id != expected.parent_branch_id {
            return Err(anyhow!(
                "branch '{}' topology changed before deletion",
                expected.name
            ));
        }
        tx.execute(
            "UPDATE branches SET parent_branch_id = ?1 WHERE parent_branch_id = ?2",
            params![parent_branch_id, branch_id],
        )?;
        let deleted = tx.execute(
            "DELETE FROM branches
             WHERE id = ?1 AND name = ?2 AND parent_branch_id IS ?3",
            params![branch_id, expected.name, parent_branch_id],
        )?;
        if deleted != 1 {
            return Err(anyhow!(
                "branch '{}' topology changed before deletion",
                expected.name
            ));
        }
        tx.commit()?;
        Ok(())
    }

    pub fn record_sync_start(&self) -> Result<i64> {
        self.conn
            .execute("INSERT INTO sync_runs(status) VALUES ('running')", [])?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn record_sync_finish(
        &self,
        id: i64,
        status: &str,
        summary_json: Option<&str>,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE sync_runs SET finished_at = CURRENT_TIMESTAMP, status = ?1, summary_json = ?2 WHERE id = ?3",
            params![status, summary_json, id],
        )?;
        Ok(())
    }
}

fn upsert_branch_on(conn: &Connection, name: &str) -> Result<i64> {
    conn.execute(
        "INSERT INTO branches(name) VALUES (?1)
         ON CONFLICT(name) DO UPDATE SET updated_at = CURRENT_TIMESTAMP",
        params![name],
    )?;
    conn.query_row(
        "SELECT id FROM branches WHERE name = ?1",
        params![name],
        |row| row.get(0),
    )
    .optional()?
    .ok_or_else(|| anyhow!("failed to upsert branch {name}"))
}

#[cfg(test)]
fn topology_revision_on(conn: &Connection) -> Result<i64> {
    conn.query_row(
        "SELECT revision FROM topology_state WHERE id = 1",
        [],
        |row| row.get(0),
    )
    .context("topology revision is missing")
}

fn branch_topology_revision_on(conn: &Connection, branch_id: i64) -> Result<i64> {
    conn.query_row(
        "SELECT topology_revision FROM branches WHERE id = ?1",
        params![branch_id],
        |row| row.get(0),
    )
    .with_context(|| format!("topology revision is missing for branch id {branch_id}"))
}

fn list_branches_on(conn: &Connection) -> Result<Vec<BranchRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, parent_branch_id, last_synced_head_sha, cached_pr_number, cached_pr_state, cached_pr_head_oid
         FROM branches ORDER BY name",
    )?;
    let mut rows = stmt.query([])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(BranchRecord {
            id: row.get(0)?,
            name: row.get(1)?,
            parent_branch_id: row.get(2)?,
            last_synced_head_sha: row.get(3)?,
            cached_pr_number: row.get(4)?,
            cached_pr_state: row.get(5)?,
            cached_pr_head_oid: row.get(6)?,
        });
    }
    Ok(out)
}

fn move_rollback_branch_state_on(
    conn: &Connection,
    id: i64,
) -> Result<Option<MoveRollbackBranchState>> {
    conn.query_row(
        "SELECT name, parent_branch_id, last_synced_head_sha, cached_pr_number,
                cached_pr_state, cached_pr_head_oid, topology_revision
         FROM branches
         WHERE id = ?1",
        params![id],
        |row| {
            Ok(MoveRollbackBranchState {
                name: row.get(0)?,
                parent_branch_id: row.get(1)?,
                last_synced_head_sha: row.get(2)?,
                cached_pr_number: row.get(3)?,
                cached_pr_state: row.get(4)?,
                cached_pr_head_oid: row.get(5)?,
                topology_revision: row.get(6)?,
            })
        },
    )
    .optional()
    .map_err(Into::into)
}

fn ensure_no_cycle_on(conn: &Connection, child_id: i64, mut parent_id: i64) -> Result<()> {
    let mut seen = std::collections::HashSet::from([child_id]);
    loop {
        if !seen.insert(parent_id) {
            return Err(anyhow!("link would create a cycle"));
        }
        let next: Option<i64> = conn
            .query_row(
                "SELECT parent_branch_id FROM branches WHERE id = ?1",
                params![parent_id],
                |row| row.get(0),
            )
            .optional()?
            .flatten();
        if let Some(next_parent) = next {
            parent_id = next_parent;
        } else {
            return Ok(());
        }
    }
}

fn ensure_temp_id(
    id_by_name: &mut std::collections::HashMap<String, i64>,
    parent_by_id: &mut std::collections::HashMap<i64, Option<i64>>,
    next_id: &mut i64,
    name: &str,
) -> i64 {
    if let Some(id) = id_by_name.get(name) {
        *id
    } else {
        let id = *next_id;
        *next_id += 1;
        id_by_name.insert(name.to_string(), id);
        parent_by_id.insert(id, None);
        id
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc, Barrier,
        atomic::{AtomicBool, Ordering},
    };
    use std::time::{Duration, Instant};

    use super::*;

    static SPLICE_WRITER_BLOCKED: AtomicBool = AtomicBool::new(false);

    fn record_splice_writer_block(_: i32) -> bool {
        SPLICE_WRITER_BLOCKED.store(true, Ordering::Release);
        true
    }

    fn database_with_failing_victim_delete() -> (tempfile::TempDir, Database) {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("stack.db")).unwrap();
        db.set_parent("victim", Some("main")).unwrap();
        db.set_parent("child", Some("victim")).unwrap();
        db.conn
            .execute_batch(
                "CREATE TEMP TRIGGER fail_victim_delete
                 BEFORE DELETE ON branches
                 WHEN OLD.name = 'victim'
                 BEGIN
                     SELECT RAISE(ABORT, 'forced victim delete failure');
                 END;",
            )
            .unwrap();
        (dir, db)
    }

    #[test]
    fn topology_revisions_track_branch_graph_changes_only() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("stack.db")).unwrap();
        let initial = topology_revision_on(&db.conn).unwrap();

        db.conn
            .execute("INSERT INTO branches(name) VALUES ('parent')", [])
            .unwrap();
        db.conn
            .execute("INSERT INTO branches(name) VALUES ('child')", [])
            .unwrap();
        let after_inserts = topology_revision_on(&db.conn).unwrap();
        assert!(after_inserts > initial);
        let parent = db.branch_by_name("parent").unwrap().unwrap();
        let child = db.branch_by_name("child").unwrap().unwrap();
        let parent_revision = branch_topology_revision_on(&db.conn, parent.id).unwrap();
        let inserted_child_revision = branch_topology_revision_on(&db.conn, child.id).unwrap();

        db.conn
            .execute(
                "UPDATE branches
                 SET parent_branch_id = (SELECT id FROM branches WHERE name = 'parent')
                 WHERE name = 'child'",
                [],
            )
            .unwrap();
        let after_parent_change = topology_revision_on(&db.conn).unwrap();
        assert!(after_parent_change > after_inserts);
        let reparented_child_revision = branch_topology_revision_on(&db.conn, child.id).unwrap();
        assert!(reparented_child_revision > inserted_child_revision);
        assert_eq!(
            branch_topology_revision_on(&db.conn, parent.id).unwrap(),
            parent_revision,
            "changing one branch must not advance an unrelated branch stamp"
        );

        db.conn
            .execute(
                "UPDATE branches
                 SET cached_pr_number = 42, cached_pr_state = 'open',
                     last_synced_head_sha = 'abc', updated_at = CURRENT_TIMESTAMP
                 WHERE name = 'child'",
                [],
            )
            .unwrap();
        assert_eq!(topology_revision_on(&db.conn).unwrap(), after_parent_change);
        assert_eq!(
            branch_topology_revision_on(&db.conn, child.id).unwrap(),
            reparented_child_revision
        );

        db.conn
            .execute(
                "UPDATE branches SET name = 'renamed-child' WHERE name = 'child'",
                [],
            )
            .unwrap();
        let after_rename = topology_revision_on(&db.conn).unwrap();
        assert!(after_rename > after_parent_change);
        assert!(
            branch_topology_revision_on(&db.conn, child.id).unwrap() > reparented_child_revision
        );

        db.conn
            .execute("DELETE FROM branches WHERE name = 'renamed-child'", [])
            .unwrap();
        assert!(topology_revision_on(&db.conn).unwrap() > after_rename);
    }

    #[test]
    fn prevents_cycles() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("stack.db")).unwrap();
        db.set_parent("b", Some("a")).unwrap();
        let err = db.set_parent("a", Some("b")).unwrap_err();
        assert!(err.to_string().contains("cycle"));
    }

    #[test]
    fn rejected_parent_update_rolls_back_new_branch_records() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("stack.db")).unwrap();

        let err = db.set_parent("self-cycle", Some("self-cycle")).unwrap_err();

        assert!(err.to_string().contains("cycle"));
        assert!(db.branch_by_name("self-cycle").unwrap().is_none());
    }

    #[test]
    fn concurrent_opposite_parent_updates_cannot_commit_a_cycle() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("stack.db");
        let db = Database::open(&db_path).unwrap();
        db.set_parent("a", None).unwrap();
        db.set_parent("b", None).unwrap();
        drop(db);

        let barrier = Arc::new(Barrier::new(3));
        let spawn_update = |child: &'static str, parent: &'static str| {
            let path = db_path.clone();
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                let db = Database::open(&path).unwrap();
                barrier.wait();
                db.set_parent(child, Some(parent))
            })
        };
        let a_to_b = spawn_update("a", "b");
        let b_to_a = spawn_update("b", "a");
        barrier.wait();

        let results = [a_to_b.join().unwrap(), b_to_a.join().unwrap()];
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert!(
            results
                .iter()
                .filter_map(|result| result.as_ref().err())
                .any(|err| err.to_string().contains("cycle"))
        );

        let db = Database::open(&db_path).unwrap();
        let a = db.branch_by_name("a").unwrap().unwrap();
        let b = db.branch_by_name("b").unwrap().unwrap();
        assert!(!(a.parent_branch_id == Some(b.id) && b.parent_branch_id == Some(a.id)));
    }

    #[test]
    fn concurrent_batch_parent_updates_cannot_commit_a_cycle() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("stack.db");
        let db = Database::open(&db_path).unwrap();
        db.set_parent("a", None).unwrap();
        db.set_parent("b", None).unwrap();
        drop(db);

        let barrier = Arc::new(Barrier::new(3));
        let spawn_update = |child: &'static str, parent: &'static str| {
            let path = db_path.clone();
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                let db = Database::open(&path).unwrap();
                barrier.wait();
                db.set_parents_batch(&[ParentUpdate {
                    child_name: child.to_string(),
                    parent_name: Some(parent.to_string()),
                }])
            })
        };
        let a_to_b = spawn_update("a", "b");
        let b_to_a = spawn_update("b", "a");
        barrier.wait();

        let results = [a_to_b.join().unwrap(), b_to_a.join().unwrap()];
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert!(
            results
                .iter()
                .filter_map(|result| result.as_ref().err())
                .any(|err| err.to_string().contains("cycle"))
        );
    }

    #[test]
    fn parent_validation_rejects_a_preexisting_cycle_without_looping() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("stack.db")).unwrap();
        db.set_parent("a", None).unwrap();
        db.set_parent("b", None).unwrap();
        db.set_parent("c", None).unwrap();
        let a = db.branch_by_name("a").unwrap().unwrap();
        let b = db.branch_by_name("b").unwrap().unwrap();
        db.conn
            .execute(
                "UPDATE branches SET parent_branch_id = ?1 WHERE id = ?2",
                params![b.id, a.id],
            )
            .unwrap();
        db.conn
            .execute(
                "UPDATE branches SET parent_branch_id = ?1 WHERE id = ?2",
                params![a.id, b.id],
            )
            .unwrap();

        let err = db.set_parent("c", Some("a")).unwrap_err();
        assert!(err.to_string().contains("cycle"));
    }

    #[test]
    fn failed_move_topology_rollback_restores_target_and_removes_inserted_parent() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("stack.db")).unwrap();
        db.set_parent("target", Some("main")).unwrap();

        let change = db.apply_move_topology("target", "new-parent").unwrap();
        assert_eq!(change.previous_parent_name(), Some("main"));
        let applied_revision = topology_revision_on(&db.conn).unwrap();

        db.rollback_move_topology(&change).unwrap();

        assert!(topology_revision_on(&db.conn).unwrap() > applied_revision);
        let target = db.branch_by_name("target").unwrap().unwrap();
        let main = db.branch_by_name("main").unwrap().unwrap();
        assert_eq!(target.parent_branch_id, Some(main.id));
        assert!(db.branch_by_name("new-parent").unwrap().is_none());
    }

    #[test]
    fn failed_move_topology_rollback_removes_inserted_target_and_parent() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("stack.db")).unwrap();
        let change = db
            .apply_move_topology("inserted-target", "inserted-parent")
            .unwrap();

        db.rollback_move_topology(&change).unwrap();

        assert!(db.branch_by_name("inserted-target").unwrap().is_none());
        assert!(db.branch_by_name("inserted-parent").unwrap().is_none());
    }

    #[test]
    fn failed_move_topology_rollback_rejects_a_concurrent_target_reparent() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("stack.db")).unwrap();
        db.set_parent("target", Some("main")).unwrap();
        db.set_parent("replacement", Some("main")).unwrap();
        let change = db.apply_move_topology("target", "new-parent").unwrap();

        db.set_parent("target", Some("replacement")).unwrap();
        let err = db.rollback_move_topology(&change).unwrap_err();

        assert!(err.to_string().contains("topology changed"));
        let target = db.branch_by_name("target").unwrap().unwrap();
        let replacement = db.branch_by_name("replacement").unwrap().unwrap();
        assert_eq!(target.parent_branch_id, Some(replacement.id));
        assert!(
            db.branch_by_name("new-parent").unwrap().is_some(),
            "a rejected rollback must not partially remove the inserted parent"
        );
    }

    #[test]
    fn failed_move_topology_rollback_rejects_a_concurrent_target_parent_aba() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stack.db");
        let failed_move_db = Database::open(&path).unwrap();
        failed_move_db.set_parent("target", Some("main")).unwrap();
        failed_move_db
            .set_parent("replacement", Some("main"))
            .unwrap();
        let change = failed_move_db
            .apply_move_topology("target", "new-parent")
            .unwrap();

        let concurrent_db = Database::open(&path).unwrap();
        concurrent_db
            .set_parent("target", Some("replacement"))
            .unwrap();
        concurrent_db
            .set_parent("target", Some("new-parent"))
            .unwrap();
        let err = failed_move_db.rollback_move_topology(&change).unwrap_err();

        assert!(err.to_string().contains("topology changed"));
        let target = failed_move_db.branch_by_name("target").unwrap().unwrap();
        let parent = failed_move_db
            .branch_by_name("new-parent")
            .unwrap()
            .unwrap();
        assert_eq!(target.parent_branch_id, Some(parent.id));
    }

    #[test]
    fn failed_move_topology_rollback_allows_an_unrelated_concurrent_reparent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stack.db");
        let failed_move_db = Database::open(&path).unwrap();
        failed_move_db.set_parent("target", Some("main")).unwrap();
        failed_move_db
            .set_parent("unrelated", Some("main"))
            .unwrap();
        failed_move_db
            .set_parent("replacement", Some("main"))
            .unwrap();
        let change = failed_move_db
            .apply_move_topology("target", "new-parent")
            .unwrap();

        let concurrent_db = Database::open(&path).unwrap();
        concurrent_db
            .set_parent("unrelated", Some("replacement"))
            .unwrap();
        failed_move_db.rollback_move_topology(&change).unwrap();

        let target = failed_move_db.branch_by_name("target").unwrap().unwrap();
        let main = failed_move_db.branch_by_name("main").unwrap().unwrap();
        assert_eq!(target.parent_branch_id, Some(main.id));
        assert!(
            failed_move_db
                .branch_by_name("new-parent")
                .unwrap()
                .is_none()
        );
        let unrelated = failed_move_db.branch_by_name("unrelated").unwrap().unwrap();
        let replacement = failed_move_db
            .branch_by_name("replacement")
            .unwrap()
            .unwrap();
        assert_eq!(unrelated.parent_branch_id, Some(replacement.id));
    }

    #[test]
    fn failed_move_topology_rollback_does_not_detach_a_child_of_an_inserted_target() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("stack.db")).unwrap();
        db.set_parent("parent", Some("main")).unwrap();
        let change = db.apply_move_topology("inserted-target", "parent").unwrap();

        db.set_parent("concurrent-child", Some("inserted-target"))
            .unwrap();
        let err = db.rollback_move_topology(&change).unwrap_err();

        assert!(err.to_string().contains("gained children"));
        let target = db.branch_by_name("inserted-target").unwrap().unwrap();
        let child = db.branch_by_name("concurrent-child").unwrap().unwrap();
        assert_eq!(child.parent_branch_id, Some(target.id));
    }

    #[test]
    fn failed_move_topology_rollback_does_not_remove_an_inserted_parent_with_a_new_child() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("stack.db")).unwrap();
        db.set_parent("target", Some("main")).unwrap();
        let change = db.apply_move_topology("target", "inserted-parent").unwrap();

        db.set_parent("concurrent-child", Some("inserted-parent"))
            .unwrap();
        let err = db.rollback_move_topology(&change).unwrap_err();

        assert!(err.to_string().contains("gained children"));
        let parent = db.branch_by_name("inserted-parent").unwrap().unwrap();
        let target = db.branch_by_name("target").unwrap().unwrap();
        let child = db.branch_by_name("concurrent-child").unwrap().unwrap();
        assert_eq!(target.parent_branch_id, Some(parent.id));
        assert_eq!(child.parent_branch_id, Some(parent.id));
    }

    #[test]
    fn failed_move_topology_rollback_preserves_metadata_added_to_inserted_rows() {
        for metadata_branch in ["inserted-target", "inserted-parent"] {
            let dir = tempfile::tempdir().unwrap();
            let db = Database::open(&dir.path().join("stack.db")).unwrap();
            let change = db
                .apply_move_topology("inserted-target", "inserted-parent")
                .unwrap();
            if metadata_branch == "inserted-target" {
                db.set_pr_cache(metadata_branch, Some(42), Some("open"), Some("abc"))
                    .unwrap();
            } else {
                db.set_sync_sha(metadata_branch, "concurrent-sha").unwrap();
            }

            let err = db.rollback_move_topology(&change).unwrap_err();

            assert!(err.to_string().contains("gained sync or PR metadata"));
            assert!(db.branch_by_name("inserted-target").unwrap().is_some());
            assert!(db.branch_by_name("inserted-parent").unwrap().is_some());
        }
    }

    #[test]
    fn failed_move_topology_rollback_is_atomic_when_inserted_parent_deletion_fails() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("stack.db")).unwrap();
        db.set_parent("target", Some("main")).unwrap();
        let change = db.apply_move_topology("target", "inserted-parent").unwrap();
        let applied_revision = topology_revision_on(&db.conn).unwrap();
        db.conn
            .execute_batch(
                "CREATE TEMP TRIGGER fail_inserted_parent_delete
                 BEFORE DELETE ON branches
                 WHEN OLD.name = 'inserted-parent'
                 BEGIN
                     SELECT RAISE(ABORT, 'forced inserted parent delete failure');
                 END;",
            )
            .unwrap();

        let err = db.rollback_move_topology(&change).unwrap_err();

        assert!(
            err.to_string()
                .contains("forced inserted parent delete failure")
        );
        assert_eq!(
            topology_revision_on(&db.conn).unwrap(),
            applied_revision,
            "a failed rollback must also roll back its topology revision"
        );
        let parent = db.branch_by_name("inserted-parent").unwrap().unwrap();
        let target = db.branch_by_name("target").unwrap().unwrap();
        assert_eq!(
            target.parent_branch_id,
            Some(parent.id),
            "the target restore must roll back with parent cleanup"
        );
    }

    #[test]
    fn splice_out_branch_relinks_children_to_parent() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("stack.db")).unwrap();
        db.set_parent("a", Some("main")).unwrap();
        db.set_parent("b", Some("a")).unwrap();
        db.splice_out_branch("a").unwrap();

        let b = db.branch_by_name("b").unwrap().unwrap();
        let main = db.branch_by_name("main").unwrap().unwrap();
        assert_eq!(b.parent_branch_id, Some(main.id));
        assert!(db.branch_by_name("a").unwrap().is_none());
    }

    #[test]
    fn delete_branch_rolls_back_child_detachment_when_delete_fails() {
        let (_dir, db) = database_with_failing_victim_delete();

        let err = db.delete_branch("victim").unwrap_err();

        assert!(err.to_string().contains("forced victim delete failure"));
        let victim = db.branch_by_name("victim").unwrap().unwrap();
        let child = db.branch_by_name("child").unwrap().unwrap();
        assert_eq!(child.parent_branch_id, Some(victim.id));
    }

    #[test]
    fn splice_out_branch_rolls_back_child_relink_when_delete_fails() {
        let (_dir, db) = database_with_failing_victim_delete();

        let err = db.splice_out_branch("victim").unwrap_err();

        assert!(err.to_string().contains("forced victim delete failure"));
        let victim = db.branch_by_name("victim").unwrap().unwrap();
        let child = db.branch_by_name("child").unwrap().unwrap();
        assert_eq!(child.parent_branch_id, Some(victim.id));
    }

    #[test]
    fn conditional_splice_rejects_stale_topology_without_mutation() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("stack.db")).unwrap();
        db.set_parent("victim", Some("main")).unwrap();
        db.set_parent("child", Some("victim")).unwrap();
        db.set_parent("replacement", Some("main")).unwrap();
        let expected = db.branch_by_name("victim").unwrap().unwrap();

        db.set_parent("victim", Some("replacement")).unwrap();
        let err = db.splice_out_branch_if_unchanged(&expected).unwrap_err();

        assert!(err.to_string().contains("topology changed"));
        let victim = db.branch_by_name("victim").unwrap().unwrap();
        let replacement = db.branch_by_name("replacement").unwrap().unwrap();
        let child = db.branch_by_name("child").unwrap().unwrap();
        assert_eq!(victim.parent_branch_id, Some(replacement.id));
        assert_eq!(child.parent_branch_id, Some(victim.id));
    }

    #[test]
    fn sync_sha_batch_rolls_back_every_update_when_one_branch_fails() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("stack.db")).unwrap();
        db.set_parent("a", Some("main")).unwrap();
        db.set_parent("b", Some("main")).unwrap();
        db.conn
            .execute_batch(
                "CREATE TEMP TRIGGER fail_b_sync_sha
                 BEFORE UPDATE OF last_synced_head_sha ON branches
                 WHEN OLD.name = 'b'
                 BEGIN
                     SELECT RAISE(ABORT, 'forced sync SHA failure');
                 END;",
            )
            .unwrap();

        let err = db
            .set_sync_shas(&[
                ("a".to_string(), "sha-a".to_string()),
                ("b".to_string(), "sha-b".to_string()),
            ])
            .unwrap_err();

        assert!(err.to_string().contains("forced sync SHA failure"));
        for branch in ["a", "b"] {
            assert!(
                db.branch_by_name(branch)
                    .unwrap()
                    .unwrap()
                    .last_synced_head_sha
                    .is_none()
            );
        }
    }

    #[test]
    fn sync_sha_batch_rejects_a_missing_branch_without_partial_updates() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("stack.db")).unwrap();
        db.set_parent("a", Some("main")).unwrap();

        let err = db
            .set_sync_shas(&[
                ("a".to_string(), "sha-a".to_string()),
                ("missing".to_string(), "sha-missing".to_string()),
            ])
            .unwrap_err();

        assert!(err.to_string().contains("missing"));
        assert!(
            db.branch_by_name("a")
                .unwrap()
                .unwrap()
                .last_synced_head_sha
                .is_none()
        );
    }

    #[test]
    fn splice_out_branch_reads_topology_after_acquiring_the_writer_lock() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stack.db");
        let writer = Database::open(&path).unwrap();
        writer.set_parent("victim", Some("main")).unwrap();
        writer.set_parent("child", Some("victim")).unwrap();
        writer.set_parent("replacement", Some("main")).unwrap();

        let splicer = Database::open(&path).unwrap();
        splicer
            .conn
            .busy_handler(Some(record_splice_writer_block))
            .unwrap();
        SPLICE_WRITER_BLOCKED.store(false, Ordering::Release);

        let replacement = writer.branch_by_name("replacement").unwrap().unwrap();
        let writer_tx =
            Transaction::new_unchecked(&writer.conn, TransactionBehavior::Immediate).unwrap();
        writer_tx
            .execute(
                "UPDATE branches SET parent_branch_id = ?1 WHERE name = 'victim'",
                params![replacement.id],
            )
            .unwrap();

        let splice_worker = std::thread::spawn(move || splicer.splice_out_branch("victim"));
        let wait_started = Instant::now();
        while !SPLICE_WRITER_BLOCKED.load(Ordering::Acquire) {
            assert!(
                wait_started.elapsed() < Duration::from_secs(5),
                "splice writer did not wait for the competing transaction"
            );
            std::thread::yield_now();
        }

        writer_tx.commit().unwrap();
        splice_worker.join().unwrap().unwrap();

        let child = writer.branch_by_name("child").unwrap().unwrap();
        assert_eq!(child.parent_branch_id, Some(replacement.id));
        assert!(writer.branch_by_name("victim").unwrap().is_none());
    }

    #[test]
    fn set_parents_batch_rejects_cycles() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("stack.db")).unwrap();
        db.set_parent("a", Some("main")).unwrap();
        db.set_parent("b", Some("a")).unwrap();

        let err = db
            .set_parents_batch(&[
                ParentUpdate {
                    child_name: "a".to_string(),
                    parent_name: Some("b".to_string()),
                },
                ParentUpdate {
                    child_name: "b".to_string(),
                    parent_name: Some("a".to_string()),
                },
            ])
            .unwrap_err();
        assert!(err.to_string().contains("cycle"));
    }

    #[test]
    fn rename_branch_updates_name_preserves_parent_links() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("stack.db")).unwrap();
        db.set_parent("parent", Some("main")).unwrap();
        db.set_parent("child", Some("parent")).unwrap();

        db.rename_branch("parent", "parent-renamed").unwrap();

        assert!(db.branch_by_name("parent").unwrap().is_none());
        let renamed = db.branch_by_name("parent-renamed").unwrap().unwrap();
        let child = db.branch_by_name("child").unwrap().unwrap();
        assert_eq!(child.parent_branch_id, Some(renamed.id));
    }

    #[test]
    fn rename_branch_fails_when_old_missing() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("stack.db")).unwrap();
        let err = db.rename_branch("missing", "new-name").unwrap_err();
        assert!(err.to_string().contains("not tracked"));
    }

    #[test]
    fn rename_branch_fails_when_new_exists() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("stack.db")).unwrap();
        db.set_parent("old", Some("main")).unwrap();
        db.set_parent("existing", Some("main")).unwrap();

        let err = db.rename_branch("old", "existing").unwrap_err();
        assert!(err.to_string().contains("already tracked"));
    }

    #[test]
    fn repository_push_configuration_round_trips_and_auto_clears_cache() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("stack.db")).unwrap();
        db.set_base_branch_if_missing("main", BaseBranchSource::LocalConvention)
            .unwrap();
        db.set_push_target("upstream").unwrap();
        db.set_placement_cache("acme/repo", Some("alice/repo"), Some("WRITE"), 123)
            .unwrap();

        let meta = db.repo_meta().unwrap();
        assert_eq!(meta.push_target.as_deref(), Some("upstream"));
        assert_eq!(meta.canonical_repo.as_deref(), Some("acme/repo"));
        assert_eq!(meta.fork_repo.as_deref(), Some("alice/repo"));
        assert_eq!(meta.push_permission.as_deref(), Some("WRITE"));
        assert_eq!(meta.permission_checked_at, Some(123));

        db.set_push_target("auto").unwrap();
        let meta = db.repo_meta().unwrap();
        assert_eq!(meta.push_target.as_deref(), Some("auto"));
        assert!(meta.push_permission.is_none());
        assert!(meta.permission_checked_at.is_none());
    }

    #[test]
    fn authoritative_remote_head_replaces_heuristic_base_discovery() {
        let dir = tempfile::tempdir().unwrap();
        let provisional_path = dir.path().join("provisional.db");
        let provisional = Database::open(&provisional_path).unwrap();
        provisional
            .set_base_branch_if_missing("feat/work", BaseBranchSource::CurrentBranch)
            .unwrap();
        let observed = provisional.repo_meta().unwrap();
        provisional
            .reconcile_base_branch(&observed, "production", BaseBranchSource::RemoteHead, true)
            .unwrap();
        let meta = provisional.repo_meta().unwrap();
        assert_eq!(meta.base_branch, "production");
        assert_eq!(meta.base_branch_source, BaseBranchSource::RemoteHead);

        let stable_path = dir.path().join("stable.db");
        let stable = Database::open(&stable_path).unwrap();
        stable
            .set_base_branch_if_missing("main", BaseBranchSource::LocalConvention)
            .unwrap();
        let observed = stable.repo_meta().unwrap();
        stable
            .reconcile_base_branch(&observed, "trunk", BaseBranchSource::RemoteHead, true)
            .unwrap();
        let meta = stable.repo_meta().unwrap();
        assert_eq!(meta.base_branch, "trunk");
        assert_eq!(meta.base_branch_source, BaseBranchSource::RemoteHead);

        let legacy_path = dir.path().join("legacy.db");
        let legacy = Database::open(&legacy_path).unwrap();
        legacy
            .set_base_branch_if_missing("master", BaseBranchSource::Legacy)
            .unwrap();
        let observed = legacy.repo_meta().unwrap();
        legacy
            .reconcile_base_branch(&observed, "production", BaseBranchSource::RemoteHead, true)
            .unwrap();
        let meta = legacy.repo_meta().unwrap();
        assert_eq!(meta.base_branch, "production");
        assert_eq!(meta.base_branch_source, BaseBranchSource::RemoteHead);

        let same_name_path = dir.path().join("same-name.db");
        let same_name = Database::open(&same_name_path).unwrap();
        same_name
            .set_base_branch_if_missing("main", BaseBranchSource::LocalConvention)
            .unwrap();
        let observed = same_name.repo_meta().unwrap();
        same_name
            .reconcile_base_branch(&observed, "main", BaseBranchSource::RemoteHead, true)
            .unwrap();
        let meta = same_name.repo_meta().unwrap();
        assert_eq!(meta.base_branch, "main");
        assert_eq!(meta.base_branch_source, BaseBranchSource::RemoteHead);

        let authoritative_path = dir.path().join("authoritative.db");
        let authoritative = Database::open(&authoritative_path).unwrap();
        authoritative
            .set_base_branch_if_missing("main", BaseBranchSource::RemoteHead)
            .unwrap();
        let observed = authoritative.repo_meta().unwrap();
        authoritative
            .reconcile_base_branch(&observed, "trunk", BaseBranchSource::RemoteHead, true)
            .unwrap();
        let meta = authoritative.repo_meta().unwrap();
        assert_eq!(meta.base_branch, "main");
        assert_eq!(meta.base_branch_source, BaseBranchSource::RemoteHead);
    }

    #[test]
    fn base_reconciliation_migrates_the_tracked_root_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("stack.db")).unwrap();
        db.set_base_branch_if_missing("main", BaseBranchSource::LocalConvention)
            .unwrap();
        db.set_parent("feat/root", Some("main")).unwrap();
        db.set_parent("feat/child", Some("feat/root")).unwrap();
        db.set_parent("feat/sibling", Some("main")).unwrap();
        db.set_parent("trunk", Some("feat/root")).unwrap();
        let observed = db.repo_meta().unwrap();

        db.reconcile_base_branch(&observed, "trunk", BaseBranchSource::RemoteHead, true)
            .unwrap();

        let meta = db.repo_meta().unwrap();
        assert_eq!(meta.base_branch, "trunk");
        assert_eq!(meta.base_branch_source, BaseBranchSource::RemoteHead);
        assert!(db.branch_by_name("main").unwrap().is_none());
        let trunk = db.branch_by_name("trunk").unwrap().unwrap();
        let root = db.branch_by_name("feat/root").unwrap().unwrap();
        let child = db.branch_by_name("feat/child").unwrap().unwrap();
        let sibling = db.branch_by_name("feat/sibling").unwrap().unwrap();
        assert_eq!(trunk.parent_branch_id, None);
        assert_eq!(root.parent_branch_id, Some(trunk.id));
        assert_eq!(sibling.parent_branch_id, Some(trunk.id));
        assert_eq!(child.parent_branch_id, Some(root.id));
    }

    #[test]
    fn stale_branch_observation_does_not_overwrite_updated_base_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("stack.db")).unwrap();
        db.set_base_branch_if_missing("feat/work", BaseBranchSource::CurrentBranch)
            .unwrap();
        let observed = db.repo_meta().unwrap();

        db.conn
            .execute(
                "UPDATE repo_meta
                 SET base_branch = 'main', base_branch_source = 'local_convention'
                 WHERE id = 1",
                [],
            )
            .unwrap();
        db.reconcile_base_branch(&observed, "production", BaseBranchSource::RemoteHead, false)
            .unwrap();

        let meta = db.repo_meta().unwrap();
        assert_eq!(meta.base_branch, "main");
        assert_eq!(meta.base_branch_source, BaseBranchSource::LocalConvention);
    }

    #[test]
    fn opening_schema_v1_database_migrates_repository_placement_columns() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stack.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE repo_meta (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                base_branch TEXT NOT NULL,
                schema_version INTEGER NOT NULL
             );
             INSERT INTO repo_meta(id, base_branch, schema_version) VALUES (1, 'main', 1);",
        )
        .unwrap();
        drop(conn);

        let db = Database::open(&path).unwrap();
        let meta = db.repo_meta().unwrap();
        assert_eq!(meta.base_branch, "main");
        assert_eq!(meta.base_branch_source, BaseBranchSource::Legacy);
        assert!(meta.push_target.is_none());
        let version: i64 = db
            .conn
            .query_row(
                "SELECT schema_version FROM repo_meta WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
        assert!(meta.base_remote.is_none());

        db.reconcile_base_branch(&meta, "production", BaseBranchSource::RemoteHead, true)
            .unwrap();
        let reconciled = db.repo_meta().unwrap();
        assert_eq!(reconciled.base_branch, "production");
        assert_eq!(reconciled.base_branch_source, BaseBranchSource::RemoteHead);
    }

    #[test]
    fn opening_schema_v3_remote_head_backfills_origin_provenance() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stack.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE repo_meta (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                base_branch TEXT NOT NULL,
                base_branch_source TEXT NOT NULL,
                schema_version INTEGER NOT NULL
             );
             INSERT INTO repo_meta(id, base_branch, base_branch_source, schema_version)
             VALUES (1, 'main', 'remote_head', 3);",
        )
        .unwrap();
        drop(conn);

        let db = Database::open(&path).unwrap();
        let (base_remote, version): (Option<String>, i64) = db
            .conn
            .query_row(
                "SELECT base_remote, schema_version FROM repo_meta WHERE id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();

        assert_eq!(base_remote.as_deref(), Some("origin"));
        assert_eq!(version, SCHEMA_VERSION);
    }

    #[test]
    fn opening_schema_v4_adds_an_unbound_pr_head_cache_column() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stack.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE branches (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL UNIQUE,
                parent_branch_id INTEGER NULL,
                last_synced_head_sha TEXT NULL,
                cached_pr_number INTEGER NULL,
                cached_pr_state TEXT NULL,
                created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
             );
             CREATE TABLE repo_meta (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                base_branch TEXT NOT NULL,
                base_branch_source TEXT NOT NULL,
                base_remote TEXT NULL,
                schema_version INTEGER NOT NULL
             );
             INSERT INTO branches(name, cached_pr_number, cached_pr_state)
             VALUES ('feat/legacy', 11, 'merged');
             INSERT INTO repo_meta(id, base_branch, base_branch_source, base_remote, schema_version)
             VALUES (1, 'main', 'remote_head', 'origin', 4);",
        )
        .unwrap();
        drop(conn);

        let db = Database::open(&path).unwrap();
        let legacy = db.branch_by_name("feat/legacy").unwrap().unwrap();
        assert_eq!(legacy.cached_pr_number, Some(11));
        assert_eq!(legacy.cached_pr_state.as_deref(), Some("merged"));
        assert!(legacy.cached_pr_head_oid.is_none());
        let version: i64 = db
            .conn
            .query_row(
                "SELECT schema_version FROM repo_meta WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
    }

    #[test]
    fn opening_schema_v6_adds_per_branch_topology_revisions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stack.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE branches (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL UNIQUE,
                parent_branch_id INTEGER NULL,
                last_synced_head_sha TEXT NULL,
                cached_pr_number INTEGER NULL,
                cached_pr_state TEXT NULL,
                cached_pr_head_oid TEXT NULL,
                created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                FOREIGN KEY(parent_branch_id) REFERENCES branches(id) ON DELETE SET NULL
             );
             CREATE TABLE repo_meta (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                base_branch TEXT NOT NULL,
                base_branch_source TEXT NOT NULL,
                base_remote TEXT NULL,
                schema_version INTEGER NOT NULL,
                push_target TEXT NULL,
                canonical_repo TEXT NULL,
                fork_repo TEXT NULL,
                push_permission TEXT NULL,
                 permission_checked_at INTEGER NULL
             );
             CREATE TABLE topology_state (
                 id INTEGER PRIMARY KEY CHECK (id = 1),
                 revision INTEGER NOT NULL
                     CHECK (revision >= 0 AND typeof(revision) = 'integer')
             );
             INSERT INTO topology_state(id, revision) VALUES (1, 12);
             CREATE TRIGGER branches_topology_revision_after_insert
             AFTER INSERT ON branches
             BEGIN
                 UPDATE topology_state SET revision = revision + 1 WHERE id = 1;
             END;
             CREATE TRIGGER branches_topology_revision_after_delete
             AFTER DELETE ON branches
             BEGIN
                 UPDATE topology_state SET revision = revision + 1 WHERE id = 1;
             END;
             CREATE TRIGGER branches_topology_revision_after_update
             AFTER UPDATE OF name, parent_branch_id ON branches
             WHEN OLD.name IS NOT NEW.name
                  OR OLD.parent_branch_id IS NOT NEW.parent_branch_id
             BEGIN
                 UPDATE topology_state SET revision = revision + 1 WHERE id = 1;
             END;
             INSERT INTO branches(id, name) VALUES (1, 'main');
             INSERT INTO branches(id, name, parent_branch_id) VALUES (2, 'target', 1);
             UPDATE topology_state SET revision = 12 WHERE id = 1;
             INSERT INTO repo_meta(
                 id, base_branch, base_branch_source, base_remote, schema_version
             ) VALUES (1, 'main', 'remote_head', 'origin', 6);",
        )
        .unwrap();
        drop(conn);

        let db = Database::open(&path).unwrap();
        let target = db.branch_by_name("target").unwrap().unwrap();
        let main = db.branch_by_name("main").unwrap().unwrap();
        assert_eq!(target.parent_branch_id, Some(main.id));
        let (version, initial_revision, initial_target_revision): (i64, i64, i64) = db
            .conn
            .query_row(
                "SELECT repo_meta.schema_version, topology_state.revision,
                        branches.topology_revision
                 FROM repo_meta
                 CROSS JOIN topology_state
                 JOIN branches ON branches.name = 'target'
                 WHERE repo_meta.id = 1 AND topology_state.id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
        assert_eq!(initial_revision, 12);
        assert_eq!(initial_target_revision, 0);

        db.set_pr_cache("target", Some(42), Some("open"), Some("abc"))
            .unwrap();
        assert_eq!(topology_revision_on(&db.conn).unwrap(), initial_revision);
        assert_eq!(
            branch_topology_revision_on(&db.conn, target.id).unwrap(),
            initial_target_revision
        );
        db.set_parent("target", None).unwrap();
        assert!(topology_revision_on(&db.conn).unwrap() > initial_revision);
        let updated_target_revision = branch_topology_revision_on(&db.conn, target.id).unwrap();
        assert!(updated_target_revision > initial_target_revision);
        drop(db);

        let reopened = Database::open(&path).unwrap();
        assert_eq!(topology_revision_on(&reopened.conn).unwrap(), 13);
        assert_eq!(
            branch_topology_revision_on(&reopened.conn, target.id).unwrap(),
            updated_target_revision
        );
    }

    #[test]
    fn base_remote_provenance_is_persisted_and_cas_protected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stack.db");
        let db = Database::open(&path).unwrap();
        db.set_base_branch_if_missing("main", BaseBranchSource::LocalConvention)
            .unwrap();
        let observed = db.repo_meta().unwrap();

        db.reconcile_base_branch_with_remote(
            &observed,
            "trunk",
            BaseBranchSource::RemoteHead,
            Some("company"),
            true,
        )
        .unwrap();
        let authoritative = db.repo_meta().unwrap();
        assert_eq!(authoritative.base_branch, "trunk");
        assert_eq!(authoritative.base_remote.as_deref(), Some("company"));

        let stale = authoritative.clone();
        db.conn
            .execute(
                "UPDATE repo_meta SET base_remote = 'replacement' WHERE id = 1",
                [],
            )
            .unwrap();
        db.reconcile_base_branch_with_remote(
            &stale,
            "production",
            BaseBranchSource::RemoteHead,
            Some("other"),
            false,
        )
        .unwrap();
        let current = db.repo_meta().unwrap();
        assert_eq!(current.base_branch, "trunk");
        assert_eq!(current.base_remote.as_deref(), Some("replacement"));
    }

    #[test]
    fn concurrent_old_schema_opens_are_serialized() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stack.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE repo_meta (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                base_branch TEXT NOT NULL,
                schema_version INTEGER NOT NULL
             );
             INSERT INTO repo_meta(id, base_branch, schema_version) VALUES (1, 'main', 1);",
        )
        .unwrap();
        drop(conn);

        let worker_count = 16;
        let barrier = Arc::new(Barrier::new(worker_count));
        let workers = (0..worker_count)
            .map(|_| {
                let path = path.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    Database::open(&path).and_then(|db| db.repo_meta())
                })
            })
            .collect::<Vec<_>>();

        for worker in workers {
            let meta = worker.join().expect("migration worker").unwrap();
            assert_eq!(meta.base_branch, "main");
            assert_eq!(meta.base_branch_source, BaseBranchSource::Legacy);
        }
    }
}
