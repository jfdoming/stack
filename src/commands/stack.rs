use std::collections::HashMap;
use std::io::{IsTerminal, stdin, stdout};

use anyhow::Result;

use crate::core::render_tree;
use crate::db::{BranchRecord, Database};
use crate::git::Git;
use crate::ui::tui;
use crate::views::{BranchView, print_json};

pub fn run(
    db: &Database,
    git: &Git,
    porcelain: bool,
    interactive: bool,
    base_branch: &str,
    base_remote: &str,
) -> Result<()> {
    let records = db.list_branches()?;
    let branch_views = to_branch_views(git, &records)?;

    if porcelain {
        return print_json(&branch_views);
    }

    let is_tty = stdout().is_terminal() && stdin().is_terminal();
    if interactive && is_tty {
        return tui::run_stack_tui(&branch_views);
    }

    let should_color = is_tty && std::env::var_os("NO_COLOR").is_none();
    let pr_base_url = git.remote_web_url(base_remote)?;
    println!(
        "{}",
        render_tree(&records, should_color, pr_base_url.as_deref(), base_branch)
    );
    Ok(())
}

fn to_branch_views(git: &Git, records: &[BranchRecord]) -> Result<Vec<BranchView>> {
    let mut id_map: HashMap<i64, String> = HashMap::new();
    for rec in records {
        id_map.insert(rec.id, rec.name.clone());
    }

    records
        .iter()
        .map(|rec| {
            let exists_in_git = git.branch_exists(&rec.name)?;
            Ok(BranchView {
                name: rec.name.clone(),
                parent: rec.parent_branch_id.and_then(|id| id_map.get(&id).cloned()),
                last_synced_head_sha: rec.last_synced_head_sha.clone(),
                cached_pr_number: rec.cached_pr_number,
                cached_pr_state: rec.cached_pr_state.clone(),
                exists_in_git,
            })
        })
        .collect()
}
