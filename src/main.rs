// stargazer — TUI browser for GitHub starred repositories.
//
// Architecture:
//   - tokio::main for async runtime
//   - Main thread: ratatui event loop (draw + input)
//   - Background task: GitHub API sync via tokio::spawn
//   - Communication: tokio::sync::mpsc channels
//
// On first launch: full import of all stars, with progress in the title bar.
// Subsequent launches: load cache instantly, then incrementally sync new stars
// in the background while the UI is already interactive.

mod app;
mod categories;
mod github;
mod ui;

use app::{App, BackgroundMsg};
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use github::{StarCache, sync_stars};
use ratatui::{backend::CrosstermBackend, Terminal};
use std::io;
use std::time::Duration;
use tokio::sync::mpsc;

/// Fallback GitHub username if STARGAZER_USER env var is not set.
/// Users should set STARGAZER_USER=<their_github_username> or export it in their shell profile.
const DEFAULT_GITHUB_USER: &str = "doublegate";

// ── Terminal setup / teardown ─────────────────────────────────────────────────

fn setup_terminal() -> anyhow::Result<Terminal<CrosstermBackend<io::Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    Ok(Terminal::new(backend)?)
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) {
    // Best-effort — ignore errors during cleanup
    let _ = disable_raw_mode();
    let _ = execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    );
    let _ = terminal.show_cursor();
}

// ── Background sync task ──────────────────────────────────────────────────────

async fn bg_sync(tx: mpsc::UnboundedSender<BackgroundMsg>, username: String) {
    let tx2 = tx.clone();
    let result = sync_stars(&username, move |count, msg| {
        // UnboundedSender::send is synchronous — safe to call from within async context
        let _ = tx2.send(BackgroundMsg::Progress(format!(
            "⟳  {msg} ({count} fetched)"
        )));
    })
    .await;

    match result {
        Ok(cache) => {
            let _ = tx.send(BackgroundMsg::Done(Box::new(cache)));
        }
        Err(e) => {
            let _ = tx.send(BackgroundMsg::Error(e.to_string()));
        }
    }
}

// ── Main ──────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Resolve GitHub username: STARGAZER_USER env var → default
    let github_user = std::env::var("STARGAZER_USER")
        .unwrap_or_else(|_| DEFAULT_GITHUB_USER.to_string());

    // Load cache immediately for instant startup
    let cache = StarCache::load().unwrap_or_default();
    let has_cache = !cache.repos.is_empty();

    // Background channel — unbounded so the sync progress closure can call send() without blocking
    let (tx, rx) = mpsc::unbounded_channel::<BackgroundMsg>();

    // Spawn background sync
    let tx_clone = tx.clone();
    let user_clone = github_user.clone();
    tokio::spawn(async move {
        bg_sync(tx_clone, user_clone).await;
    });

    // Build app state
    let mut app = App::new(cache, rx, github_user.clone());
    if !has_cache {
        app.is_loading = true;
        app.status = "Performing first-time import — this may take a minute…".to_string();
    } else {
        app.status = format!(
            "{} stars loaded from cache · syncing in background…",
            app.total_count()
        );
    }

    // Setup terminal
    let mut terminal = setup_terminal()?;

    // Event loop
    let tick_rate = Duration::from_millis(80);
    let result = run_loop(&mut terminal, &mut app, tick_rate).await;

    // Always restore terminal, even on error
    restore_terminal(&mut terminal);

    result
}

// ── Event loop ────────────────────────────────────────────────────────────────

async fn run_loop(
    terminal: &mut ratatui::Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    tick_rate: Duration,
) -> anyhow::Result<()> {
    loop {
        // Drain background messages
        app.poll_background();

        // Draw frame
        terminal.draw(|f| ui::render(f, app))?;

        // Poll for input with timeout
        if event::poll(tick_rate)? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    handle_key(app, key.code);
                }
                Event::Resize(_, _) => {
                    // Ratatui handles resize automatically on next draw
                }
                _ => {}
            }
        }

        if app.should_quit {
            break;
        }
    }
    Ok(())
}

// ── Key handling ──────────────────────────────────────────────────────────────

fn handle_key(app: &mut App, key: KeyCode) {
    // Search mode: route most keys to the search buffer
    if app.searching {
        match key {
            KeyCode::Esc => {
                app.clear_search();
            }
            KeyCode::Enter => {
                app.searching = false;
                if let Err(e) = app.open_selected() {
                    app.status = format!("Could not open browser: {e}");
                }
            }
            KeyCode::Backspace => {
                app.pop_search_char();
            }
            KeyCode::Down => app.move_repo(1),
            KeyCode::Up => app.move_repo(-1),
            KeyCode::Char(c) => {
                app.push_search_char(c);
            }
            _ => {}
        }
        return;
    }

    // Normal mode
    match key {
        // Quit
        KeyCode::Char('q') | KeyCode::Char('Q') => {
            app.should_quit = true;
        }

        // Enter search mode
        KeyCode::Char('/') => {
            app.searching = true;
            app.active_panel = app::Panel::Repos;
        }

        // Open selected repo in browser
        KeyCode::Enter => {
            if let Err(e) = app.open_selected() {
                app.status = format!("Could not open browser: {e}");
            }
        }

        // Panel switching
        KeyCode::Right | KeyCode::Tab => {
            app.active_panel = app::Panel::Repos;
        }
        KeyCode::Left | KeyCode::BackTab => {
            app.active_panel = app::Panel::Categories;
        }

        // Navigation — route to active panel
        KeyCode::Up | KeyCode::Char('k') => match app.active_panel {
            app::Panel::Repos => app.move_repo(-1),
            app::Panel::Categories => app.move_category(-1),
        },
        KeyCode::Down | KeyCode::Char('j') => match app.active_panel {
            app::Panel::Repos => app.move_repo(1),
            app::Panel::Categories => app.move_category(1),
        },

        // Page up/down for repo list
        KeyCode::PageUp => app.move_repo(-15),
        KeyCode::PageDown => app.move_repo(15),

        // Jump to top/bottom
        KeyCode::Home | KeyCode::Char('g') => {
            app.repo_sel = 0;
            app.repo_scroll = 0;
        }
        KeyCode::End | KeyCode::Char('G') => {
            let last = app.visible_count().saturating_sub(1);
            app.repo_sel = last;
            app.adjust_scroll(20);
        }

        // Cycle sort mode
        KeyCode::Char('s') | KeyCode::Char('S') => {
            app.cycle_sort();
            app.status = format!("Sort: {}", app.sort_mode.label());
        }

        // Manual refresh (force re-sync)
        KeyCode::Char('r') | KeyCode::Char('R') => {
            app.status = "Manual refresh not available mid-session — restart to re-sync.".to_string();
        }

        // Clear search
        KeyCode::Esc => {
            app.clear_search();
        }

        _ => {}
    }
}
