use std::io::Write;

use anyhow::{Context, Result};
use crossterm::cursor::{Hide, MoveToColumn, RestorePosition, SavePosition, Show};
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use crossterm::execute;
use crossterm::style::Stylize;
use crossterm::terminal::{Clear, ClearType, disable_raw_mode, enable_raw_mode};
use dialoguer::console::Term;
use dialoguer::{Select, theme::ColorfulTheme};
use thiserror::Error;

#[derive(Debug, Error)]
#[error("cancelled by user")]
pub struct UserCancelled;

pub fn prompt_or_cancel<T>(result: dialoguer::Result<T>) -> Result<T> {
    match result {
        Ok(value) => Ok(value),
        Err(err) => {
            let _ = Term::stdout().show_cursor();
            let _ = Term::stderr().show_cursor();
            match err {
                dialoguer::Error::IO(io_err)
                    if io_err.kind() == std::io::ErrorKind::Interrupted =>
                {
                    Err(UserCancelled.into())
                }
                other => Err(other.into()),
            }
        }
    }
}

pub fn confirm_inline_yes_no(prompt: &str) -> Result<bool> {
    if !should_use_inline_confirm(prompt)? {
        return confirm_select_yes_no(prompt);
    }

    let mut out = std::io::stdout();
    enable_raw_mode().context("failed to enable raw mode for inline confirm")?;
    execute!(out, Hide).context("failed to hide cursor for inline confirm")?;
    execute!(out, SavePosition).context("failed to save cursor for inline confirm")?;

    let result = (|| -> Result<bool> {
        let mut yes_selected = true;
        loop {
            execute!(
                out,
                RestorePosition,
                MoveToColumn(0),
                Clear(ClearType::FromCursorDown)
            )
            .context("failed to clear inline confirm area")?;
            write!(out, "{prompt}  ").context("failed to write prompt")?;

            let yes = if yes_selected {
                format!("{} {}", "●".green().bold(), "Yes".green().bold())
            } else {
                format!("{} {}", "○".dark_grey(), "Yes")
            };
            let no = if yes_selected {
                format!("{} {}", "○".dark_grey(), "No")
            } else {
                format!("{} {}", "●".yellow().bold(), "No".yellow().bold())
            };
            write!(out, "{yes}   {no}").context("failed to write options")?;
            out.flush().context("failed to flush inline confirm")?;

            if let Event::Key(key) =
                event::read().context("failed to read key for inline confirm")?
            {
                match key.code {
                    KeyCode::Left | KeyCode::Up => yes_selected = true,
                    KeyCode::Right | KeyCode::Down => yes_selected = false,
                    KeyCode::Tab => yes_selected = !yes_selected,
                    KeyCode::Enter => return Ok(yes_selected),
                    KeyCode::Esc => return Err(UserCancelled.into()),
                    KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        return Err(UserCancelled.into());
                    }
                    _ => {}
                }
            }
        }
    })();

    let _ = execute!(out, RestorePosition, Show, Clear(ClearType::FromCursorDown));
    let _ = disable_raw_mode();
    let _ = writeln!(out);
    result
}

pub fn should_use_inline_confirm(prompt: &str) -> Result<bool> {
    let (width, _) = crossterm::terminal::size().context("failed to read terminal size")?;
    let prompt_len = prompt.chars().count();
    let min_options_len = "  ○ Yes   ○ No".chars().count();
    Ok(prompt_len + min_options_len < width as usize)
}

pub fn confirm_select_yes_no(prompt: &str) -> Result<bool> {
    let theme = ColorfulTheme::default();
    let options = ["Yes", "No"];
    let idx = prompt_or_cancel(
        Select::with_theme(&theme)
            .with_prompt(prompt)
            .items(&options)
            .default(0)
            .interact(),
    )?;
    Ok(idx == 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_interrupt_maps_to_user_cancelled_error() {
        let err = dialoguer::Error::IO(std::io::Error::new(
            std::io::ErrorKind::Interrupted,
            "ctrl-c",
        ));
        let result = prompt_or_cancel::<()>(Err(err));
        assert!(result.is_err());
        let got = result.unwrap_err();
        assert!(got.downcast_ref::<UserCancelled>().is_some());
    }

    #[test]
    fn long_prompt_exceeds_example_width_budget() {
        let prompt = "Open PR from 'main' into 'main' even though the branch is not stacked?";
        let prompt_len = prompt.chars().count();
        let min_options_len = "  ○ Yes   ○ No".chars().count();
        assert!(prompt_len + min_options_len > 74);
    }
}
