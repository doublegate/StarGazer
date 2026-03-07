// ui.rs — Ratatui rendering for stargazer.
//
// Layout (three horizontal bands):
//
//   ┌─ title bar ──────────────────────────────────────────────────────────┐
//   │ ┌─ categories (25%) ─┐ ┌─ repo list (75%) ───────────────────────┐  │
//   │ │                    │ │                                           │  │
//   │ │                    │ │                                           │  │
//   │ └────────────────────┘ └───────────────────────────────────────────┘  │
//   ├─ search bar ─────────────────────────────────────────────────────────┤
//   └─ help / status bar ──────────────────────────────────────────────────┘

use crate::app::{App, Panel};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
    Frame,
};

// ── Palette ───────────────────────────────────────────────────────────────────

const C_TITLE: Color = Color::Cyan;
const C_SELECTED: Color = Color::LightYellow;
const C_ACTIVE_BORDER: Color = Color::Cyan;
const C_INACTIVE_BORDER: Color = Color::DarkGray;
const C_STARS: Color = Color::Yellow;
const C_LANG: Color = Color::Green;
const C_DIM: Color = Color::DarkGray;
const C_LOADING: Color = Color::LightMagenta;
const C_COUNT: Color = Color::LightBlue;

/// Format a star count compactly: 1_234_567 → "1.2M", 12_345 → "12.3k".
fn fmt_stars(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

/// Category icon mapping.
fn cat_icon(cat: &str) -> &'static str {
    match cat {
        "Rust" => "🦀",
        "AI / ML" => "🤖",
        "Security" => "🔐",
        "Systems / OS" => "⚙️",
        "Games / Emulation" => "🎮",
        "Hardware / FPGA" => "🔌",
        "TUI / GUI" => "🖥",
        "DevTools" => "🔧",
        "Networking" => "🌐",
        "Databases" => "🗄",
        "Web / Frontend" => "🌍",
        _ => "📦",
    }
}

// ── Main render entry point ───────────────────────────────────────────────────

pub fn render(f: &mut Frame, app: &mut App) {
    let area = f.area();

    // ── Outer layout: title | body | search | help ────────────────────────────
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),  // title bar
            Constraint::Min(5),     // body
            Constraint::Length(1),  // search bar
            Constraint::Length(1),  // help/status bar
        ])
        .split(area);

    render_title(f, app, outer[0]);
    render_body(f, app, outer[1]);
    render_search(f, app, outer[2]);
    render_help(f, app, outer[3]);
}

// ── Title bar ─────────────────────────────────────────────────────────────────

fn render_title(f: &mut Frame, app: &App, area: Rect) {
    let loading_badge = if app.is_loading {
        Span::styled(" ⟳ syncing… ", Style::default().fg(C_LOADING))
    } else {
        Span::raw("")
    };

    let total_span = Span::styled(
        format!(" ⭐ {} stars ", app.total_count()),
        Style::default().fg(C_COUNT).add_modifier(Modifier::BOLD),
    );

    let sort_span = Span::styled(
        format!(" [{}] ", app.sort_mode.label()),
        Style::default().fg(C_DIM),
    );

    let title = Paragraph::new(Line::from(vec![
        Span::styled(
            "  ✦ STARGAZER ",
            Style::default()
                .fg(C_TITLE)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            app.github_user.clone(),
            Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
        ),
        total_span,
        loading_badge,
        sort_span,
    ]));
    f.render_widget(title, area);
}

// ── Body (categories + repo list) ─────────────────────────────────────────────

fn render_body(f: &mut Frame, app: &mut App, area: Rect) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(24), Constraint::Percentage(76)])
        .split(area);

    render_categories(f, app, cols[0]);
    render_repos(f, app, cols[1]);
}

// ── Category sidebar ──────────────────────────────────────────────────────────

fn render_categories(f: &mut Frame, app: &App, area: Rect) {
    let is_active = app.active_panel == Panel::Categories;
    let border_style = if is_active {
        Style::default().fg(C_ACTIVE_BORDER)
    } else {
        Style::default().fg(C_INACTIVE_BORDER)
    };

    let block = Block::default()
        .title(Span::styled(" Categories ", Style::default().fg(C_TITLE)))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border_style);

    let inner = block.inner(area);

    let items: Vec<ListItem> = app
        .category_list
        .iter()
        .map(|(name, count)| {
            let icon = if name == "All" { "☆ " } else { cat_icon(name) };
            let display = format!("{icon}{name} ({count})");
            ListItem::new(Line::from(display))
        })
        .collect();

    let mut state = ListState::default();
    state.select(Some(app.category_sel));

    let list = List::new(items)
        .block(block)
        .highlight_style(
            Style::default()
                .fg(C_SELECTED)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ");

    f.render_stateful_widget(list, area, &mut state);
    let _ = inner; // used implicitly by block.inner above
}

// ── Repo list ─────────────────────────────────────────────────────────────────

fn render_repos(f: &mut Frame, app: &mut App, area: Rect) {
    let is_active = app.active_panel == Panel::Repos;
    let border_style = if is_active {
        Style::default().fg(C_ACTIVE_BORDER)
    } else {
        Style::default().fg(C_INACTIVE_BORDER)
    };

    let visible_count = app.visible_count();
    let cat_name = app
        .category_list
        .get(app.category_sel)
        .map(|(c, _)| c.as_str())
        .unwrap_or("All");

    let title = if app.search.is_empty() {
        format!(" {cat_name}  ({visible_count}) ")
    } else {
        format!(" {cat_name} · \"{}\"  ({visible_count}) ", app.search)
    };

    let block = Block::default()
        .title(Span::styled(title, Style::default().fg(C_TITLE)))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border_style);

    let inner = block.inner(area);
    let viewport_height = inner.height as usize;

    // Let the app know the real viewport size so it can clamp scroll correctly
    app.adjust_scroll(viewport_height);

    if app.visible.is_empty() {
        let placeholder = Paragraph::new(if app.all_repos.is_empty() {
            "  Loading stars…"
        } else {
            "  No repositories match your search."
        })
        .block(block)
        .style(Style::default().fg(C_DIM));
        f.render_widget(placeholder, area);
        return;
    }

    let window = app
        .visible
        .iter()
        .skip(app.repo_scroll)
        .take(viewport_height);

    let items: Vec<ListItem> = window
        .map(|&idx| {
            let repo = &app.all_repos[idx];
            let name_span = Span::styled(
                repo.full_name.clone(),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            );
            let stars_span = Span::styled(
                format!(" ⭐ {} ", fmt_stars(repo.stargazers_count)),
                Style::default().fg(C_STARS),
            );
            let lang_span = Span::styled(
                repo.language
                    .as_deref()
                    .map(|l| format!(" [{l}]"))
                    .unwrap_or_default(),
                Style::default().fg(C_LANG),
            );
            let cat_span = Span::styled(
                format!(" {} ", repo.category),
                Style::default().fg(C_DIM),
            );

            let first_line = Line::from(vec![name_span, stars_span, lang_span, cat_span]);

            let desc = repo
                .description
                .as_deref()
                .unwrap_or("")
                .chars()
                .take(90)
                .collect::<String>();
            let desc_line = Line::from(Span::styled(
                format!("  {desc}"),
                Style::default().fg(Color::Gray),
            ));

            ListItem::new(vec![first_line, desc_line])
        })
        .collect();

    // Map absolute repo_sel to position within the visible window
    let list_sel = if app.repo_sel >= app.repo_scroll {
        Some(app.repo_sel - app.repo_scroll)
    } else {
        None
    };
    let mut state = ListState::default();
    state.select(list_sel);

    let list = List::new(items)
        .block(block)
        .highlight_style(
            Style::default()
                .fg(C_SELECTED)
                .bg(Color::Reset)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ");

    f.render_stateful_widget(list, area, &mut state);
}

// ── Search bar ────────────────────────────────────────────────────────────────

fn render_search(f: &mut Frame, app: &App, area: Rect) {
    let (label, cursor) = if app.searching {
        (
            Span::styled("/ Search: ", Style::default().fg(C_ACTIVE_BORDER)),
            Span::styled("█", Style::default().fg(C_SELECTED)),
        )
    } else {
        (
            Span::styled("  Press / to search", Style::default().fg(C_DIM)),
            Span::raw(""),
        )
    };

    let query_span = Span::styled(app.search.clone(), Style::default().fg(Color::White));
    let bar = Paragraph::new(Line::from(vec![label, query_span, cursor]));
    f.render_widget(bar, area);
}

// ── Help/status bar ───────────────────────────────────────────────────────────

fn render_help(f: &mut Frame, app: &App, area: Rect) {
    let help = if app.searching {
        Paragraph::new(Line::from(vec![
            key("Esc"), txt(" cancel  "),
            key("Enter"), txt(" open  "),
            key("Backspace"), txt(" delete char"),
        ]))
    } else {
        Paragraph::new(Line::from(vec![
            key("↑↓"), txt(" repos  "),
            key("←→"), txt(" panels  "),
            key("Enter"), txt(" open in browser  "),
            key("/"), txt(" search  "),
            key("s"), txt(" sort  "),
            key("r"), txt(" refresh  "),
            key("q"), txt(" quit"),
        ]))
    };

    // Status message overrides if present
    let widget = if !app.status.is_empty() && !app.is_loading {
        Paragraph::new(Span::styled(
            format!("  {}", app.status),
            Style::default().fg(C_DIM),
        ))
    } else {
        help
    };

    f.render_widget(widget, area);
}

fn key(s: &str) -> Span<'_> {
    Span::styled(
        format!("[{s}]"),
        Style::default()
            .fg(Color::Black)
            .bg(C_INACTIVE_BORDER),
    )
}
fn txt(s: &'static str) -> Span<'static> {
    Span::styled(s, Style::default().fg(C_DIM))
}
