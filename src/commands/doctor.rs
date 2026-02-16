use std::collections::{HashMap, HashSet};

use anyhow::Result;

use crate::db::{BranchRecord, Database};
use crate::git::Git;
use crate::output::DoctorIssueView;

pub fn run(db: &Database, git: &Git, porcelain: bool, fix: bool) -> Result<()> {
    let records = db.list_branches()?;
    let mut issues = Vec::new();
    let mut id_to_name = HashMap::new();

    for branch in &records {
        id_to_name.insert(branch.id, branch.name.clone());
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
                db.clear_parent(&branch.name)?;
            }
        }
    }

    issues.extend(cycle_issues(&records));

    if porcelain {
        return crate::output::print_json(&serde_json::json!({ "issues": issues, "fix_applied": fix }));
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

fn cycle_issues(records: &[BranchRecord]) -> Vec<DoctorIssueView> {
    let mut issues = Vec::new();
    let mut by_id: HashMap<i64, &BranchRecord> = HashMap::new();
    for r in records {
        by_id.insert(r.id, r);
    }

    for r in records {
        let mut seen = HashSet::new();
        let mut cursor = r.parent_branch_id;
        while let Some(id) = cursor {
            if !seen.insert(id) {
                issues.push(DoctorIssueView {
                    severity: "error".to_string(),
                    code: "cycle".to_string(),
                    message: format!("cycle detected starting at '{}'", r.name),
                    branch: Some(r.name.clone()),
                });
                break;
            }
            cursor = by_id.get(&id).and_then(|p| p.parent_branch_id);
        }
    }

    issues
}
