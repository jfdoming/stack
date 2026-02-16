use std::collections::{HashMap, HashSet};
use std::io;

use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
};

use crate::views::BranchView;

pub fn run_stack_tui(branches: &[BranchView]) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let ordered = build_tree_rows(branches);
    let mut selected: usize = 0;

    loop {
        terminal.draw(|f| {
            let chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(58), Constraint::Percentage(42)])
                .split(f.area());

            let items: Vec<ListItem<'_>> = if ordered.is_empty() {
                vec![ListItem::new(Line::from("(no stack branches tracked)"))]
            } else {
                ordered.iter().map(to_list_item).collect()
            };

            let list = List::new(items)
                .block(
                    Block::default()
                        .title("Stack Graph (Interactive)")
                        .borders(Borders::ALL),
                )
                .highlight_symbol("▶ ")
                .highlight_style(Style::default().add_modifier(Modifier::BOLD));

            let details = if let Some(row) = ordered.get(selected.min(ordered.len().saturating_sub(1))) {
                let branch = row.branch;
                let parent = branch.parent.as_deref().unwrap_or("<root>");
                let pr_num = branch
                    .cached_pr_number
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "none".to_string());
                let pr_state = branch.cached_pr_state.as_deref().unwrap_or("unknown");
                let synced = branch
                    .last_synced_head_sha
                    .as_deref()
                    .unwrap_or("unknown");
                format!(
                    "Branch: {}\nParent: {}\nPR: #{} ({})\nLast synced SHA: {}\nExists in git: {}\n\nKeys: j/k or arrows to move, q or Ctrl-C to quit",
                    branch.name, parent, pr_num, pr_state, synced, branch.exists_in_git
                )
            } else {
                "No branch selected\n\nKeys: q or Ctrl-C to quit".to_string()
            };

            let paragraph = Paragraph::new(details)
                .block(Block::default().title("Details").borders(Borders::ALL));

            let mut state = ListState::default();
            if !ordered.is_empty() {
                state.select(Some(selected.min(ordered.len() - 1)));
            }

            f.render_stateful_widget(list, chunks[0], &mut state);
            f.render_widget(paragraph, chunks[1]);
        })?;

        if event::poll(std::time::Duration::from_millis(250))?
            && let Event::Key(key) = event::read()?
        {
            match key.code {
                KeyCode::Char('q') => break,
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => break,
                KeyCode::Down | KeyCode::Char('j') => {
                    if !ordered.is_empty() {
                        selected = (selected + 1).min(ordered.len() - 1);
                    }
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    selected = selected.saturating_sub(1);
                }
                _ => {}
            }
        }
    }

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

struct TreeRow<'a> {
    connector: String,
    branch: &'a BranchView,
}

fn build_tree_rows(branches: &[BranchView]) -> Vec<TreeRow<'_>> {
    let mut by_name: HashMap<&str, &BranchView> = HashMap::new();
    let mut children: HashMap<String, Vec<&BranchView>> = HashMap::new();

    for branch in branches {
        by_name.insert(&branch.name, branch);
    }

    let mut roots: Vec<&BranchView> = Vec::new();
    for branch in branches {
        match branch.parent.as_deref() {
            Some(parent) if by_name.contains_key(parent) => {
                children.entry(parent.to_string()).or_default().push(branch);
            }
            _ => roots.push(branch),
        }
    }

    roots.sort_by(|a, b| a.name.cmp(&b.name));
    for vals in children.values_mut() {
        vals.sort_by(|a, b| a.name.cmp(&b.name));
    }

    let mut rows = Vec::new();
    let mut seen = HashSet::new();

    fn walk<'a>(
        rows: &mut Vec<TreeRow<'a>>,
        children: &HashMap<String, Vec<&'a BranchView>>,
        node: &'a BranchView,
        prefix: &str,
        is_last: bool,
        seen: &mut HashSet<&'a str>,
    ) {
        if !seen.insert(node.name.as_str()) {
            return;
        }

        let connector = if is_last {
            format!("{prefix}└─ ")
        } else {
            format!("{prefix}├─ ")
        };

        rows.push(TreeRow {
            connector,
            branch: node,
        });

        let next_prefix = if prefix.is_empty() {
            String::new()
        } else if is_last {
            format!("{prefix}   ")
        } else {
            format!("{prefix}│  ")
        };

        if let Some(kids) = children.get(&node.name) {
            for (idx, child) in kids.iter().enumerate() {
                walk(
                    rows,
                    children,
                    child,
                    &next_prefix,
                    idx + 1 == kids.len(),
                    seen,
                );
            }
        }
    }

    for root in &roots {
        rows.push(TreeRow {
            connector: "● ".to_string(),
            branch: root,
        });

        if let Some(kids) = children.get(&root.name) {
            for (kidx, child) in kids.iter().enumerate() {
                walk(
                    &mut rows,
                    &children,
                    child,
                    "",
                    kidx + 1 == kids.len(),
                    &mut seen,
                );
            }
        }
    }

    rows
}

fn to_list_item(row: &TreeRow<'_>) -> ListItem<'static> {
    let mut spans = Vec::new();
    spans.push(Span::styled(
        row.connector.clone(),
        Style::default().fg(Color::DarkGray),
    ));
    spans.push(Span::styled(
        row.branch.name.clone(),
        Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD),
    ));

    let pr = row.branch.cached_pr_state.as_deref().unwrap_or("none");
    let (pr_label, pr_color) = match pr {
        "open" => (" PR:open", Color::Yellow),
        "merged" => (" PR:merged", Color::Green),
        "closed" => (" PR:closed", Color::Red),
        _ => ("", Color::DarkGray),
    };
    if !pr_label.is_empty() {
        spans.push(Span::styled(
            pr_label,
            Style::default().fg(pr_color).add_modifier(Modifier::BOLD),
        ));
    }

    let sync = if row.branch.last_synced_head_sha.is_some() {
        (" SYNC:tracked", Color::Cyan)
    } else {
        (" SYNC:never", Color::Magenta)
    };
    spans.push(Span::styled(sync.0, Style::default().fg(sync.1)));

    ListItem::new(Line::from(spans))
}
