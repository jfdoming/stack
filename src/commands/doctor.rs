use std::collections::{HashMap, HashSet};

use anyhow::Result;

use crate::db::{BranchRecord, Database};
use crate::git::Git;
use crate::views::DoctorIssueView;

pub fn run(db: &Database, git: &Git, porcelain: bool, fix: bool) -> Result<()> {
    let mut records = db.list_branches()?;
    let base_branch = db.repo_meta()?.base_branch;
    let mut issues = Vec::new();
    let mut clear_parent_fixes: HashSet<String> = HashSet::new();
    let mut clear_pr_cache_fixes: HashSet<String> = HashSet::new();

    for branch in &records {
        if !git.branch_exists(&branch.name)? {
            issues.push(DoctorIssueView {
                severity: "error".to_string(),
                code: "missing_git_branch".to_string(),
                message: format!("tracked branch '{}' does not exist in git", branch.name),
                branch: Some(branch.name.clone()),
            });
            if fix {
                db.delete_branch(&branch.name)?;
            }
        }
    }

    if fix {
        records = db.list_branches()?;
    }

    let mut id_to_name = HashMap::new();
    for branch in &records {
        id_to_name.insert(branch.id, branch.name.clone());
    }

    for branch in &records {
        if let Some(pid) = branch.parent_branch_id
            && !id_to_name.contains_key(&pid)
        {
            issues.push(DoctorIssueView {
                severity: "error".to_string(),
                code: "missing_parent_record".to_string(),
                message: format!(
                    "branch '{}' points to unknown parent id {}",
                    branch.name, pid
                ),
                branch: Some(branch.name.clone()),
            });
            if fix {
                clear_parent_fixes.insert(branch.name.clone());
            }
        }
    }

    for branch in &records {
        if branch.name == base_branch && branch.parent_branch_id.is_some() {
            issues.push(DoctorIssueView {
                severity: "error".to_string(),
                code: "base_has_parent".to_string(),
                message: format!(
                    "base branch '{}' should not have a parent link",
                    branch.name
                ),
                branch: Some(branch.name.clone()),
            });
            if fix {
                clear_parent_fixes.insert(branch.name.clone());
            }
        }

        let has_pr_number = branch.cached_pr_number.is_some();
        let has_pr_state = branch.cached_pr_state.is_some();
        if has_pr_number != has_pr_state {
            issues.push(DoctorIssueView {
                severity: "warning".to_string(),
                code: "incomplete_pr_cache".to_string(),
                message: format!(
                    "branch '{}' has partial PR cache metadata; both number and state are required",
                    branch.name
                ),
                branch: Some(branch.name.clone()),
            });
            if fix {
                clear_pr_cache_fixes.insert(branch.name.clone());
            }
        }
    }

    let cycle_branches = cycle_branches(&records);
    for branch_name in &cycle_branches {
        issues.push(DoctorIssueView {
            severity: "error".to_string(),
            code: "cycle".to_string(),
            message: format!("cycle detected starting at '{}'", branch_name),
            branch: Some(branch_name.clone()),
        });
        if fix {
            clear_parent_fixes.insert(branch_name.clone());
        }
    }

    if fix {
        for branch_name in clear_parent_fixes {
            db.clear_parent(&branch_name)?;
        }
        for branch_name in clear_pr_cache_fixes {
            db.set_pr_cache(&branch_name, None, None)?;
        }
    }

    if porcelain {
        return crate::views::print_json(
            &serde_json::json!({ "issues": issues, "fix_applied": fix }),
        );
    }

    if issues.is_empty() {
        println!("doctor: no issues found");
    } else {
        println!("doctor: {} issue(s)", issues.len());
        for issue in &issues {
            println!("- [{}] {}: {}", issue.severity, issue.code, issue.message);
        }
    }
    if fix {
        println!("doctor maintenance applied where possible");
    }

    Ok(())
}

fn cycle_branches(records: &[BranchRecord]) -> HashSet<String> {
    let mut branches = HashSet::new();
    let mut by_id: HashMap<i64, &BranchRecord> = HashMap::new();
    for r in records {
        by_id.insert(r.id, r);
    }

    for r in records {
        let mut seen = HashSet::new();
        let mut cursor = r.parent_branch_id;
        while let Some(id) = cursor {
            if !seen.insert(id) {
                branches.insert(r.name.clone());
                break;
            }
            cursor = by_id.get(&id).and_then(|p| p.parent_branch_id);
        }
    }

    branches
}
