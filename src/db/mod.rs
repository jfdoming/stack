use std::path::Path;

use anyhow::{Context, Result, anyhow};
use rusqlite::{Connection, OptionalExtension, params};

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
        let db = Self { conn };
        db.migrate()?;
        Ok(db)
    }

    fn migrate(&self) -> Result<()> {
        self.conn.execute_batch(
            "
            PRAGMA foreign_keys = ON;
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
                schema_version INTEGER NOT NULL
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
        Ok(())
    }

    pub fn set_base_branch_if_missing(&self, base_branch: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO repo_meta(id, base_branch, schema_version)
             VALUES (1, ?1, 1)
             ON CONFLICT(id) DO NOTHING",
            params![base_branch],
        )?;
        Ok(())
    }

    pub fn repo_meta(&self) -> Result<RepoMeta> {
        self.conn
            .query_row(
                "SELECT base_branch FROM repo_meta WHERE id = 1",
                [],
                |row| {
                    Ok(RepoMeta {
                        base_branch: row.get(0)?,
                    })
                },
            )
            .optional()?
            .ok_or_else(|| anyhow!("repo metadata missing"))
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
}
