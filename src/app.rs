//! Application state and input handling.
//!
//! `App` owns all UI state and is deliberately free of I/O: input handlers
//! mutate state and queue [`Effect`]s (open a URL, start a sync, fetch a
//! README, ...) which the event loop in [`crate::tui`] executes. Results come
//! back as [`BgMsg`]s via [`App::apply`]. That split keeps every key binding
//! unit-testable without a terminal or network.

use crate::cache::{StarCache, StarredRepo};
use crate::categories::category_counts;
use crate::text::sanitize;
use chrono::{DateTime, Utc};
use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Matcher, Utf32Str};
use ratatui::layout::{Position, Rect};
use ratatui::widgets::ListState;
use serde::Deserialize;
use std::collections::HashMap;
use std::time::{Duration, Instant};

// ── Sort mode ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, clap::ValueEnum)]
pub enum SortMode {
    /// Most-starred first.
    #[default]
    #[serde(rename = "stars")]
    #[value(name = "stars")]
    Stars,
    /// Most recently pushed first.
    #[serde(rename = "recent")]
    #[value(name = "recent")]
    RecentlyActive,
    /// `owner/repo` alphabetical.
    #[serde(rename = "name")]
    #[value(name = "name")]
    Name,
    /// Most recently starred first.
    #[serde(rename = "starred")]
    #[value(name = "starred")]
    StarredAt,
}

impl SortMode {
    /// Title-bar label (the v0.1 labels; ASCII variants for `--ascii`).
    pub fn label(self, ascii: bool) -> &'static str {
        match (self, ascii) {
            (Self::Stars, false) => "Stars ↓",
            (Self::Stars, true) => "Stars v",
            (Self::RecentlyActive, _) => "Recently Active",
            (Self::Name, false) => "Name A→Z",
            (Self::Name, true) => "Name A-Z",
            (Self::StarredAt, false) => "Starred ↓",
            (Self::StarredAt, true) => "Starred v",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Stars => Self::RecentlyActive,
            Self::RecentlyActive => Self::Name,
            Self::Name => Self::StarredAt,
            Self::StarredAt => Self::Stars,
        }
    }
}

/// Sort `repos` in place by `mode`; ties broken by name for stable output.
pub fn sort_repos(repos: &mut [StarredRepo], mode: SortMode) {
    let name = |r: &StarredRepo| r.full_name.to_lowercase();
    match mode {
        SortMode::Stars => repos.sort_by(|a, b| {
            b.stargazers_count
                .cmp(&a.stargazers_count)
                .then_with(|| name(a).cmp(&name(b)))
        }),
        SortMode::RecentlyActive => repos.sort_by(|a, b| {
            let t = |r: &StarredRepo| r.pushed_at.unwrap_or(r.updated_at);
            t(b).cmp(&t(a)).then_with(|| name(a).cmp(&name(b)))
        }),
        SortMode::Name => repos.sort_by_cached_key(name),
        SortMode::StarredAt => repos.sort_by(|a, b| {
            let t = |r: &StarredRepo| r.starred_at.unwrap_or(r.updated_at);
            t(b).cmp(&t(a)).then_with(|| name(a).cmp(&name(b)))
        }),
    }
}

// ── Focus, overlays, effects, messages ───────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Categories,
    Repos,
    Detail,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Overlay {
    None,
    Help,
    Languages {
        /// `None` entry = all languages.
        items: Vec<(Option<String>, usize)>,
        selected: usize,
    },
    ConfirmUnstar {
        id: u64,
        full_name: String,
    },
}

/// Side effects requested by the UI, executed by the event loop.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    Sync { full: bool },
    Open(String),
    Copy(String),
    FetchReadme { id: u64, full_name: String },
    Unstar { id: u64, full_name: String },
    ClearScreen,
}

/// Results delivered back from background tasks.
#[derive(Debug)]
pub enum BgMsg {
    SyncProgress(String),
    SyncWarning(String),
    SyncDone(Box<StarCache>),
    SyncFailed {
        error: String,
        partial: Option<Box<StarCache>>,
    },
    Readme {
        id: u64,
        result: Result<Option<String>, String>,
    },
    Unstarred {
        id: u64,
        full_name: String,
        result: Result<(), String>,
    },
    Copied(Result<String, String>),
    Opened(Result<(), String>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Readme {
    Loading,
    Loaded(String),
    Missing,
    Failed(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone)]
pub struct Status {
    pub text: String,
    pub level: Level,
    pub at: Instant,
}

impl Status {
    fn ttl(&self) -> Duration {
        match self.level {
            Level::Info => Duration::from_secs(4),
            Level::Warn => Duration::from_secs(8),
            Level::Error => Duration::from_secs(15),
        }
    }
}

/// Screen regions recorded by the renderer, used for mouse hit-testing.
#[derive(Debug, Clone, Copy, Default)]
pub struct Regions {
    pub categories: Rect,
    pub repos: Rect,
    pub detail: Option<Rect>,
    /// Repo rows that fit in the list viewport (for paging).
    pub repo_page: usize,
    /// Detail viewport height (for paging the README).
    pub detail_page: usize,
}

/// Lines per repo list item (name line + description line).
pub const REPO_ITEM_HEIGHT: usize = 2;
const README_DEBOUNCE: Duration = Duration::from_millis(250);

// ── App ──────────────────────────────────────────────────────────────────────

pub struct App {
    pub user: String,
    /// A token is available (unstar is possible).
    pub can_write: bool,
    /// Plain-ASCII mode (affects labels built here).
    pub ascii: bool,

    pub repos: Vec<StarredRepo>,
    haystacks: Vec<String>,
    /// `(name, count)` for the sidebar; index 0 is "All".
    pub categories: Vec<(String, usize)>,
    pub cat_sel: usize,
    pub cat_list: ListState,
    /// Indices into `repos` matching all filters, in display order.
    pub visible: Vec<usize>,
    pub repo_sel: usize,
    pub repo_list: ListState,

    pub search: String,
    pub searching: bool,
    pub language: Option<String>,
    pub hide_archived: bool,
    pub hide_forks: bool,
    pub sort: SortMode,

    pub focus: Focus,
    pub overlay: Overlay,
    pub show_detail: bool,
    pub readmes: HashMap<u64, Readme>,
    pub readme_scroll: u16,
    selection_changed: Instant,

    pub syncing: bool,
    pub progress: Option<String>,
    pub status: Option<Status>,
    /// Error from the most recent sync, cleared by the next successful one.
    pub sync_error: Option<String>,
    pub last_synced: Option<DateTime<Utc>>,
    pub frame: usize,

    pub regions: Regions,
    pub effects: Vec<Effect>,
    pub should_quit: bool,
    matcher: Matcher,
}

impl App {
    pub fn new(user: impl Into<String>, cache: StarCache, sort: SortMode, can_write: bool) -> Self {
        let mut app = Self {
            user: user.into(),
            can_write,
            ascii: false,
            repos: Vec::new(),
            haystacks: Vec::new(),
            categories: Vec::new(),
            cat_sel: 0,
            cat_list: ListState::default(),
            visible: Vec::new(),
            repo_sel: 0,
            repo_list: ListState::default(),
            search: String::new(),
            searching: false,
            language: None,
            hide_archived: false,
            hide_forks: false,
            sort,
            focus: Focus::Repos,
            overlay: Overlay::None,
            show_detail: false,
            readmes: HashMap::new(),
            readme_scroll: 0,
            selection_changed: Instant::now(),
            syncing: false,
            progress: None,
            status: None,
            sync_error: None,
            last_synced: None,
            frame: 0,
            regions: Regions::default(),
            effects: Vec::new(),
            should_quit: false,
            matcher: Matcher::new(nucleo_matcher::Config::DEFAULT),
        };
        app.ingest(cache);
        app
    }

    // ── Data ────────────────────────────────────────────────────────────────

    /// Replace the repo list, preserving the selected repo and category.
    pub fn ingest(&mut self, cache: StarCache) {
        let keep_repo = self.selected_repo().map(|r| r.id);
        let keep_cat = self.selected_category().to_string();

        let mut repos: Vec<StarredRepo> = cache
            .repos
            .into_iter()
            .map(StarredRepo::sanitized)
            .collect();
        sort_repos(&mut repos, self.sort);
        self.haystacks = repos.iter().map(haystack).collect();
        self.repos = repos;
        self.last_synced = cache.fetched_at;

        self.rebuild_categories(&keep_cat);
        self.rebuild_visible();
        if let Some(id) = keep_repo {
            self.select_repo_id(id);
        }
    }

    fn passes_filters(&self, r: &StarredRepo) -> bool {
        !(self.hide_archived && r.archived
            || self.hide_forks && r.fork
            || self
                .language
                .as_deref()
                .is_some_and(|l| r.language.as_deref() != Some(l)))
    }

    /// Sidebar label of the own-repos view, e.g. "doublegate's repos".
    pub fn own_label(&self) -> String {
        format!("{}'s repos", self.user)
    }

    /// True when `name` is the own-repos view rather than a real category.
    pub fn is_own_view(&self, name: &str) -> bool {
        name == self.own_label()
    }

    /// The repo belongs to the browsed user (GitHub logins are
    /// case-insensitive).
    fn is_own(&self, r: &StarredRepo) -> bool {
        r.full_name
            .split_once('/')
            .is_some_and(|(owner, _)| owner.eq_ignore_ascii_case(&self.user))
    }

    fn rebuild_categories(&mut self, keep: &str) {
        let filtered: Vec<StarredRepo> = self
            .repos
            .iter()
            .filter(|r| self.passes_filters(r))
            .cloned()
            .collect();
        let mut cats = vec![("All".to_string(), filtered.len())];
        // "<user>'s repos": starred repos owned by the browsed user. A view,
        // not a category: those repos stay in their own category as well.
        let own = filtered.iter().filter(|r| self.is_own(r)).count();
        if own > 0 {
            cats.push((self.own_label(), own));
        }
        cats.extend(category_counts(&filtered));
        self.categories = cats;
        self.cat_sel = self
            .categories
            .iter()
            .position(|(c, _)| c == keep)
            .unwrap_or(0);
    }

    /// Recompute `visible` from category, filters and search.
    pub fn rebuild_visible(&mut self) {
        let cat = self.selected_category().to_string();
        let own_view = self.is_own_view(&cat);
        let candidates: Vec<usize> = self
            .repos
            .iter()
            .enumerate()
            .filter(|(_, r)| {
                let in_view = if own_view {
                    self.is_own(r)
                } else {
                    cat == "All" || r.category == cat
                };
                in_view && self.passes_filters(r)
            })
            .map(|(i, _)| i)
            .collect();

        if self.search.trim().is_empty() {
            self.visible = candidates;
        } else {
            // As in v0.1: case-insensitive fuzzy *filter* that keeps the current
            // sort order (the title's sort label stays truthful while searching).
            let pattern = Pattern::parse(&self.search, CaseMatching::Ignore, Normalization::Smart);
            let mut buf = Vec::new();
            let (haystacks, matcher) = (&self.haystacks, &mut self.matcher);
            self.visible = candidates
                .into_iter()
                .filter(|&i| {
                    pattern
                        .score(Utf32Str::new(&haystacks[i], &mut buf), matcher)
                        .is_some()
                })
                .collect();
        }
        self.repo_sel = self.repo_sel.min(self.visible.len().saturating_sub(1));
        self.selection_moved();
    }

    fn select_repo_id(&mut self, id: u64) {
        if let Some(pos) = self.visible.iter().position(|&i| self.repos[i].id == id) {
            self.repo_sel = pos;
        }
    }

    fn selection_moved(&mut self) {
        self.selection_changed = Instant::now();
        self.readme_scroll = 0;
    }

    // ── Accessors ───────────────────────────────────────────────────────────

    pub fn selected_repo(&self) -> Option<&StarredRepo> {
        self.visible
            .get(self.repo_sel)
            .and_then(|&i| self.repos.get(i))
    }

    pub fn selected_category(&self) -> &str {
        self.categories
            .get(self.cat_sel)
            .map(|(c, _)| c.as_str())
            .unwrap_or("All")
    }

    pub fn total(&self) -> usize {
        self.repos.len()
    }

    pub fn set_status(&mut self, level: Level, text: impl AsRef<str>) {
        self.status = Some(Status {
            text: sanitize(text.as_ref()),
            level,
            at: Instant::now(),
        });
    }

    /// Active filter summary for the title bar, e.g. `lang:Rust -archived`.
    pub fn filter_summary(&self) -> String {
        let mut parts = Vec::new();
        if let Some(l) = &self.language {
            parts.push(format!("lang:{l}"));
        }
        if self.hide_archived {
            parts.push("-archived".to_string());
        }
        if self.hide_forks {
            parts.push("-forks".to_string());
        }
        parts.join(" ")
    }

    // ── Navigation ──────────────────────────────────────────────────────────

    /// Move the repo cursor by `delta`, clamped (no wrap-around).
    pub fn move_repo(&mut self, delta: isize) {
        if self.visible.is_empty() {
            return;
        }
        let next = step(self.repo_sel, delta, self.visible.len());
        if next != self.repo_sel {
            self.repo_sel = next;
            self.selection_moved();
        }
    }

    pub fn move_category(&mut self, delta: isize) {
        if self.categories.is_empty() {
            return;
        }
        let next = step(self.cat_sel, delta, self.categories.len());
        if next != self.cat_sel {
            self.cat_sel = next;
            self.repo_sel = 0;
            self.repo_list = ListState::default();
            self.rebuild_visible();
        }
    }

    fn scroll_readme(&mut self, delta: isize) {
        self.readme_scroll =
            (self.readme_scroll as isize + delta).clamp(0, u16::MAX as isize) as u16;
    }

    fn page(&self) -> isize {
        match self.focus {
            Focus::Detail => self.regions.detail_page.max(1) as isize,
            _ => self.regions.repo_page.max(1) as isize,
        }
    }

    fn move_focused(&mut self, delta: isize) {
        match self.focus {
            Focus::Categories => self.move_category(delta),
            Focus::Repos => self.move_repo(delta),
            Focus::Detail => self.scroll_readme(delta),
        }
    }

    /// Paging keys: like v0.1, they always page the repo list unless the
    /// detail pane (new in v0.2) has focus, where they page the README.
    fn page_focused(&mut self, delta: isize) {
        match self.focus {
            Focus::Detail => self.scroll_readme(delta),
            _ => self.move_repo(delta),
        }
    }

    /// Mouse-wheel step: clamped, never wraps.
    fn scroll_repo(&mut self, delta: isize) {
        if self.visible.is_empty() {
            return;
        }
        let next = self
            .repo_sel
            .saturating_add_signed(delta)
            .min(self.visible.len() - 1);
        if next != self.repo_sel {
            self.repo_sel = next;
            self.selection_moved();
        }
    }

    /// Top/bottom keys: like v0.1, they jump in the repo list regardless of
    /// sidebar focus; in the detail pane they jump within the README.
    fn jump(&mut self, to_end: bool) {
        match self.focus {
            Focus::Categories | Focus::Repos => {
                let target = if to_end {
                    self.visible.len().saturating_sub(1)
                } else {
                    0
                };
                self.move_repo(target as isize - self.repo_sel as isize);
            }
            Focus::Detail => {
                self.readme_scroll = if to_end { u16::MAX / 2 } else { 0 };
            }
        }
    }

    fn cycle_focus(&mut self, forward: bool) {
        let order: &[Focus] = if self.show_detail {
            &[Focus::Categories, Focus::Repos, Focus::Detail]
        } else {
            &[Focus::Categories, Focus::Repos]
        };
        let i = order.iter().position(|&f| f == self.focus).unwrap_or(1);
        let n = order.len();
        self.focus = order[if forward {
            (i + 1) % n
        } else {
            (i + n - 1) % n
        }];
    }

    fn focus_left(&mut self) {
        self.focus = match self.focus {
            Focus::Detail => Focus::Repos,
            _ => Focus::Categories,
        };
    }

    fn focus_right(&mut self) {
        self.focus = match self.focus {
            Focus::Categories => Focus::Repos,
            _ if self.show_detail => Focus::Detail,
            other => other,
        };
    }

    // ── Commands ────────────────────────────────────────────────────────────

    pub fn cycle_sort(&mut self) {
        self.set_sort(self.sort.next());
    }

    pub fn set_sort(&mut self, sort: SortMode) {
        let keep = self.selected_repo().map(|r| r.id);
        self.sort = sort;
        let mut paired: Vec<(StarredRepo, String)> = std::mem::take(&mut self.repos)
            .into_iter()
            .zip(std::mem::take(&mut self.haystacks))
            .collect();
        let mut repos: Vec<StarredRepo> = paired.iter().map(|(r, _)| r.clone()).collect();
        sort_repos(&mut repos, sort);
        let mut by_id: HashMap<u64, String> = paired.drain(..).map(|(r, h)| (r.id, h)).collect();
        self.haystacks = repos
            .iter()
            .map(|r| by_id.remove(&r.id).unwrap_or_else(|| haystack(r)))
            .collect();
        self.repos = repos;
        self.rebuild_visible();
        if let Some(id) = keep {
            self.select_repo_id(id);
        }
        self.set_status(Level::Info, format!("Sort: {}", sort.label(self.ascii)));
    }

    fn refilter(&mut self) {
        let keep_cat = self.selected_category().to_string();
        let keep_repo = self.selected_repo().map(|r| r.id);
        self.rebuild_categories(&keep_cat);
        self.rebuild_visible();
        if let Some(id) = keep_repo {
            self.select_repo_id(id);
        }
    }

    fn open_selected(&mut self) {
        if let Some(r) = self.selected_repo() {
            let url = r.html_url.clone();
            self.effects.push(Effect::Open(url));
        }
    }

    fn copy_selected(&mut self, clone_cmd: bool) {
        if let Some(r) = self.selected_repo() {
            let text = if clone_cmd {
                format!("git clone {}.git", r.html_url)
            } else {
                r.html_url.clone()
            };
            self.effects.push(Effect::Copy(text));
        }
    }

    fn request_unstar(&mut self) {
        if !self.can_write {
            self.set_status(
                Level::Warn,
                "Unstar needs a GitHub token (gh auth login or GITHUB_TOKEN)",
            );
            return;
        }
        if let Some(r) = self.selected_repo() {
            self.overlay = Overlay::ConfirmUnstar {
                id: r.id,
                full_name: r.full_name.clone(),
            };
        }
    }

    fn open_language_picker(&mut self) {
        let mut counts: HashMap<&str, usize> = HashMap::new();
        for r in &self.repos {
            if let Some(l) = r.language.as_deref() {
                *counts.entry(l).or_default() += 1;
            }
        }
        let mut items: Vec<(Option<String>, usize)> = counts
            .into_iter()
            .map(|(l, n)| (Some(l.to_string()), n))
            .collect();
        items.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        items.insert(0, (None, self.repos.len()));
        let selected = items
            .iter()
            .position(|(l, _)| *l == self.language)
            .unwrap_or(0);
        self.overlay = Overlay::Languages { items, selected };
    }

    pub fn start_sync(&mut self, full: bool) {
        if self.syncing {
            self.set_status(Level::Info, "A sync is already running");
            return;
        }
        self.syncing = true;
        self.effects.push(Effect::Sync { full });
    }

    // ── Input ───────────────────────────────────────────────────────────────

    pub fn handle_key(&mut self, key: KeyEvent) {
        if key.kind == KeyEventKind::Release {
            return;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && matches!(key.code, KeyCode::Char('c')) {
            self.should_quit = true;
            return;
        }
        if ctrl && matches!(key.code, KeyCode::Char('l')) {
            self.effects.push(Effect::ClearScreen);
            return;
        }

        match &mut self.overlay {
            Overlay::None => {}
            Overlay::Help => {
                self.overlay = Overlay::None;
                return;
            }
            Overlay::Languages { items, selected } => {
                match key.code {
                    KeyCode::Up | KeyCode::Char('k') => *selected = selected.saturating_sub(1),
                    KeyCode::Down | KeyCode::Char('j') => {
                        *selected = (*selected + 1).min(items.len().saturating_sub(1))
                    }
                    KeyCode::Home | KeyCode::Char('g') => *selected = 0,
                    KeyCode::End | KeyCode::Char('G') => *selected = items.len().saturating_sub(1),
                    KeyCode::Enter => {
                        let choice = items.get(*selected).and_then(|(l, _)| l.clone());
                        self.overlay = Overlay::None;
                        self.language = choice;
                        self.refilter();
                    }
                    KeyCode::Esc | KeyCode::Char('q') => self.overlay = Overlay::None,
                    _ => {}
                }
                return;
            }
            Overlay::ConfirmUnstar { id, full_name } => {
                match key.code {
                    KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                        self.effects.push(Effect::Unstar {
                            id: *id,
                            full_name: full_name.clone(),
                        });
                        self.overlay = Overlay::None;
                    }
                    KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc | KeyCode::Char('q') => {
                        self.overlay = Overlay::None
                    }
                    _ => {}
                }
                return;
            }
        }

        if self.searching {
            self.handle_search_key(key, ctrl);
            return;
        }

        match key.code {
            KeyCode::Char('q') | KeyCode::Char('Q') => self.should_quit = true,
            KeyCode::Char('?') => self.overlay = Overlay::Help,
            KeyCode::Char('/') => {
                self.searching = true;
                self.focus = Focus::Repos;
            }
            KeyCode::Esc => {
                if !self.search.is_empty() {
                    self.search.clear();
                    self.refilter();
                } else if self.show_detail {
                    self.show_detail = false;
                    if self.focus == Focus::Detail {
                        self.focus = Focus::Repos;
                    }
                }
            }
            KeyCode::Tab => self.cycle_focus(true),
            KeyCode::BackTab => self.cycle_focus(false),
            KeyCode::Left | KeyCode::Char('h') => self.focus_left(),
            KeyCode::Right => self.focus_right(),
            KeyCode::Up | KeyCode::Char('k') => self.move_focused(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_focused(1),
            KeyCode::PageUp => self.page_focused(-self.page()),
            KeyCode::PageDown => self.page_focused(self.page()),
            KeyCode::Char('u') if ctrl => self.page_focused(-(self.page() / 2).max(2)),
            KeyCode::Char('d') if ctrl => self.page_focused((self.page() / 2).max(2)),
            KeyCode::Home | KeyCode::Char('g') => self.jump(false),
            KeyCode::End | KeyCode::Char('G') => self.jump(true),
            KeyCode::Enter | KeyCode::Char('o') => self.open_selected(),
            KeyCode::Char('d') => {
                self.show_detail = !self.show_detail;
                if self.show_detail {
                    // Retry READMEs that failed earlier (e.g. while offline).
                    self.readmes.retain(|_, r| !matches!(r, Readme::Failed(_)));
                } else if self.focus == Focus::Detail {
                    self.focus = Focus::Repos;
                }
                self.selection_moved();
            }
            KeyCode::Char('y') => self.copy_selected(false),
            KeyCode::Char('Y') => self.copy_selected(true),
            KeyCode::Char('s') | KeyCode::Char('S') => self.cycle_sort(),
            KeyCode::Char('l') => self.open_language_picker(),
            KeyCode::Char('a') => {
                self.hide_archived = !self.hide_archived;
                self.refilter();
                let state = if self.hide_archived {
                    "hidden"
                } else {
                    "shown"
                };
                self.set_status(Level::Info, format!("Archived repos {state}"));
            }
            KeyCode::Char('f') => {
                self.hide_forks = !self.hide_forks;
                self.refilter();
                let state = if self.hide_forks { "hidden" } else { "shown" };
                self.set_status(Level::Info, format!("Forks {state}"));
            }
            KeyCode::Char('r') | KeyCode::Char('R') => self.start_sync(true),
            KeyCode::Char('u') => self.request_unstar(),
            _ => {}
        }
    }

    fn handle_search_key(&mut self, key: KeyEvent, ctrl: bool) {
        match key.code {
            KeyCode::Esc => {
                self.search.clear();
                self.searching = false;
                self.refilter();
            }
            KeyCode::Enter => {
                // v0.1 behavior: leave search mode (keeping the filter) and
                // open the selected match, so "/name<Enter>" jumps straight there.
                self.searching = false;
                self.open_selected();
            }
            KeyCode::Backspace => {
                self.search.pop();
                self.repo_sel = 0;
                self.rebuild_visible();
            }
            KeyCode::Char('u') if ctrl => {
                self.search.clear();
                self.repo_sel = 0;
                self.rebuild_visible();
            }
            KeyCode::Up => self.move_repo(-1),
            KeyCode::Down => self.move_repo(1),
            KeyCode::PageUp => self.move_repo(-self.page()),
            KeyCode::PageDown => self.move_repo(self.page()),
            KeyCode::Char(c) if !ctrl && !c.is_control() && self.search.chars().count() < 100 => {
                self.search.push(c);
                self.repo_sel = 0;
                self.rebuild_visible();
            }
            _ => {}
        }
    }

    pub fn handle_mouse(&mut self, ev: MouseEvent) {
        if self.overlay != Overlay::None {
            return;
        }
        let pos = Position::new(ev.column, ev.row);
        let r = self.regions;
        let in_detail = r.detail.is_some_and(|d| d.contains(pos));
        match ev.kind {
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
                let delta = if ev.kind == MouseEventKind::ScrollDown {
                    1
                } else {
                    -1
                };
                if in_detail {
                    self.scroll_readme(delta * 3);
                } else if r.categories.contains(pos) {
                    let next = (self.cat_sel as isize + delta)
                        .clamp(0, self.categories.len().saturating_sub(1) as isize);
                    self.move_category(next - self.cat_sel as isize);
                } else if r.repos.contains(pos) {
                    self.scroll_repo(delta);
                }
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if in_detail {
                    self.focus = Focus::Detail;
                } else if let Some(row) = inner_row(r.categories, pos) {
                    self.focus = Focus::Categories;
                    let idx = self.cat_list.offset() + row;
                    if idx < self.categories.len() {
                        self.move_category(idx as isize - self.cat_sel as isize);
                    }
                } else if let Some(row) = inner_row(r.repos, pos) {
                    self.focus = Focus::Repos;
                    let idx = self.repo_list.offset() + row / REPO_ITEM_HEIGHT;
                    if idx < self.visible.len() {
                        if idx == self.repo_sel {
                            self.open_selected();
                        } else {
                            self.move_repo(idx as isize - self.repo_sel as isize);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    // ── Time and background results ─────────────────────────────────────────

    /// Periodic housekeeping: expire status, animate spinner, debounce README
    /// fetches. Returns true when a redraw is needed.
    pub fn tick(&mut self, now: Instant) -> bool {
        let mut redraw = false;
        if self.syncing {
            self.frame = self.frame.wrapping_add(1);
            redraw = true;
        }
        if self
            .status
            .as_ref()
            .is_some_and(|s| now.duration_since(s.at) > s.ttl())
        {
            self.status = None;
            redraw = true;
        }
        if self.show_detail
            && now.duration_since(self.selection_changed) >= README_DEBOUNCE
            && let Some(r) = self.selected_repo()
            && !self.readmes.contains_key(&r.id)
        {
            let (id, full_name) = (r.id, r.full_name.clone());
            self.readmes.insert(id, Readme::Loading);
            self.effects.push(Effect::FetchReadme { id, full_name });
            redraw = true;
        }
        redraw
    }

    /// Apply a background result.
    pub fn apply(&mut self, msg: BgMsg) {
        match msg {
            BgMsg::SyncProgress(p) => {
                self.syncing = true;
                self.progress = Some(sanitize(&p));
            }
            BgMsg::SyncWarning(w) => self.set_status(Level::Warn, w),
            BgMsg::SyncDone(cache) => {
                self.syncing = false;
                self.sync_error = None;
                let done = self.progress.take();
                self.ingest(*cache);
                self.set_status(Level::Info, done.unwrap_or_else(|| "Sync complete".into()));
            }
            BgMsg::SyncFailed { error, partial } => {
                self.syncing = false;
                self.progress = None;
                if let Some(p) = partial {
                    self.ingest(*p);
                }
                let error = sanitize(&error);
                self.set_status(Level::Error, format!("Sync failed: {error}"));
                // Stays on the idle status line until a sync succeeds (v0.1
                // kept the error visible; the transient status expires).
                self.sync_error = Some(error);
            }
            BgMsg::Readme { id, result } => {
                let state = match result {
                    Ok(Some(text)) => Readme::Loaded(text),
                    Ok(None) => Readme::Missing,
                    Err(e) => Readme::Failed(sanitize(&e)),
                };
                self.readmes.insert(id, state);
            }
            BgMsg::Unstarred {
                id,
                full_name,
                result,
            } => match result {
                Ok(()) => {
                    let keep_cat = self.selected_category().to_string();
                    if let Some(pos) = self.repos.iter().position(|r| r.id == id) {
                        self.repos.remove(pos);
                        self.haystacks.remove(pos);
                    }
                    self.rebuild_categories(&keep_cat);
                    self.rebuild_visible();
                    self.set_status(Level::Info, format!("Unstarred {full_name}"));
                }
                Err(e) => self.set_status(Level::Error, format!("Unstar failed: {e}")),
            },
            BgMsg::Copied(result) => match result {
                Ok(text) => self.set_status(Level::Info, format!("Copied {text}")),
                Err(e) => self.set_status(Level::Error, format!("Copy failed: {e}")),
            },
            BgMsg::Opened(result) => {
                if let Err(e) = result {
                    self.set_status(Level::Error, format!("Could not open browser: {e}"));
                }
            }
        }
    }

    /// Take queued effects.
    pub fn drain_effects(&mut self) -> Vec<Effect> {
        std::mem::take(&mut self.effects)
    }
}

/// Move a cursor in a list of `len` items. Single steps wrap around (as in
/// v0.1: `j` on the last item goes to the first); larger jumps clamp, so
/// PgDn near the bottom stops at the last item instead of wrapping.
fn step(cur: usize, delta: isize, len: usize) -> usize {
    debug_assert!(len > 0);
    if delta.unsigned_abs() == 1 {
        (cur as isize + delta).rem_euclid(len as isize) as usize
    } else {
        cur.saturating_add_signed(delta).min(len - 1)
    }
}

/// Row index inside a bordered region, or `None` outside its inner area.
fn inner_row(area: Rect, pos: Position) -> Option<usize> {
    let inner = Rect::new(
        area.x.saturating_add(1),
        area.y.saturating_add(1),
        area.width.saturating_sub(2),
        area.height.saturating_sub(2),
    );
    inner.contains(pos).then(|| (pos.y - inner.y) as usize)
}

/// Search text for a repo.
fn haystack(r: &StarredRepo) -> String {
    format!(
        "{} {} {} {}",
        r.full_name,
        r.description.as_deref().unwrap_or(""),
        r.topics.join(" "),
        r.language.as_deref().unwrap_or("")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(id: u64, name: &str, stars: u64, cat: &str, lang: Option<&str>) -> StarredRepo {
        StarredRepo {
            id,
            full_name: format!("o/{name}"),
            name: name.into(),
            html_url: format!("https://github.com/o/{name}"),
            stargazers_count: stars,
            category: cat.into(),
            language: lang.map(str::to_string),
            ..StarredRepo::test_default()
        }
    }

    fn app() -> App {
        let cache = StarCache {
            repos: vec![
                repo(1, "alpha", 10, "Rust", Some("Rust")),
                repo(2, "beta", 50, "Other", Some("Python")),
                repo(3, "gamma", 30, "Rust", Some("Rust")),
                repo(4, "delta", 5, "Other", None),
            ],
            ..Default::default()
        };
        let mut a = App::new("me", cache, SortMode::Stars, true);
        a.regions.repo_page = 2;
        a
    }

    fn key(c: KeyCode) -> KeyEvent {
        KeyEvent::new(c, KeyModifiers::NONE)
    }
    fn ch(c: char) -> KeyEvent {
        key(KeyCode::Char(c))
    }
    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }
    fn names(a: &App) -> Vec<&str> {
        a.visible
            .iter()
            .map(|&i| a.repos[i].name.as_str())
            .collect()
    }

    #[test]
    fn sorted_by_stars_initially() {
        assert_eq!(names(&app()), ["beta", "gamma", "alpha", "delta"]);
    }

    #[test]
    fn single_steps_wrap_but_paging_clamps() {
        let mut a = app();
        // v0.1 behavior: k on the first item wraps to the last, j wraps back.
        a.handle_key(key(KeyCode::Up));
        assert_eq!(a.repo_sel, 3);
        a.handle_key(ch('j'));
        assert_eq!(a.repo_sel, 0);
        // Paging clamps at the ends instead of wrapping.
        a.handle_key(key(KeyCode::PageUp));
        assert_eq!(a.repo_sel, 0);
        a.handle_key(key(KeyCode::PageDown));
        a.handle_key(key(KeyCode::PageDown));
        a.handle_key(key(KeyCode::PageDown));
        assert_eq!(a.repo_sel, 3);
        a.handle_key(ch('g'));
        assert_eq!(a.repo_sel, 0);
        a.handle_key(ch('G'));
        assert_eq!(a.repo_sel, 3);
    }

    #[test]
    fn category_selection_filters() {
        let mut a = app();
        a.handle_key(key(KeyCode::Left));
        assert_eq!(a.focus, Focus::Categories);
        a.handle_key(key(KeyCode::Down)); // "Rust"
        assert_eq!(a.selected_category(), "Rust");
        assert_eq!(names(&a), ["gamma", "alpha"]);
    }

    #[test]
    fn tab_cycles_focus_forward_and_back() {
        let mut a = app();
        assert_eq!(a.focus, Focus::Repos);
        a.handle_key(key(KeyCode::Tab));
        assert_eq!(a.focus, Focus::Categories);
        a.handle_key(key(KeyCode::BackTab));
        assert_eq!(a.focus, Focus::Repos);
        a.handle_key(ch('d'));
        a.handle_key(key(KeyCode::Tab));
        assert_eq!(a.focus, Focus::Detail);
    }

    #[test]
    fn search_enter_opens_match_and_keeps_filter() {
        let mut a = app();
        a.handle_key(ch('/'));
        assert!(a.searching);
        for c in "gam".chars() {
            a.handle_key(ch(c));
        }
        assert_eq!(names(&a), ["gamma"]);
        a.handle_key(key(KeyCode::Enter));
        assert!(!a.searching);
        assert_eq!(names(&a), ["gamma"]);
        assert_eq!(
            a.drain_effects(),
            vec![Effect::Open("https://github.com/o/gamma".into())],
            "v0.1 behavior: Enter in search opens the selected match"
        );
        a.handle_key(key(KeyCode::Esc));
        assert_eq!(a.visible.len(), 4);
    }

    #[test]
    fn v01_uppercase_aliases_and_enter_from_sidebar() {
        let mut a = app();
        a.handle_key(key(KeyCode::Left));
        a.handle_key(key(KeyCode::Enter));
        assert_eq!(
            a.drain_effects(),
            vec![Effect::Open("https://github.com/o/beta".into())],
            "Enter opens the selected repo even with the sidebar focused"
        );
        a.handle_key(ch('S'));
        assert_eq!(a.sort, SortMode::RecentlyActive);
        a.handle_key(ch('R'));
        assert_eq!(a.drain_effects(), vec![Effect::Sync { full: true }]);
        a.handle_key(ch('Q'));
        assert!(a.should_quit);
    }

    #[test]
    fn q_in_search_is_text_but_ctrl_c_quits() {
        let mut a = app();
        a.handle_key(ch('/'));
        a.handle_key(ch('q'));
        assert!(!a.should_quit);
        assert_eq!(a.search, "q");
        a.handle_key(ctrl('c'));
        assert!(a.should_quit);
    }

    #[test]
    fn enter_and_o_open_selected() {
        let mut a = app();
        a.handle_key(key(KeyCode::Enter));
        a.handle_key(ch('o'));
        assert_eq!(
            a.drain_effects(),
            vec![
                Effect::Open("https://github.com/o/beta".into()),
                Effect::Open("https://github.com/o/beta".into())
            ]
        );
    }

    #[test]
    fn copy_url_and_clone_command() {
        let mut a = app();
        a.handle_key(ch('y'));
        a.handle_key(ch('Y'));
        assert_eq!(
            a.drain_effects(),
            vec![
                Effect::Copy("https://github.com/o/beta".into()),
                Effect::Copy("git clone https://github.com/o/beta.git".into())
            ]
        );
    }

    #[test]
    fn sort_cycle_preserves_selection() {
        let mut a = app();
        a.handle_key(key(KeyCode::Down)); // gamma
        a.handle_key(ch('s')); // recent
        a.handle_key(ch('s')); // name
        assert_eq!(names(&a), ["alpha", "beta", "delta", "gamma"]);
        assert_eq!(a.selected_repo().unwrap().name, "gamma");
    }

    #[test]
    fn language_picker_filters() {
        let mut a = app();
        a.handle_key(ch('l'));
        let Overlay::Languages { items, .. } = &a.overlay else {
            panic!("expected picker")
        };
        assert_eq!(items[0], (None, 4));
        assert_eq!(items[1], (Some("Rust".into()), 2));
        a.handle_key(key(KeyCode::Down));
        a.handle_key(key(KeyCode::Enter));
        assert_eq!(a.language.as_deref(), Some("Rust"));
        assert_eq!(names(&a), ["gamma", "alpha"]);
        assert_eq!(a.categories[0], ("All".into(), 2));
    }

    #[test]
    fn archived_and_fork_toggles() {
        let mut a = app();
        a.repos[0].archived = true; // beta (sorted first)
        a.handle_key(ch('a'));
        assert!(!names(&a).contains(&"beta"));
        a.handle_key(ch('a'));
        assert!(names(&a).contains(&"beta"));
    }

    #[test]
    fn unstar_requires_confirmation() {
        let mut a = app();
        a.handle_key(ch('u'));
        assert!(matches!(a.overlay, Overlay::ConfirmUnstar { id: 2, .. }));
        a.handle_key(ch('n'));
        assert_eq!(a.overlay, Overlay::None);
        assert!(a.drain_effects().is_empty());
        a.handle_key(ch('u'));
        a.handle_key(ch('y'));
        assert_eq!(
            a.drain_effects(),
            vec![Effect::Unstar {
                id: 2,
                full_name: "o/beta".into()
            }]
        );
        a.apply(BgMsg::Unstarred {
            id: 2,
            full_name: "o/beta".into(),
            result: Ok(()),
        });
        assert_eq!(a.total(), 3);
        assert!(!names(&a).contains(&"beta"));
    }

    #[test]
    fn unstar_without_token_warns() {
        let mut a = app();
        a.can_write = false;
        a.handle_key(ch('u'));
        assert_eq!(a.overlay, Overlay::None);
        assert_eq!(a.status.as_ref().unwrap().level, Level::Warn);
    }

    #[test]
    fn refresh_queues_one_full_sync() {
        let mut a = app();
        a.handle_key(ch('r'));
        a.handle_key(ch('r'));
        assert_eq!(a.drain_effects(), vec![Effect::Sync { full: true }]);
    }

    #[test]
    fn sync_done_preserves_selected_repo() {
        let mut a = app();
        a.handle_key(key(KeyCode::Down)); // gamma
        let mut repos: Vec<StarredRepo> = a.repos.clone();
        repos.push(repo(9, "zeta", 100, "Other", None));
        a.apply(BgMsg::SyncDone(Box::new(StarCache {
            repos,
            ..Default::default()
        })));
        assert_eq!(a.selected_repo().unwrap().name, "gamma");
        assert_eq!(a.total(), 5);
    }

    #[test]
    fn readme_fetch_is_debounced_and_once() {
        let mut a = app();
        a.handle_key(ch('d'));
        assert!(!a.tick(Instant::now()) || a.effects.is_empty());
        let later = Instant::now() + Duration::from_millis(300);
        a.tick(later);
        a.tick(later);
        assert_eq!(
            a.drain_effects(),
            vec![Effect::FetchReadme {
                id: 2,
                full_name: "o/beta".into()
            }]
        );
    }

    #[test]
    fn help_overlay_closes_on_any_key() {
        let mut a = app();
        a.handle_key(ch('?'));
        assert_eq!(a.overlay, Overlay::Help);
        a.handle_key(ch('x'));
        assert_eq!(a.overlay, Overlay::None);
    }

    #[test]
    fn mouse_click_selects_then_opens() {
        let mut a = app();
        a.regions.repos = Rect::new(20, 1, 60, 20);
        let click = |row| MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 30,
            row,
            modifiers: KeyModifiers::NONE,
        };
        a.handle_mouse(click(2 + 2)); // inner y=2; second item rows 4-5
        assert_eq!(a.repo_sel, 1);
        assert!(a.drain_effects().is_empty());
        a.handle_mouse(click(5));
        assert_eq!(
            a.drain_effects(),
            vec![Effect::Open("https://github.com/o/gamma".into())]
        );
    }

    #[test]
    fn search_keeps_sort_order_and_ignores_case() {
        let mut a = app();
        a.handle_key(ch('/'));
        // "A" (capital) must still match lowercase names, as in v0.1.
        a.handle_key(ch('A'));
        // Every name contains an 'a'; results must stay in star order
        // (beta 50, gamma 30, alpha 10, delta 5), not match-score order.
        assert_eq!(names(&a), ["beta", "gamma", "alpha", "delta"]);
    }

    #[test]
    fn paging_and_jumps_act_on_repo_list_from_sidebar() {
        let mut a = app();
        a.handle_key(key(KeyCode::Left));
        assert_eq!(a.focus, Focus::Categories);
        a.handle_key(ch('G'));
        assert_eq!(a.repo_sel, 3, "G jumps to the last repo (v0.1)");
        assert_eq!(a.cat_sel, 0, "category untouched");
        a.handle_key(key(KeyCode::PageUp));
        assert_eq!(a.repo_sel, 1);
        a.handle_key(key(KeyCode::Home));
        assert_eq!(a.repo_sel, 0);
    }

    #[test]
    fn mouse_wheel_clamps_instead_of_wrapping() {
        let mut a = app();
        a.regions.repos = Rect::new(20, 1, 60, 20);
        let wheel = |kind| MouseEvent {
            kind,
            column: 30,
            row: 5,
            modifiers: KeyModifiers::NONE,
        };
        a.handle_mouse(wheel(MouseEventKind::ScrollUp));
        assert_eq!(a.repo_sel, 0, "wheel up at the top stays at the top");
        for _ in 0..10 {
            a.handle_mouse(wheel(MouseEventKind::ScrollDown));
        }
        assert_eq!(
            a.repo_sel, 3,
            "wheel down at the bottom stays at the bottom"
        );
    }

    #[test]
    fn sync_error_persists_until_next_success() {
        let mut a = app();
        a.apply(BgMsg::SyncFailed {
            error: "network down".into(),
            partial: None,
        });
        a.tick(Instant::now() + Duration::from_secs(60));
        assert!(a.status.is_none(), "transient status expired");
        assert_eq!(a.sync_error.as_deref(), Some("network down"));
        a.apply(BgMsg::SyncDone(Box::new(StarCache {
            repos: a.repos.clone(),
            ..Default::default()
        })));
        assert!(a.sync_error.is_none());
    }

    #[test]
    fn failed_readme_is_retried_when_detail_reopened() {
        let mut a = app();
        a.readmes.insert(2, Readme::Failed("offline".into()));
        a.handle_key(ch('d'));
        assert!(!a.readmes.contains_key(&2));
    }

    #[test]
    fn own_repos_view_lists_owner_matches_and_keeps_categories() {
        let repos = vec![
            StarredRepo {
                id: 1,
                full_name: "Me/tool".into(),
                html_url: "https://github.com/Me/tool".into(),
                category: "Security".into(),
                ..StarredRepo::test_default()
            },
            StarredRepo {
                id: 2,
                full_name: "other/lib".into(),
                html_url: "https://github.com/other/lib".into(),
                category: "Security".into(),
                ..StarredRepo::test_default()
            },
            StarredRepo {
                id: 3,
                // Owner merely *contains* the login: not ours.
                full_name: "me-fan/thing".into(),
                html_url: "https://github.com/me-fan/thing".into(),
                category: "Other".into(),
                ..StarredRepo::test_default()
            },
        ];
        let cache = StarCache {
            repos,
            ..Default::default()
        };
        // Browsing "me": the owner "Me" matches case-insensitively.
        let mut a = App::new("me", cache, SortMode::Name, false);
        let labels: Vec<&str> = a.categories.iter().map(|(c, _)| c.as_str()).collect();
        assert_eq!(labels, ["All", "me's repos", "Security", "Other"]);
        assert_eq!(a.categories[1].1, 1);

        a.handle_key(key(KeyCode::Left));
        a.handle_key(key(KeyCode::Down));
        assert_eq!(a.visible.len(), 1);
        assert_eq!(a.repos[a.visible[0]].full_name, "Me/tool");

        // ...and it is still listed under its real category.
        a.handle_key(key(KeyCode::Down));
        assert_eq!(a.selected_category(), "Security");
        assert!(a.visible.iter().any(|&i| a.repos[i].full_name == "Me/tool"));
    }

    #[test]
    fn own_repos_view_absent_when_user_owns_none() {
        assert!(
            !app()
                .categories
                .iter()
                .any(|(c, _)| c.ends_with("'s repos"))
        );
    }

    #[test]
    fn status_expires() {
        let mut a = app();
        a.set_status(Level::Info, "hi");
        assert!(!a.tick(Instant::now()) || a.status.is_some());
        a.tick(Instant::now() + Duration::from_secs(5));
        assert!(a.status.is_none());
    }

    #[test]
    fn ingest_sanitizes_text() {
        let mut r = repo(1, "x", 1, "Other", None);
        r.description = Some("\u{2699}\u{FE0F} gear \u{1F9D1}\u{200D}\u{1F527}".into());
        let a = App::new(
            "me",
            StarCache {
                repos: vec![r],
                ..Default::default()
            },
            SortMode::Stars,
            false,
        );
        assert_eq!(
            a.repos[0].description.as_deref(),
            Some("\u{2699} gear \u{1F9D1}")
        );
    }
}
