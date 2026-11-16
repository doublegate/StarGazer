//! Rendering.
//!
//! ```text
//!  ✦ STARGAZER user · ★ 645 · Stars · lang:Rust              ⠋ syncing
//! ╭ Categories ──────╮╭ Rust (61) · 3/61 ──────────────────────────────╮
//! │▸ 🌟 All (645)    ││▸ owner/repo ★ 12.3k [Rust]                     ║
//! │  🦀 Rust (61)    ││    description…                                ║
//! ╰──────────────────╯╰────────────────────────────────────────────────╯
//!  status / search line
//!  key hints
//! ```
//!
//! All dynamic text is width-checked: repo strings are sanitized at ingest
//! (see [`crate::text`]) and every line is truncated to its pane's inner width
//! by display columns, so nothing ever spills into a border.

use crate::app::{App, Focus, Level, Overlay, REPO_ITEM_HEIGHT, Readme};
use crate::cache::StarredRepo;
use crate::text::{sanitize, truncate_width, width};
use crate::theme::{Glyphs, Theme};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Margin, Position, Rect},
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{
        Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph, Scrollbar,
        ScrollbarOrientation, ScrollbarState, Wrap,
    },
};

/// Renderer configuration.
pub struct Ui {
    pub theme: Theme,
    pub glyphs: Glyphs,
}

/// Compact star count: 1_234_567 -> "1.2M", 12_345 -> "12.3k".
pub fn fmt_stars(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

/// Keep as many spans as fit in `max` columns, truncating the last partial one.
fn fit_spans<'a>(spans: Vec<Span<'a>>, max: usize, ellipsis: &str) -> Vec<Span<'a>> {
    let mut out = Vec::with_capacity(spans.len());
    let mut used = 0;
    for span in spans {
        let w = width(&span.content);
        if used + w <= max {
            used += w;
            out.push(span);
        } else {
            let rest = max - used;
            if rest > 0 {
                let cut = truncate_width(&span.content, rest, ellipsis);
                out.push(Span::styled(cut, span.style));
            }
            break;
        }
    }
    out
}

impl Ui {
    pub fn new(theme: Theme, glyphs: Glyphs) -> Self {
        Self { theme, glyphs }
    }

    pub fn render(&self, f: &mut Frame, app: &mut App) {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(4),
                Constraint::Length(1),
                Constraint::Length(1),
            ])
            .split(f.area());

        self.render_title(f, app, rows[0]);
        self.render_body(f, app, rows[1]);
        self.render_status(f, app, rows[2]);
        self.render_hints(f, app, rows[3]);

        match &app.overlay {
            Overlay::None => {}
            Overlay::Help => self.render_help(f),
            Overlay::Languages { items, selected } => {
                let items = items.clone();
                self.render_languages(f, &items, *selected);
            }
            Overlay::ConfirmUnstar { full_name, .. } => {
                let name = full_name.clone();
                self.render_confirm(f, &name);
            }
        }
    }

    // ── Title ───────────────────────────────────────────────────────────────

    fn render_title(&self, f: &mut Frame, app: &App, area: Rect) {
        let t = &self.theme;
        let g = &self.glyphs;
        let sep = format!(" {} ", g.sep);
        let mut spans = vec![
            Span::styled(
                format!(" {} STARGAZER ", g.logo),
                t.fg(t.accent).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                sanitize(&app.user),
                t.fg(t.text).add_modifier(Modifier::BOLD),
            ),
            Span::styled(sep.clone(), t.fg(t.dim)),
            Span::styled(
                format!("{} {} stars", g.star, app.total()),
                t.fg(t.stars).add_modifier(Modifier::BOLD),
            ),
            Span::styled(sep.clone(), t.fg(t.dim)),
            Span::styled(format!("[{}]", app.sort.label(g.ascii)), t.fg(t.dim)),
        ];
        let filters = app.filter_summary();
        if !filters.is_empty() {
            spans.push(Span::styled(sep, t.fg(t.dim)));
            spans.push(Span::styled(filters, t.fg(t.warning)));
        }
        if app.syncing {
            let frame = g.spinner[app.frame % g.spinner.len()];
            spans.push(Span::styled(format!("  {frame} syncing"), t.fg(t.accent)));
        }
        let spans = fit_spans(spans, area.width as usize, g.ellipsis);
        f.render_widget(Paragraph::new(Line::from(spans)), area);
    }

    // ── Body ────────────────────────────────────────────────────────────────

    fn render_body(&self, f: &mut Frame, app: &mut App, area: Rect) {
        // Always drawn, like v0.1 (24%); on wide terminals bounded to 22..34
        // columns so it neither truncates names nor wastes space.
        let cat_w = if area.width >= 92 {
            (area.width * 24 / 100).clamp(22, 34)
        } else {
            area.width * 24 / 100
        };
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(cat_w), Constraint::Min(10)])
            .split(area);

        if cat_w > 0 {
            self.render_categories(f, app, cols[0]);
        }
        app.regions.categories = if cat_w > 0 { cols[0] } else { Rect::default() };

        if app.show_detail {
            let dir = if cols[1].width >= 120 {
                Direction::Horizontal
            } else {
                Direction::Vertical
            };
            let parts = Layout::default()
                .direction(dir)
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(cols[1]);
            self.render_repos(f, app, parts[0]);
            self.render_detail(f, app, parts[1]);
            app.regions.detail = Some(parts[1]);
        } else {
            self.render_repos(f, app, cols[1]);
            app.regions.detail = None;
        }
    }

    fn block<'a>(&self, title: String, focused: bool) -> Block<'a> {
        let t = &self.theme;
        Block::default()
            .title(Span::styled(
                title,
                t.fg(t.accent).add_modifier(if focused {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
            ))
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(t.border_style(focused))
    }

    fn render_categories(&self, f: &mut Frame, app: &mut App, area: Rect) {
        let focused = app.focus == Focus::Categories;
        let block = self.block(" Categories ".into(), focused);
        let inner = block.inner(area);
        let max = (inner.width as usize).saturating_sub(width(self.glyphs.pointer));

        let items: Vec<ListItem> = app
            .categories
            .iter()
            .map(|(name, count)| {
                let icon = if app.is_own_view(name) {
                    self.glyphs.own
                } else {
                    self.glyphs.category_icon(name)
                };
                let label = format!("{icon} {name} ({count})");
                ListItem::new(Line::from(truncate_width(
                    &label,
                    max,
                    self.glyphs.ellipsis,
                )))
                .style(self.theme.fg(self.theme.text))
            })
            .collect();

        let list = List::new(items)
            .block(block)
            .highlight_style(self.theme.selection(focused))
            .highlight_symbol(self.glyphs.pointer);
        app.cat_list.select(Some(app.cat_sel));
        f.render_stateful_widget(list, area, &mut app.cat_list);
    }

    fn repo_item(&self, r: &StarredRepo, max: usize, show_cat: bool) -> ListItem<'static> {
        let t = &self.theme;
        let g = &self.glyphs;
        let mut spans = vec![
            Span::styled(
                r.full_name.clone(),
                t.fg(t.text).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(" {} {}", g.star, fmt_stars(r.stargazers_count)),
                t.fg(t.stars),
            ),
        ];
        if let Some(lang) = &r.language {
            spans.push(Span::styled(format!(" [{lang}]"), t.fg(t.language)));
        }
        if r.archived {
            spans.push(Span::styled(format!(" {}", g.archived), t.fg(t.warning)));
        }
        if r.fork {
            spans.push(Span::styled(format!(" {}", g.fork), t.fg(t.dim)));
        }
        if show_cat {
            spans.push(Span::styled(
                format!(" {} {}", g.sep, r.category),
                t.fg(t.dim),
            ));
        }
        let first = Line::from(fit_spans(spans, max, g.ellipsis));

        let desc_max = max.saturating_sub(2);
        let second = match &r.description {
            Some(d) => Line::from(Span::styled(
                format!("  {}", truncate_width(d, desc_max, g.ellipsis)),
                t.fg(t.description),
            )),
            None => Line::from(Span::styled(
                truncate_width("  (no description)", max, g.ellipsis),
                t.fg(t.dim).add_modifier(Modifier::ITALIC),
            )),
        };
        ListItem::new(vec![first, second])
    }

    fn render_repos(&self, f: &mut Frame, app: &mut App, area: Rect) {
        let focused = app.focus == Focus::Repos;
        let cat = app.selected_category().to_string();
        let pos = if app.visible.is_empty() {
            0
        } else {
            app.repo_sel + 1
        };
        let mut title = format!(" {cat} {} {pos}/{} ", self.glyphs.sep, app.visible.len());
        if !app.search.trim().is_empty() {
            // Results are ranked by relevance while searching, not by the
            // sort mode shown in the header.
            title.push_str(&format!(
                "{} \"{}\" {} by relevance ",
                self.glyphs.sep,
                sanitize(&app.search),
                self.glyphs.sep
            ));
        }
        let title = truncate_width(
            &title,
            (area.width as usize).saturating_sub(4),
            self.glyphs.ellipsis,
        );
        let block = self.block(title, focused);
        let inner = block.inner(area);
        app.regions.repos = area;
        app.regions.repo_page = (inner.height as usize / REPO_ITEM_HEIGHT).max(1);

        if app.visible.is_empty() {
            let msg = if app.repos.is_empty() {
                if app.syncing {
                    "Loading stars..."
                } else {
                    "No stars cached yet. Press r to sync."
                }
            } else {
                "No repositories match the current filters."
            };
            f.render_widget(
                Paragraph::new(format!("  {msg}"))
                    .block(block)
                    .style(self.theme.fg(self.theme.dim)),
                area,
            );
            return;
        }

        // Reserve one column on the right for the scrollbar, inside the border.
        let max = (inner.width as usize)
            .saturating_sub(width(self.glyphs.pointer))
            .saturating_sub(1);
        // v0.1 showed each repo's category on every row; keep that.
        let show_cat = true;
        let items: Vec<ListItem> = app
            .visible
            .iter()
            .map(|&i| self.repo_item(&app.repos[i], max, show_cat))
            .collect();

        let list = List::new(items)
            .block(block)
            .highlight_style(self.theme.selection(focused))
            .highlight_symbol(self.glyphs.pointer);
        app.repo_list.select(Some(app.repo_sel));
        f.render_stateful_widget(list, area, &mut app.repo_list);

        if app.visible.len() > app.regions.repo_page {
            let mut sb = ScrollbarState::new(app.visible.len()).position(app.repo_sel);
            let bar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None)
                .track_symbol(Some(if self.glyphs.ascii { "|" } else { "│" }))
                .thumb_symbol(if self.glyphs.ascii { "#" } else { "┃" })
                .style(self.theme.border_style(focused));
            f.render_stateful_widget(
                bar,
                area.inner(Margin {
                    vertical: 1,
                    horizontal: 0,
                }),
                &mut sb,
            );
        }
    }

    fn render_detail(&self, f: &mut Frame, app: &mut App, area: Rect) {
        let t = &self.theme;
        let g = &self.glyphs;
        let focused = app.focus == Focus::Detail;
        let block = self.block(" Details ".into(), focused);
        let inner = block.inner(area);
        app.regions.detail_page = (inner.height as usize).max(1);
        let max = inner.width as usize;

        let Some(r) = app.selected_repo().cloned() else {
            f.render_widget(Paragraph::new("").block(block), area);
            return;
        };

        let field = |label: &str, value: String, style: Style| -> Line<'static> {
            Line::from(fit_spans(
                vec![
                    Span::styled(format!("{label:<9}"), t.fg(t.dim)),
                    Span::styled(value, style),
                ],
                max,
                g.ellipsis,
            ))
        };
        let date = |d: Option<chrono::DateTime<chrono::Utc>>| {
            d.map(|d| d.format("%Y-%m-%d").to_string())
                .unwrap_or_else(|| "-".into())
        };

        let mut lines: Vec<Line<'static>> = vec![
            Line::from(Span::styled(
                truncate_width(&r.full_name, max, g.ellipsis),
                t.fg(t.accent).add_modifier(Modifier::BOLD),
            )),
            field("url", r.html_url.clone(), t.fg(t.text)),
            field(
                "stars",
                format!("{} {}", g.star, fmt_stars(r.stargazers_count)),
                t.fg(t.stars),
            ),
            field(
                "language",
                r.language.clone().unwrap_or_else(|| "-".into()),
                t.fg(t.language),
            ),
            field("category", r.category.clone(), t.fg(t.text)),
            field("starred", date(r.starred_at), t.fg(t.text)),
            field("pushed", date(r.pushed_at), t.fg(t.text)),
        ];
        let mut flags = Vec::new();
        if r.archived {
            flags.push(g.archived);
        }
        if r.fork {
            flags.push(g.fork);
        }
        if !flags.is_empty() {
            lines.push(field("flags", flags.join(", "), t.fg(t.warning)));
        }
        if !r.topics.is_empty() {
            lines.push(field("topics", r.topics.join(", "), t.fg(t.dim)));
        }
        if let Some(d) = &r.description {
            lines.push(Line::default());
            lines.push(Line::from(Span::styled(d.clone(), t.fg(t.description))));
        }
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(
            "README",
            t.fg(t.accent)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        )));
        match app.readmes.get(&r.id) {
            None | Some(Readme::Loading) => {
                lines.push(Line::from(Span::styled("Loading...", t.fg(t.dim))))
            }
            Some(Readme::Missing) => {
                lines.push(Line::from(Span::styled("(no README)", t.fg(t.dim))))
            }
            Some(Readme::Failed(e)) => lines.push(Line::from(Span::styled(
                format!("{} {e}", g.error),
                t.fg(t.error),
            ))),
            Some(Readme::Loaded(md)) => lines.extend(markdown_lines(md, t, g)),
        }

        let para = Paragraph::new(Text::from(lines))
            .block(block)
            .wrap(Wrap { trim: false });
        // Scroll is measured in *wrapped* rows; clamp so the last page is
        // fully visible (and `G` lands exactly at the end).
        let borders = (area.height - inner.height) as usize;
        let rows = para.line_count(inner.width).saturating_sub(borders);
        let max_scroll = rows
            .saturating_sub(inner.height as usize)
            .min(u16::MAX as usize) as u16;
        app.readme_scroll = app.readme_scroll.min(max_scroll);
        f.render_widget(para.scroll((app.readme_scroll, 0)), area);
    }

    // ── Status and hints ────────────────────────────────────────────────────

    fn render_status(&self, f: &mut Frame, app: &App, area: Rect) {
        let t = &self.theme;
        let g = &self.glyphs;
        let max = area.width as usize;

        if app.searching || !app.search.is_empty() {
            let prefix = " / ";
            let query = sanitize(&app.search);
            let q = truncate_width(&query, max.saturating_sub(width(prefix) + 1), g.ellipsis);
            let line = Line::from(vec![
                Span::styled(prefix, t.fg(t.accent).add_modifier(Modifier::BOLD)),
                Span::styled(q.clone(), t.fg(t.text)),
            ]);
            f.render_widget(Paragraph::new(line), area);
            if app.searching {
                let x = area.x + (width(prefix) + width(&q)) as u16;
                f.set_cursor_position(Position::new(x.min(area.right().saturating_sub(1)), area.y));
            }
            return;
        }

        let line = if let Some(s) = &app.status {
            let style = match s.level {
                Level::Info => t.fg(t.text),
                Level::Notice => t.fg(t.accent).add_modifier(Modifier::BOLD),
                Level::Warn => t.fg(t.warning),
                Level::Error => t.fg(t.error).add_modifier(Modifier::BOLD),
            };
            Line::from(Span::styled(
                truncate_width(&format!(" {}", s.text), max, g.ellipsis),
                style,
            ))
        } else if let Some(p) = &app.progress {
            Line::from(Span::styled(
                truncate_width(&format!(" {p}"), max, g.ellipsis),
                t.fg(t.accent),
            ))
        } else if let Some(err) = &app.sync_error {
            Line::from(Span::styled(
                truncate_width(&format!(" {} Sync error: {err}", g.error), max, g.ellipsis),
                t.fg(t.error),
            ))
        } else {
            let synced = app
                .last_synced
                .map(|d| format!("last synced {}", d.format("%Y-%m-%d %H:%M UTC")))
                .unwrap_or_else(|| "never synced".into());
            Line::from(Span::styled(
                truncate_width(
                    &format!(" {} stars  {}  {synced}", app.total(), g.sep),
                    max,
                    g.ellipsis,
                ),
                t.fg(t.dim),
            ))
        };
        f.render_widget(Paragraph::new(line), area);
    }

    fn render_hints(&self, f: &mut Frame, app: &App, area: Rect) {
        let t = &self.theme;
        let hints: &[(&str, &str)] = if app.searching {
            &[
                ("Enter", "open"),
                ("Esc", "clear"),
                ("Up/Down", "move"),
                ("Ctrl-U", "erase"),
            ]
        } else {
            match app.focus {
                Focus::Categories => &[
                    ("j/k", "category"),
                    ("Tab", "focus"),
                    ("/", "search"),
                    ("l", "language"),
                    ("s", "sort"),
                    ("?", "help"),
                    ("q", "quit"),
                ],
                Focus::Repos => &[
                    ("j/k", "move"),
                    ("Enter", "open"),
                    ("d", "details"),
                    ("/", "search"),
                    ("y", "copy"),
                    ("s", "sort"),
                    ("l", "language"),
                    ("r", "refresh"),
                    ("?", "help"),
                    ("q", "quit"),
                ],
                Focus::Detail => &[
                    ("j/k", "scroll"),
                    ("PgUp/PgDn", "page"),
                    ("Tab", "focus"),
                    ("d", "close"),
                    ("?", "help"),
                    ("q", "quit"),
                ],
            }
        };
        let key_style = if t.mono {
            Style::default().add_modifier(Modifier::BOLD)
        } else {
            t.fg(t.accent).add_modifier(Modifier::BOLD)
        };
        let mut spans = vec![Span::raw(" ")];
        for (k, d) in hints {
            spans.push(Span::styled(*k, key_style));
            spans.push(Span::styled(format!(" {d}  "), t.fg(t.dim)));
        }
        let spans = fit_spans(spans, area.width as usize, self.glyphs.ellipsis);
        f.render_widget(Paragraph::new(Line::from(spans)), area);
    }

    // ── Overlays ────────────────────────────────────────────────────────────

    fn popup(&self, f: &mut Frame, title: &str, w: u16, h: u16) -> (Rect, Block<'static>) {
        let area = f.area();
        let w = w.min(area.width.saturating_sub(2)).max(10);
        let h = h.min(area.height.saturating_sub(2)).max(3);
        let rect = Rect::new(
            area.x + (area.width.saturating_sub(w)) / 2,
            area.y + (area.height.saturating_sub(h)) / 2,
            w,
            h,
        );
        f.render_widget(Clear, rect);
        (rect, self.block(format!(" {title} "), true))
    }

    fn render_help(&self, f: &mut Frame) {
        let t = &self.theme;
        const KEYS: &[(&str, &str)] = &[
            ("j / k, Up / Down", "move in the focused pane"),
            ("PgUp / PgDn, Ctrl-U / D", "page / half page"),
            ("g / G, Home / End", "jump to top / bottom"),
            ("Tab / Shift-Tab", "cycle focus"),
            ("h / Left, Right", "focus left / right"),
            ("Enter / o", "open repo in browser"),
            ("d", "toggle details + README pane"),
            ("/", "fuzzy search (Enter opens match, Esc clears)"),
            ("s", "cycle sort mode"),
            ("l", "filter by language"),
            ("a / f", "hide / show archived, forks"),
            ("y / Y", "copy URL / git clone command"),
            ("r", "full refresh (also drops unstarred)"),
            ("u", "unstar selected repo (asks first)"),
            ("Ctrl-L", "redraw screen"),
            ("q / Ctrl-C", "quit"),
            ("mouse", "wheel scrolls, click selects, click again opens"),
        ];
        let lines: Vec<Line> = KEYS
            .iter()
            .map(|(k, d)| {
                Line::from(vec![
                    Span::styled(
                        format!("{k:<26}"),
                        t.fg(t.accent).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(*d, t.fg(t.text)),
                ])
            })
            .chain(std::iter::once(Line::default()))
            .chain(std::iter::once(Line::from(Span::styled(
                "Press any key to close",
                t.fg(t.dim),
            ))))
            .collect();
        let (rect, block) = self.popup(f, "Keys", 76, lines.len() as u16 + 2);
        f.render_widget(Paragraph::new(lines).block(block), rect);
    }

    fn render_languages(&self, f: &mut Frame, items: &[(Option<String>, usize)], selected: usize) {
        let t = &self.theme;
        let (rect, block) = self.popup(f, "Language", 36, (items.len() as u16 + 2).min(22));
        let inner_w = (rect.width as usize)
            .saturating_sub(2)
            .saturating_sub(width(self.glyphs.pointer));
        let list_items: Vec<ListItem> = items
            .iter()
            .map(|(l, n)| {
                let name = l.as_deref().unwrap_or("All languages");
                ListItem::new(truncate_width(
                    &format!("{name} ({n})"),
                    inner_w,
                    self.glyphs.ellipsis,
                ))
                .style(t.fg(t.text))
            })
            .collect();
        let mut state = ListState::default().with_selected(Some(selected));
        let list = List::new(list_items)
            .block(block)
            .highlight_style(t.selection(true))
            .highlight_symbol(self.glyphs.pointer);
        f.render_stateful_widget(list, rect, &mut state);
    }

    fn render_confirm(&self, f: &mut Frame, full_name: &str) {
        let t = &self.theme;
        let (rect, block) = self.popup(f, "Unstar", 52, 5);
        let max = (rect.width as usize).saturating_sub(2);
        let lines = vec![
            Line::from(Span::styled(
                truncate_width(
                    &format!("Remove your star from {full_name}?"),
                    max,
                    self.glyphs.ellipsis,
                ),
                t.fg(t.text),
            )),
            Line::default(),
            Line::from(vec![
                Span::styled("y", t.fg(t.accent).add_modifier(Modifier::BOLD)),
                Span::styled(" unstar   ", t.fg(t.dim)),
                Span::styled("n", t.fg(t.accent).add_modifier(Modifier::BOLD)),
                Span::styled(" cancel", t.fg(t.dim)),
            ]),
        ];
        f.render_widget(Paragraph::new(lines).block(block), rect);
    }
}

// ── Light Markdown ──────────────────────────────────────────────────────────

/// Remove inline Markdown: images, link targets, emphasis and code markers.
fn strip_inline(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        // ![alt](url) -> dropped
        if c == '!'
            && chars.get(i + 1) == Some(&'[')
            && let Some(end) = link_end(&chars, i + 1)
        {
            i = end;
            continue;
        }
        // [text](url) -> text
        if c == '['
            && let Some(close) = chars[i + 1..]
                .iter()
                .position(|&c| c == ']')
                .map(|p| p + i + 1)
            && chars.get(close + 1) == Some(&'(')
            && let Some(end) = chars[close + 1..]
                .iter()
                .position(|&c| c == ')')
                .map(|p| p + close + 2)
        {
            out.extend(&chars[i + 1..close]);
            i = end;
            continue;
        }
        if matches!(c, '*' | '`') || (c == '_' && chars.get(i + 1) == Some(&'_')) {
            i += if c == '_' { 2 } else { 1 };
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

/// For `[...](...)` starting at `open` (the `[`), the index after `)`.
fn link_end(chars: &[char], open: usize) -> Option<usize> {
    let close = chars[open..].iter().position(|&c| c == ']')? + open;
    if chars.get(close + 1) != Some(&'(') {
        return None;
    }
    Some(chars[close + 1..].iter().position(|&c| c == ')')? + close + 2)
}

/// Render README Markdown as styled, sanitized lines (no HTML, no images).
pub fn markdown_lines(md: &str, t: &Theme, g: &Glyphs) -> Vec<Line<'static>> {
    const MAX_LINES: usize = 2000;
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut in_code = false;
    let mut in_html_comment = false;
    let mut last_blank = true;

    for raw in md.lines() {
        if out.len() >= MAX_LINES {
            out.push(Line::from(Span::styled("[truncated]", t.fg(t.dim))));
            break;
        }
        let trimmed = raw.trim();
        if in_html_comment {
            if trimmed.contains("-->") {
                in_html_comment = false;
            }
            continue;
        }
        if trimmed.starts_with("<!--") {
            in_html_comment = !trimmed.contains("-->");
            continue;
        }
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_code = !in_code;
            continue;
        }
        let indent = raw.len() - raw.trim_start().len();
        if in_code {
            let code = format!("  {}{}", " ".repeat(indent.min(16)), sanitize(raw));
            out.push(Line::from(Span::styled(code, t.fg(t.language))));
            last_blank = false;
            continue;
        }
        if trimmed.is_empty()
            || trimmed.starts_with('<')
            || trimmed
                .chars()
                .all(|c| matches!(c, '-' | '=' | '*' | '_' | '|' | ':' | ' '))
        {
            // Blank line, raw HTML (badges, centered blocks) or rule/table border.
            if !last_blank && !out.is_empty() {
                out.push(Line::default());
                last_blank = true;
            }
            continue;
        }

        let text = sanitize(&strip_inline(trimmed));
        if text.is_empty() {
            continue;
        }
        let line = if let Some(level) = trimmed
            .find(|c| c != '#')
            .filter(|&n| n > 0 && n <= 6 && trimmed[n..].starts_with(' '))
        {
            let heading = sanitize(&strip_inline(&trimmed[level..]));
            let mut style = t.fg(t.accent).add_modifier(Modifier::BOLD);
            if level == 1 {
                style = style.add_modifier(Modifier::UNDERLINED);
            }
            Line::from(Span::styled(heading, style))
        } else if let Some(rest) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
            .or_else(|| trimmed.strip_prefix("+ "))
        {
            let bullet = if g.ascii { "-" } else { "•" };
            let pad = " ".repeat(indent.min(8));
            Line::from(vec![
                Span::styled(format!("{pad}{bullet} "), t.fg(t.accent)),
                Span::styled(sanitize(&strip_inline(rest)), t.fg(t.text)),
            ])
        } else if let Some(rest) = trimmed.strip_prefix("> ") {
            Line::from(Span::styled(
                format!("| {}", sanitize(&strip_inline(rest))),
                t.fg(t.dim).add_modifier(Modifier::ITALIC),
            ))
        } else {
            Line::from(Span::styled(text, t.fg(t.text)))
        };
        out.push(line);
        last_blank = false;
    }
    while out.last().is_some_and(|l| l.spans.is_empty()) {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ThemeName;

    #[test]
    fn star_formatting() {
        assert_eq!(fmt_stars(999), "999");
        assert_eq!(fmt_stars(12_345), "12.3k");
        assert_eq!(fmt_stars(1_234_567), "1.2M");
    }

    #[test]
    fn fit_spans_truncates_last_span() {
        let spans = vec![Span::raw("hello "), Span::raw("world")];
        let fitted = fit_spans(spans, 9, "…");
        let s: String = fitted.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(s, "hello wo…");
        assert_eq!(width(&s), 9);
    }

    #[test]
    fn inline_markdown_is_stripped() {
        assert_eq!(
            strip_inline("See **[docs](https://x.y)** and `code` ![img](a.png) here"),
            "See docs and code  here"
        );
    }

    #[test]
    fn markdown_rendering_skips_html_and_handles_blocks() {
        let md = "<p align=\"center\"><img src=x></p>\n# Title\n\nSome *text*.\n\n- item one\n```\nlet x = 1;\n```\n<!-- hidden\ncomment -->\n> quote";
        let t = Theme::preset(ThemeName::Dark);
        let lines = markdown_lines(md, &t, &Glyphs::ASCII);
        let text: Vec<String> = lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        assert_eq!(
            text,
            vec![
                "Title",
                "",
                "Some text.",
                "",
                "- item one",
                "  let x = 1;",
                "| quote"
            ]
        );
    }
}
