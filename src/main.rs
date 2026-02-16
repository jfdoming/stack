mod app;
mod args;
mod commands;
mod core;
mod db;
mod git;
mod provider;
mod views;

mod ui;
mod util;

use anyhow::Result;
use crossterm::style::Stylize;

fn main() -> Result<()> {
    if let Err(err) = app::run() {
        if err
            .downcast_ref::<ui::interaction::UserCancelled>()
            .is_some()
        {
            eprintln!("\n{}", "cancelled by user".red().bold());
            std::process::exit(130);
        }
        return Err(err);
    }
    Ok(())
}
