use std::path::Path;

use anyhow::{Context, Result, anyhow};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

#[derive(Debug, Clone)]
pub struct BranchRecord {
    pub id: i64,
    pub name: String,
    pub parent_branch_id: Option<i64>,
    pub last_synced_head_sha: Option<String>,
    pub cached_pr_number: Option<i64>,
    pub cached_pr_state: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RepoMeta {
    pub base_branch: String,
    pub base_branch_source: BaseBranchSource,
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

    fn is_provisional(self) -> bool {
        matches!(self, Self::CurrentBranch | Self::FirstLocal | Self::Default)
    }
}

#[derive(Debug, Clone)]
pub struct ParentUpdate {
    pub child_name: String,
    pub parent_name: Option<String>,
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
                created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                FOREIGN KEY(parent_branch_id) REFERENCES branches(id) ON DELETE SET NULL
            );
            CREATE TABLE IF NOT EXISTS repo_meta (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                base_branch TEXT NOT NULL,
                base_branch_source TEXT NOT NULL DEFAULT 'legacy',
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
            ",
        )?;
        for (name, ty) in [
            ("base_branch_source", "TEXT NOT NULL DEFAULT 'legacy'"),
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
        tx.execute("UPDATE repo_meta SET schema_version = 3 WHERE id = 1", [])?;
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

    pub fn set_base_branch_if_missing(
        &self,
        base_branch: &str,
        source: BaseBranchSource,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO repo_meta(id, base_branch, base_branch_source, schema_version)
             VALUES (1, ?1, ?2, 3)
             ON CONFLICT(id) DO NOTHING",
            params![base_branch, source.as_str()],
        )?;
        Ok(())
    }

    pub fn reconcile_base_branch(
        &self,
        observed: &RepoMeta,
        candidate: &str,
        candidate_source: BaseBranchSource,
        observed_branch_exists: bool,
    ) -> Result<()> {
        let should_replace = !observed_branch_exists
            || (observed.base_branch_source.is_provisional()
                && candidate_source == BaseBranchSource::RemoteHead);
        if should_replace {
            self.conn.execute(
                "UPDATE repo_meta
                 SET base_branch = ?1, base_branch_source = ?2
                 WHERE id = 1 AND base_branch = ?3 AND base_branch_source = ?4",
                params![
                    candidate,
                    candidate_source.as_str(),
                    observed.base_branch,
                    observed.base_branch_source.as_str()
                ],
            )?;
        }
        Ok(())
    }

    pub fn repo_meta(&self) -> Result<RepoMeta> {
        let raw = self
            .conn
            .query_row(
                "SELECT base_branch, base_branch_source, push_target, canonical_repo, fork_repo,
                        push_permission, permission_checked_at
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
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| anyhow!("repo metadata missing"))?;
        let source = BaseBranchSource::parse(&raw.1)
            .ok_or_else(|| anyhow!("invalid cached base branch source '{}'", raw.1))?;
        Ok(RepoMeta {
            base_branch: raw.0,
            base_branch_source: source,
            push_target: raw.2,
            canonical_repo: raw.3,
            fork_repo: raw.4,
            push_permission: raw.5,
            permission_checked_at: raw.6,
        })
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

    pub fn upsert_branch(&self, name: &str) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO branches(name) VALUES (?1)
             ON CONFLICT(name) DO UPDATE SET updated_at = CURRENT_TIMESTAMP",
            params![name],
        )?;
        self.branch_by_name(name)?
            .map(|b| b.id)
            .ok_or_else(|| anyhow!("failed to upsert branch {name}"))
    }

    pub fn branch_by_name(&self, name: &str) -> Result<Option<BranchRecord>> {
        self.conn
            .query_row(
                "SELECT id, name, parent_branch_id, last_synced_head_sha, cached_pr_number, cached_pr_state
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
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn list_branches(&self) -> Result<Vec<BranchRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, parent_branch_id, last_synced_head_sha, cached_pr_number, cached_pr_state
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
            });
        }
        Ok(out)
    }

    pub fn set_parent(&self, child_name: &str, parent_name: Option<&str>) -> Result<()> {
        let child_id = self.upsert_branch(child_name)?;
        let parent_id = if let Some(p) = parent_name {
            Some(self.upsert_branch(p)?)
        } else {
            None
        };
        if let Some(pid) = parent_id {
            self.ensure_no_cycle(child_id, pid)?;
        }
        self.conn.execute(
            "UPDATE branches SET parent_branch_id = ?1, updated_at = CURRENT_TIMESTAMP WHERE id = ?2",
            params![parent_id, child_id],
        )?;
        Ok(())
    }

    pub fn set_parents_batch(&self, updates: &[ParentUpdate]) -> Result<()> {
        if updates.is_empty() {
            return Ok(());
        }

        let existing = self.list_branches()?;
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

        let tx = self.conn.unchecked_transaction()?;
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

    fn ensure_no_cycle(&self, child_id: i64, mut parent_id: i64) -> Result<()> {
        loop {
            if parent_id == child_id {
                return Err(anyhow!("link would create a cycle"));
            }
            let next: Option<i64> = self
                .conn
                .query_row(
                    "SELECT parent_branch_id FROM branches WHERE id = ?1",
                    params![parent_id],
                    |row| row.get(0),
                )
                .optional()?
                .flatten();
            if let Some(n) = next {
                parent_id = n;
            } else {
                break;
            }
        }
        Ok(())
    }

    pub fn set_sync_sha(&self, branch_name: &str, sha: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE branches SET last_synced_head_sha = ?1, updated_at = CURRENT_TIMESTAMP WHERE name = ?2",
            params![sha, branch_name],
        )?;
        Ok(())
    }

    pub fn set_pr_cache(
        &self,
        branch_name: &str,
        number: Option<i64>,
        state: Option<&str>,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE branches SET cached_pr_number = ?1, cached_pr_state = ?2, updated_at = CURRENT_TIMESTAMP WHERE name = ?3",
            params![number, state, branch_name],
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
        self.conn.execute("UPDATE branches SET parent_branch_id = NULL WHERE parent_branch_id = (SELECT id FROM branches WHERE name = ?1)", params![branch_name])?;
        self.conn
            .execute("DELETE FROM branches WHERE name = ?1", params![branch_name])?;
        Ok(())
    }

    pub fn splice_out_branch(&self, branch_name: &str) -> Result<()> {
        let branch = self
            .branch_by_name(branch_name)?
            .ok_or_else(|| anyhow!("branch '{branch_name}' is not tracked"))?;
        self.conn.execute(
            "UPDATE branches SET parent_branch_id = ?1 WHERE parent_branch_id = ?2",
            params![branch.parent_branch_id, branch.id],
        )?;
        self.conn
            .execute("DELETE FROM branches WHERE id = ?1", params![branch.id])?;
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
    use std::sync::{Arc, Barrier};

    use super::*;

    #[test]
    fn prevents_cycles() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("stack.db")).unwrap();
        db.set_parent("b", Some("a")).unwrap();
        let err = db.set_parent("a", Some("b")).unwrap_err();
        assert!(err.to_string().contains("cycle"));
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
    fn authoritative_remote_head_replaces_only_provisional_base_discovery() {
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
        assert_eq!(meta.base_branch, "main");
        assert_eq!(meta.base_branch_source, BaseBranchSource::LocalConvention);
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
        assert_eq!(version, 3);
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
