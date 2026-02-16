use std::io::{IsTerminal, stdin, stdout};

use anyhow::{Result, anyhow};
use clap::CommandFactory;
use dialoguer::{Select, theme::ColorfulTheme};

use crate::args::Cli;
use crate::ui::interaction::prompt_or_cancel;

pub fn run(shell: Option<clap_complete::Shell>) -> Result<()> {
    let shell = if let Some(shell) = shell {
        shell
    } else if stdout().is_terminal() && stdin().is_terminal() {
        let theme = ColorfulTheme::default();
        let shells = [
            clap_complete::Shell::Bash,
            clap_complete::Shell::Zsh,
            clap_complete::Shell::Fish,
            clap_complete::Shell::Elvish,
            clap_complete::Shell::PowerShell,
        ];
        let labels = ["bash", "zsh", "fish", "elvish", "powershell"];
        let idx = prompt_or_cancel(
            Select::with_theme(&theme)
                .with_prompt("Select shell for completion script")
                .items(&labels)
                .default(1)
                .interact(),
        )?;
        shells[idx]
    } else {
        return Err(anyhow!(
            "shell required in non-interactive mode; pass stack completions <shell>"
        ));
    };

    let mut cmd = Cli::command();
    let bin_name = cmd.get_name().to_string();
    clap_complete::generate(shell, &mut cmd, bin_name, &mut std::io::stdout());
    Ok(())
}
