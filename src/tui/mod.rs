use std::io;

use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    widgets::{Block, Borders, List, ListItem, Paragraph},
};

use crate::output::BranchView;

pub fn run_stack_tui(branches: &[BranchView]) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut selected: usize = 0;
    loop {
        terminal.draw(|f| {
            let chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
                .split(f.area());

            let items: Vec<ListItem> = if branches.is_empty() {
                vec![ListItem::new("(no stack branches tracked)")]
            } else {
                branches
                    .iter()
                    .map(|b| {
                        let parent = b.parent.as_deref().unwrap_or("<root>");
                        let pr = b.cached_pr_state.as_deref().unwrap_or("none");
                        ListItem::new(format!("{}  parent:{}  pr:{}", b.name, parent, pr))
                    })
                    .collect()
            };

            let list = List::new(items)
                .block(Block::default().title("Stack Branches").borders(Borders::ALL))
                .highlight_symbol("-> ");

            let details = if let Some(branch) = branches.get(selected.min(branches.len().saturating_sub(1))) {
                let parent = branch.parent.as_deref().unwrap_or("<root>");
                let pr_num = branch.cached_pr_number.map(|n| n.to_string()).unwrap_or_else(|| "none".to_string());
                let pr_state = branch.cached_pr_state.as_deref().unwrap_or("unknown");
                let synced = branch.last_synced_head_sha.as_deref().unwrap_or("unknown");
                format!(
                    "Branch: {}\nParent: {}\nPR: #{} ({})\nLast synced SHA: {}\nExists in git: {}\n\nKeys: j/k or arrows to move, q to quit",
                    branch.name, parent, pr_num, pr_state, synced, branch.exists_in_git
                )
            } else {
                "No branch selected\n\nKeys: q to quit".to_string()
            };

            let paragraph = Paragraph::new(details)
                .block(Block::default().title("Details").borders(Borders::ALL));

            let mut state = ratatui::widgets::ListState::default();
            if !branches.is_empty() {
                state.select(Some(selected.min(branches.len() - 1)));
            }

            f.render_stateful_widget(list, chunks[0], &mut state);
            f.render_widget(paragraph, chunks[1]);
        })?;

        if event::poll(std::time::Duration::from_millis(250))? {
            match event::read()? {
                Event::Key(key) => match key.code {
                    KeyCode::Char('q') => break,
                    KeyCode::Down | KeyCode::Char('j') => {
                        if !branches.is_empty() {
                            selected = (selected + 1).min(branches.len() - 1);
                        }
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        selected = selected.saturating_sub(1);
                    }
                    _ => {}
                },
                _ => {}
            }
        }
    }

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}
