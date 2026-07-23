use anyhow::Result;

use crate::args::{ConfigArgs, ConfigCommand};
use crate::core::placement_status;
use crate::db::Database;
use crate::git::Git;

pub fn run(db: &Database, git: &Git, args: &ConfigArgs, porcelain: bool) -> Result<()> {
    match &args.command {
        ConfigCommand::PushTarget(args) => {
            if let Some(target) = args.target {
                db.set_push_target(target.as_str())?;
            }
            let status = placement_status(db, git)?;
            if porcelain {
                return crate::views::print_json(&status);
            }
            println!(
                "push target: {}",
                status.push_target.as_deref().unwrap_or("not configured")
            );
            println!(
                "resolved target: {}",
                status
                    .resolved_target
                    .map(|target| target.as_str())
                    .unwrap_or("not resolved")
            );
            if let Some(repository) = status.canonical_repository.as_deref() {
                println!(
                    "upstream: {}{}",
                    repository,
                    status
                        .canonical_remote
                        .as_deref()
                        .map(|remote| format!(" ({remote})"))
                        .unwrap_or_default()
                );
            }
            if let Some(repository) = status.fork_repository.as_deref() {
                println!(
                    "fork: {}{}",
                    repository,
                    status
                        .fork_remote
                        .as_deref()
                        .map(|remote| format!(" ({remote})"))
                        .unwrap_or_default()
                );
            }
            println!(
                "cached upstream permission: {}",
                status.push_permission.as_deref().unwrap_or("not detected")
            );
            let age = status
                .cache_age_seconds
                .map(|seconds| format!(" ({seconds}s old)"))
                .unwrap_or_default();
            println!("cached detection: {}{age}", status.cache_state);
            Ok(())
        }
    }
}
