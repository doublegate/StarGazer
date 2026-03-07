// app.rs — Application state, event handling, and business logic.
//
// The App struct owns all mutable state. The TUI renders from it; the
// event loop mutates it. A background tokio task communicates via
// tokio::sync::mpsc channels to deliver fetch progress and results.

use crate::categories::{category_counts, CATEGORIES};
use crate::github::{StarCache, StarredRepo};
use anyhow::Result;
use fuzzy_matcher::{skim::SkimMatcherV2, FuzzyMatcher};
use std::sync::Arc;

// ── Sort mode ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortMode {
    Stars,        // ⭐ descending (default)
    RecentlyUsed, // pushed_at descending
    Name,         // alphabetical
    StarredAt,    // when we starred it, newest first
}

impl SortMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Stars => "Stars ↓",
            Self::RecentlyUsed => "Recently Active",
            Self::Name => "Name A→Z",
            Self::StarredAt => "Starred ↓",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Stars => Self::RecentlyUsed,
            Self::RecentlyUsed => Self::Name,
            Self::Name => Self::StarredAt,
            Self::StarredAt => Self::Stars,
        }
    }
}

// ── Active panel ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Panel {
    Categories,
    Repos,
}

// ── Background messages ───────────────────────────────────────────────────────

pub enum BackgroundMsg {
    Progress(String),
    Done(Box<StarCache>),
    Error(String),
}

// ── Application state ─────────────────────────────────────────────────────────

pub struct App {
    // ── Raw data ──────────────────────────────────────────────────────────────
    /// All repos, sorted by current sort mode within their category.
    pub all_repos: Vec<StarredRepo>,
    /// (category_name, count) for the sidebar
    pub category_list: Vec<(String, usize)>,

    // ── Filter / search state ─────────────────────────────────────────────────
    /// Selected category index (0 = All)
    pub category_sel: usize,
    /// Indices into `all_repos` that match current category + search
    pub visible: Vec<usize>,
    /// Repo cursor within `visible`
    pub repo_sel: usize,
    /// Current search string
    pub search: String,
    /// True when the search bar has focus / is being typed into
    pub searching: bool,

    // ── UI layout ─────────────────────────────────────────────────────────────
    pub active_panel: Panel,
    pub sort_mode: SortMode,
    /// Viewport offset for the repo list (scroll)
    pub repo_scroll: usize,
    /// Viewport offset for the category list
    pub _cat_scroll: usize,

    // ── Status / loading ──────────────────────────────────────────────────────
    pub status: String,
    pub is_loading: bool,
    pub last_updated: Option<chrono::DateTime<chrono::Utc>>,

    // ── Background channel ────────────────────────────────────────────────────
    pub bg_rx: tokio::sync::mpsc::UnboundedReceiver<BackgroundMsg>,

    // ── Identity ──────────────────────────────────────────────────────────────
    pub github_user: String,

    // ── Control ───────────────────────────────────────────────────────────────
    pub should_quit: bool,

    // ── Fuzzy matcher (cached) ────────────────────────────────────────────────
    matcher: Arc<SkimMatcherV2>,
}

impl App {
    pub fn new(
        cache: StarCache,
        bg_rx: tokio::sync::mpsc::UnboundedReceiver<BackgroundMsg>,
        github_user: String,
    ) -> Self {
        let mut app = Self {
            all_repos: Vec::new(),
            category_list: Vec::new(),
            category_sel: 0,
            visible: Vec::new(),
            repo_sel: 0,
            search: String::new(),
            searching: false,
            active_panel: Panel::Repos,
            sort_mode: SortMode::Stars,
            repo_scroll: 0,
            _cat_scroll: 0,
            status: String::new(),
            is_loading: !cache.repos.is_empty(),
            last_updated: cache.fetched_at,
            bg_rx,
            github_user,
            should_quit: false,
            matcher: Arc::new(SkimMatcherV2::default()),
        };
        app.ingest_cache(cache);
        app
    }

    // ── Data ingestion ────────────────────────────────────────────────────────

    pub fn ingest_cache(&mut self, cache: StarCache) {
        let mut repos = cache.repos;
        self.sort_repos(&mut repos);
        self.all_repos = repos;
        self.last_updated = cache.fetched_at;
        self.rebuild_categories();
        self.rebuild_visible();
    }

    fn sort_repos(&self, repos: &mut Vec<StarredRepo>) {
        match self.sort_mode {
            SortMode::Stars => {
                repos.sort_by(|a, b| b.stargazers_count.cmp(&a.stargazers_count))
            }
            SortMode::RecentlyUsed => {
                repos.sort_by(|a, b| {
                    let a_t = a.pushed_at.unwrap_or(a.updated_at);
                    let b_t = b.pushed_at.unwrap_or(b.updated_at);
                    b_t.cmp(&a_t)
                })
            }
            SortMode::Name => {
                repos.sort_by(|a, b| a.full_name.to_lowercase().cmp(&b.full_name.to_lowercase()))
            }
            SortMode::StarredAt => {
                repos.sort_by(|a, b| {
                    let a_t = a.starred_at.unwrap_or(a.updated_at);
                    let b_t = b.starred_at.unwrap_or(b.updated_at);
                    b_t.cmp(&a_t)
                })
            }
        }
    }

    fn rebuild_categories(&mut self) {
        let total = self.all_repos.len();
        let mut cats = vec![("All".to_string(), total)];
        cats.extend(category_counts(&self.all_repos));
        self.category_list = cats;
    }

    pub fn rebuild_visible(&mut self) {
        let cat_filter = self
            .category_list
            .get(self.category_sel)
            .map(|(c, _)| c.as_str())
            .unwrap_or("All");

        let search_lower = self.search.to_lowercase();
        let has_search = !search_lower.is_empty();
        let matcher = Arc::clone(&self.matcher);

        self.visible = self
            .all_repos
            .iter()
            .enumerate()
            .filter(|(_, r)| {
                // Category filter
                let cat_ok = cat_filter == "All" || r.category == cat_filter;
                if !cat_ok {
                    return false;
                }
                // Search filter
                if has_search {
                    let haystack = format!(
                        "{} {} {}",
                        r.full_name,
                        r.description.as_deref().unwrap_or(""),
                        r.topics.join(" "),
                    );
                    matcher.fuzzy_match(&haystack, &search_lower).is_some()
                } else {
                    true
                }
            })
            .map(|(i, _)| i)
            .collect();

        // Clamp selection
        if self.repo_sel >= self.visible.len() && !self.visible.is_empty() {
            self.repo_sel = self.visible.len() - 1;
        }
        self.clamp_repo_scroll();
    }

    // ── Navigation ────────────────────────────────────────────────────────────

    pub fn move_repo(&mut self, delta: i64) {
        if self.visible.is_empty() {
            return;
        }
        let len = self.visible.len() as i64;
        self.repo_sel = ((self.repo_sel as i64 + delta).rem_euclid(len)) as usize;
        self.clamp_repo_scroll();
    }

    pub fn move_category(&mut self, delta: i64) {
        if self.category_list.is_empty() {
            return;
        }
        let len = self.category_list.len() as i64;
        self.category_sel = ((self.category_sel as i64 + delta).rem_euclid(len)) as usize;
        self.repo_sel = 0;
        self.repo_scroll = 0;
        self.rebuild_visible();
    }

    fn clamp_repo_scroll(&mut self) {
        // Keep selected item in view (viewport_height is approximate; UI fixes it up)
        const VIEWPORT: usize = 20;
        if self.repo_sel < self.repo_scroll {
            self.repo_scroll = self.repo_sel;
        } else if self.repo_sel >= self.repo_scroll + VIEWPORT {
            self.repo_scroll = self.repo_sel + 1 - VIEWPORT;
        }
    }

    /// Adjust scroll after UI knows the real viewport height.
    pub fn adjust_scroll(&mut self, viewport_height: usize) {
        if self.repo_sel < self.repo_scroll {
            self.repo_scroll = self.repo_sel;
        } else if viewport_height > 0 && self.repo_sel >= self.repo_scroll + viewport_height {
            self.repo_scroll = self.repo_sel + 1 - viewport_height;
        }
    }

    // ── Sort ──────────────────────────────────────────────────────────────────

    pub fn cycle_sort(&mut self) {
        self.sort_mode = self.sort_mode.next();
        let mut repos = std::mem::take(&mut self.all_repos);
        self.sort_repos(&mut repos);
        self.all_repos = repos;
        self.rebuild_visible();
    }

    // ── Search ────────────────────────────────────────────────────────────────

    pub fn push_search_char(&mut self, c: char) {
        self.search.push(c);
        self.repo_sel = 0;
        self.repo_scroll = 0;
        self.rebuild_visible();
    }

    pub fn pop_search_char(&mut self) {
        self.search.pop();
        self.repo_sel = 0;
        self.repo_scroll = 0;
        self.rebuild_visible();
    }

    pub fn clear_search(&mut self) {
        self.search.clear();
        self.searching = false;
        self.repo_sel = 0;
        self.repo_scroll = 0;
        self.rebuild_visible();
    }

    // ── Open in browser ───────────────────────────────────────────────────────

    pub fn open_selected(&self) -> Result<()> {
        if let Some(&idx) = self.visible.get(self.repo_sel) {
            if let Some(repo) = self.all_repos.get(idx) {
                open::that(&repo.html_url)?;
            }
        }
        Ok(())
    }

    // ── Background message polling ────────────────────────────────────────────

    /// Drain pending background messages. Returns true if state changed.
    pub fn poll_background(&mut self) -> bool {
        let mut changed = false;
        while let Ok(msg) = self.bg_rx.try_recv() {
            match msg {
                BackgroundMsg::Progress(s) => {
                    self.status = s;
                    self.is_loading = true;
                    changed = true;
                }
                BackgroundMsg::Done(cache) => {
                    self.ingest_cache(*cache);
                    self.is_loading = false;
                    self.status = format!(
                        "{} stars  ·  last synced {}",
                        self.all_repos.len(),
                        self.last_updated
                            .map(|t| t.format("%Y-%m-%d %H:%M UTC").to_string())
                            .unwrap_or_else(|| "just now".to_string())
                    );
                    changed = true;
                }
                BackgroundMsg::Error(e) => {
                    self.is_loading = false;
                    self.status = format!("⚠ Sync error: {e}");
                    changed = true;
                }
            }
        }
        changed
    }

    // ── Accessors ─────────────────────────────────────────────────────────────

    /// The repo currently under the cursor, if any.
    #[allow(dead_code)]
    pub fn selected_repo(&self) -> Option<&StarredRepo> {
        self.visible
            .get(self.repo_sel)
            .and_then(|&i| self.all_repos.get(i))
    }

    pub fn total_count(&self) -> usize {
        self.all_repos.len()
    }

    pub fn visible_count(&self) -> usize {
        self.visible.len()
    }
}
