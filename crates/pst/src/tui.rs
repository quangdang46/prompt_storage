//! TUI picker (bead P5.1) — `pst i`, ratatui, thin core consumer.
//!
//! Fast-first picker per plan §10: type-to-filter using the same core search
//! as `pst search`; Enter copies + exits; `p` prints to stdout after exit;
//! Ctrl-C/q aborts. Terminal state always restored (panic hook included).

use std::io::{self, Stdout};

use anyhow::Result;
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    crossterm::{
        event::{self, Event, KeyCode, KeyEventKind},
        execute,
        terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
    },
};

use crate::model::PromptSummary;

/// What the user chose when the TUI exited.
#[derive(Debug)]
pub enum TuiAction {
    Copy(String),
    Print(String),
    Abort,
}

struct App<'a> {
    prompts: &'a [PromptSummary],
    query: String,
    filtered: Vec<usize>,
    selected: usize,
    preview: Option<String>,
    status: Option<String>,
    done: bool,
}

impl<'a> App<'a> {
    fn new(prompts: &'a [PromptSummary]) -> Self {
        let mut app = Self {
            prompts,
            query: String::new(),
            filtered: (0..prompts.len()).collect(),
            selected: 0,
            preview: None,
            status: None,
            done: false,
        };
        app.refilter();
        app
    }

    fn refilter(&mut self) {
        self.filtered.clear();
        for (i, p) in self.prompts.iter().enumerate() {
            if p.id.contains(&self.query)
                || p.title.to_lowercase().contains(&self.query.to_lowercase())
                || p.tags.iter().any(|t| t.contains(&self.query))
            {
                self.filtered.push(i);
            }
        }
        if self.selected >= self.filtered.len() {
            self.selected = self.filtered.len().saturating_sub(1);
        }
    }

    fn current(&self) -> Option<&PromptSummary> {
        self.filtered.get(self.selected).map(|i| &self.prompts[*i])
    }
}

fn ui(f: &mut ratatui::Frame, app: &App) {
    use ratatui::layout::{Constraint, Layout};
    use ratatui::style::{Modifier, Style};
    use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};

    let chunks = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(5),
        Constraint::Length(1),
    ])
    .split(f.area());

    // Search bar.
    f.render_widget(
        Paragraph::new(format!("Search: {}", app.query))
            .block(Block::default().borders(Borders::ALL)),
        chunks[0],
    );

    // Results list.
    let items: Vec<ListItem> = app
        .filtered
        .iter()
        .map(|i| {
            let p = &app.prompts[*i];
            ListItem::new(format!("{:<28} {}", p.id, p.title))
        })
        .collect();
    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title("Prompts"))
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol("> ");
    let mut state = ListState::default();
    if !app.filtered.is_empty() {
        state.select(Some(app.selected));
    }
    f.render_stateful_widget(list, chunks[1], &mut state);

    // Status line.
    let status = app
        .status
        .clone()
        .or_else(|| {
            app.current().map(|p| {
                format!(
                    "{} — Enter: copy · p: print · o: preview · Esc: clear · q: quit",
                    p.id
                )
            })
        })
        .unwrap_or_default();
    f.render_widget(Paragraph::new(status), chunks[2]);
}

fn restore_terminal(mut term: Terminal<CrosstermBackend<Stdout>>) {
    let _ = disable_raw_mode();
    let _ = execute!(term.backend_mut(), LeaveAlternateScreen);
    let _ = term.show_cursor();
}

/// Run the interactive picker. Returns the chosen action.
/// Caller decides what to do with Copy/Print payloads (post-terminal-restore).
pub fn run_tui(db: &crate::storage::database::Database) -> Result<TuiAction> {
    if !atty::is(atty::Stream::Stdin) || !atty::is(atty::Stream::Stdout) {
        anyhow::bail!("tty_required");
    }

    let summaries = db.list_summaries()?;
    let mut app = App::new(&summaries);

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut term = Terminal::new(backend)?;

    // Panic hook so a crash never leaves a broken terminal.
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        original_hook(info);
    }));

    let mut action: Option<TuiAction> = None;
    loop {
        term.draw(|f| ui(f, &app))?;

        if let Event::Key(key) = event::read()? {
            if key.kind != KeyEventKind::Press {
                continue;
            }
            match key.code {
                KeyCode::Char('q') if app.query.is_empty() => break,
                KeyCode::Char('p') => {
                    if let Some(p) = app.current() {
                        action = Some(TuiAction::Print(p.id.clone()));
                        break;
                    }
                    app.status = Some("nothing selected".into());
                }
                KeyCode::Char(c) => {
                    app.query.push(c);
                    app.refilter();
                    app.status = None;
                }
                KeyCode::Backspace => {
                    app.query.pop();
                    app.refilter();
                    app.status = None;
                }
                KeyCode::Down => {
                    if app.selected + 1 < app.filtered.len() {
                        app.selected += 1;
                        app.preview = None;
                    }
                }
                KeyCode::Up => {
                    app.selected = app.selected.saturating_sub(1);
                    app.preview = None;
                }
                KeyCode::Esc => {
                    app.query.clear();
                    app.refilter();
                    app.preview = None;
                }
                KeyCode::Enter => {
                    if let Some(p) = app.current() {
                        action = Some(TuiAction::Copy(p.id.clone()));
                        break;
                    }
                    app.status = Some("nothing selected".into());
                }
                _ => {}
            }
        }
        if app.done {
            break;
        }
    }

    restore_terminal(term);
    // Reinstall default panic behavior post-TUI.
    let _ = std::panic::take_hook();

    Ok(match action {
        Some(TuiAction::Copy(id)) => {
            let content = db.get_prompt(&id)?.map(|p| p.content).unwrap_or_default();
            match crate::clipboard::copy_to_clipboard(&content) {
                Ok(()) => TuiAction::Copy(content),
                Err(e) => {
                    eprintln!(
                        "{}",
                        serde_json::json!({"error":"clipboard_failed","message":e})
                    );
                    TuiAction::Abort
                }
            }
        }
        Some(TuiAction::Print(id)) => {
            let content = db.get_prompt(&id)?.map(|p| p.content).unwrap_or_default();
            TuiAction::Print(content)
        }
        other => other.unwrap_or(TuiAction::Abort),
    })
}
