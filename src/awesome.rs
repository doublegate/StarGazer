//! Parsing of the awesome index (https://github.com/sindresorhus/awesome)
//! and of the individual awesome lists it links to.
//!
//! Both are untrusted Markdown. Instead of matching lines, the parsers walk
//! the pulldown-cmark event stream, which handles every bullet style (`-`,
//! `*`, `+`), bold names, nested lists and tables uniformly. Formats seen
//! in a survey of 20 real lists (v0.3.0 research):
//!
//! - `- [Name](https://github.com/o/r) - Description.` (awesome-lint style)
//! - `* [Name](url) [[crate](https://crates.io/..)] - Desc [![CI](..)](..)`
//! - `* **[Name](url)**` (bold names)
//! - Table rows `| [Name](url) | ... | Description |` (ToolsOfTheTrade keeps
//!   ~470 entries this way)
//! - A table of contents of `#anchor` links, and HTML blocks (sponsor
//!   banners) at the top
//!
//! Rules: an entry's name and URL come from the first link with text in a
//! list item or table row; badge images are dropped; the description is
//! the text after the dash that follows the name (which also drops crate
//! tags); only http(s) links count; entries under housekeeping headings
//! (Contents, Contributing, License, ...) are skipped. Every string passes
//! through [`crate::text::sanitize`].

use crate::cache::StarredRepo;
use crate::github::{
    FetchEvent, GitHubClient, GitHubError, Progress, RepoMeta, validate_full_name,
};
use crate::text::sanitize;
use chrono::{DateTime, Utc};
use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Hard cap on entries taken from one list.
pub const MAX_ENTRIES: usize = 10_000;
/// Hard cap on lists taken from the index (the real one has ~680).
pub const MAX_LISTS: usize = 5_000;
/// Hard cap on Markdown input; longer input is cut (at a char boundary).
pub const MAX_INPUT_BYTES: usize = 3 * 1024 * 1024;
/// Hard cap on a URL's length.
const MAX_URL: usize = 2048;
/// Per-string caps (characters), so a hostile list cannot inflate memory
/// through a few huge names, descriptions or headings: every entry clones
/// its heading path, so the heading cap bounds that copy too.
const MAX_HEADING: usize = 120;
const MAX_NAME: usize = 200;
const MAX_DESC: usize = 500;

/// Sanitize and cap to `max` characters.
fn clean(s: &str, max: usize) -> String {
    let s = sanitize(s.trim());
    match s.char_indices().nth(max) {
        Some((cut, _)) => s[..cut].to_string(),
        None => s,
    }
}

/// `md` cut to [`MAX_INPUT_BYTES`] at a char boundary.
fn bounded(md: &str) -> &str {
    if md.len() <= MAX_INPUT_BYTES {
        return md;
    }
    let mut cut = MAX_INPUT_BYTES;
    while !md.is_char_boundary(cut) {
        cut -= 1;
    }
    &md[..cut]
}

/// Headings whose entries are not part of the list.
const SKIP_HEADINGS: &[&str] = &[
    "contents",
    "table of contents",
    "contributing",
    "contribute",
    "contribution",
    "contributors",
    "license",
    "footnotes",
    "credits",
    "sponsors",
    "backers",
];

/// The index: sections of lists.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AwesomeIndex {
    pub sections: Vec<Section>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Section {
    pub name: String,
    pub items: Vec<ListRef>,
}

/// One awesome list in the index. Group entries (e.g. "Linux") have no URL
/// and only children.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ListRef {
    pub name: String,
    #[serde(default)]
    pub desc: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    /// `owner/repo` when the list lives in a GitHub repository.
    #[serde(default)]
    pub repo: Option<String>,
    #[serde(default)]
    pub children: Vec<ListRef>,
}

/// One entry of an awesome list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub name: String,
    pub url: String,
    #[serde(default)]
    pub desc: Option<String>,
    /// Headings inside the list, outermost first (the list's H1 excluded).
    #[serde(default)]
    pub path: Vec<String>,
    /// `owner/repo` when the entry is a GitHub repository.
    #[serde(default)]
    pub repo: Option<String>,
    /// Position in the README (0-based), for "List order".
    pub rank: u32,
}

/// Accept only absolute http(s) URLs of sane length made of visible ASCII.
/// Non-ASCII is rejected outright: valid URLs percent-encode it, and
/// allowing it would let invisible bidi/format characters make a URL
/// display differently from what is opened.
pub fn clean_url(url: &str) -> Option<String> {
    let url = url.trim();
    let ok = (url.starts_with("https://") || url.starts_with("http://"))
        && url.len() <= MAX_URL
        && url.bytes().all(|b| b.is_ascii_graphic());
    ok.then(|| url.to_string())
}

/// GitHub's own first-level paths, which are not user or org names.
const RESERVED: &[&str] = &[
    "about",
    "apps",
    "collections",
    "customer-stories",
    "enterprise",
    "events",
    "explore",
    "features",
    "issues",
    "login",
    "marketplace",
    "new",
    "notifications",
    "orgs",
    "pricing",
    "pulls",
    "search",
    "security",
    "settings",
    "site",
    "sponsors",
    "topics",
    "trending",
];

/// `owner/repo` when `url` points at a GitHub repository itself (not a file,
/// wiki, profile or GitHub's own pages). Fragments, queries, a trailing
/// slash and `.git` are ignored.
pub fn github_repo(url: &str) -> Option<String> {
    let rest = [
        "https://github.com/",
        "http://github.com/",
        "https://www.github.com/",
    ]
    .iter()
    .find_map(|p| url.strip_prefix(p))?;
    let rest = rest.split(['#', '?']).next()?.trim_end_matches('/');
    let mut parts = rest.split('/');
    let (owner, repo) = (parts.next()?, parts.next()?);
    if parts.next().is_some() || RESERVED.contains(&owner.to_ascii_lowercase().as_str()) {
        return None;
    }
    let repo = repo.strip_suffix(".git").unwrap_or(repo);
    let full = format!("{owner}/{repo}");
    crate::github::validate_full_name(&full).ok().map(|_| full)
}

/// A repository link in canonical form: `https://github.com/owner/repo`
/// when it is one (dropping any query, fragment or `.git`), else as given.
fn canonical(url: &str, repo: Option<&str>) -> String {
    match repo {
        Some(r) => format!("https://github.com/{r}"),
        None => url.to_string(),
    }
}

/// Split "Name - description" style text at the first dash separator.
fn split_dash(text: &str) -> (String, Option<String>) {
    for sep in [" - ", " – ", " — "] {
        if let Some((a, b)) = text.split_once(sep) {
            let b = b.trim();
            return (a.trim().to_string(), (!b.is_empty()).then(|| b.to_string()));
        }
    }
    (text.trim().to_string(), None)
}

/// Description text after an entry's name: everything after the first dash
/// separator, or the whole remainder when there is none, minus leading
/// punctuation.
fn description(after: &str) -> Option<String> {
    let after = after.trim();
    let text = ["- ", "– ", "— "]
        .iter()
        .find_map(|sep| {
            if after.starts_with(sep.trim_end()) {
                Some(after[sep.trim_end().len()..].trim())
            } else {
                after.split_once(&format!(" {sep}")).map(|(_, b)| b.trim())
            }
        })
        .unwrap_or(after);
    let text = text.trim_start_matches([':', '-', '–', '—', ' ']).trim();
    let text = clean(text, MAX_DESC);
    (!text.is_empty() && text != "-").then_some(text)
}

/// A list item or table row being collected.
#[derive(Default)]
struct Block {
    /// Text before the first named link.
    before: String,
    /// First link with visible text: (url, text).
    link: Option<(String, String)>,
    /// Text after that link.
    after: String,
    /// Currently inside the first link.
    in_first_link: bool,
    /// The first link's URL while its text is being collected.
    pending: Option<String>,
    /// Table cells (rows only): text per cell.
    cells: Vec<String>,
}

impl Block {
    fn push_text(&mut self, t: &str) {
        if self.in_first_link {
            if let Some((_, text)) = self.link.as_mut() {
                text.push_str(t);
            }
        } else if self.link.is_some() {
            self.after.push_str(t);
        } else {
            self.before.push_str(t);
        }
        if let Some(cell) = self.cells.last_mut() {
            cell.push_str(t);
        }
    }
}

/// The block that text currently belongs to: an open table row, else the
/// innermost open list item.
fn current<'a>(stack: &'a mut [Block], row: &'a mut Option<Block>) -> Option<&'a mut Block> {
    match row {
        Some(r) => Some(r),
        None => stack.last_mut(),
    }
}

/// Generic walker: calls `on_block(depth, heading_path, block)` for every
/// list item (depth = nesting level, 1 = top) and table row (depth 0).
/// Items are reported when they close, so nested items arrive before their
/// parent. Returning `false` from `on_block` stops the walk.
fn walk(md: &str, mut on_block: impl FnMut(usize, &[String], Block) -> bool) {
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_STRIKETHROUGH);

    let mut headings: Vec<(HeadingLevel, String)> = Vec::new();
    let mut heading_text: Option<(HeadingLevel, String)> = None;
    let mut stack: Vec<Block> = Vec::new(); // open list items
    let mut row: Option<Block> = None; // open table row
    let mut in_table_head = false;
    let mut image_depth = 0usize;

    for event in Parser::new_ext(md, opts) {
        match event {
            Event::Start(Tag::Heading { level, .. }) => heading_text = Some((level, String::new())),
            Event::End(TagEnd::Heading(_)) => {
                if let Some((level, text)) = heading_text.take() {
                    headings.retain(|(l, _)| *l < level);
                    headings.push((level, clean(&text, MAX_HEADING)));
                }
            }
            Event::Start(Tag::Item) => {
                // Text after a nested list belongs to the nested items.
                stack.push(Block::default());
            }
            Event::End(TagEnd::Item) => {
                if let Some(block) = stack.pop() {
                    let path = heading_path(&headings);
                    if !on_block(stack.len() + 1, &path, block) {
                        return;
                    }
                }
            }
            Event::Start(Tag::TableHead) => in_table_head = true,
            Event::End(TagEnd::TableHead) => in_table_head = false,
            Event::Start(Tag::TableRow) if !in_table_head => row = Some(Block::default()),
            Event::End(TagEnd::TableRow) => {
                if let Some(block) = row.take() {
                    let path = heading_path(&headings);
                    if !on_block(0, &path, block) {
                        return;
                    }
                }
            }
            Event::Start(Tag::TableCell) => {
                if let Some(r) = row.as_mut() {
                    r.cells.push(String::new());
                }
            }
            Event::Start(Tag::Image { .. }) => image_depth += 1,
            Event::End(TagEnd::Image) => image_depth = image_depth.saturating_sub(1),
            Event::Start(Tag::Link { dest_url, .. }) if image_depth == 0 => {
                if let Some(b) = current(&mut stack, &mut row)
                    && b.link.is_none()
                    && b.pending.is_none()
                {
                    b.pending = Some(dest_url.to_string());
                    b.link = Some((dest_url.to_string(), String::new()));
                    b.in_first_link = true;
                }
            }
            Event::End(TagEnd::Link) if image_depth == 0 => {
                if let Some(b) = current(&mut stack, &mut row)
                    && b.in_first_link
                {
                    b.in_first_link = false;
                    // A link without visible text (a badge) does not name
                    // the entry; keep looking for the next one.
                    if b.link.as_ref().is_some_and(|(_, t)| t.trim().is_empty()) {
                        b.link = None;
                        b.pending = None;
                    }
                }
            }
            Event::Text(t) | Event::Code(t) => {
                if image_depth > 0 {
                    continue;
                }
                if let Some((_, h)) = heading_text.as_mut() {
                    h.push_str(&t);
                } else if let Some(b) = current(&mut stack, &mut row) {
                    b.push_text(&t);
                }
            }
            Event::SoftBreak | Event::HardBreak => {
                if let Some(b) = current(&mut stack, &mut row) {
                    b.push_text(" ");
                }
            }
            _ => {}
        }
    }
}

/// Heading names below the document title (H1), outermost first.
fn heading_path(headings: &[(HeadingLevel, String)]) -> Vec<String> {
    headings
        .iter()
        .filter(|(l, _)| *l != HeadingLevel::H1)
        .map(|(_, t)| t.clone())
        .collect()
}

fn skipped(path: &[String]) -> bool {
    path.iter()
        .any(|h| SKIP_HEADINGS.contains(&h.trim().to_lowercase().as_str()))
}

/// Parse the awesome index README into sections of lists.
pub fn parse_index(md: &str) -> AwesomeIndex {
    let mut index = AwesomeIndex::default();
    // Items close innermost-first, so a list's children arrive before the
    // list itself; they wait here until their parent closes.
    let mut pending_children: Vec<ListRef> = Vec::new();
    let mut lists = 0usize;
    walk(bounded(md), |depth, path, block| {
        // Sections are the H2 headings; entries outside one are preamble.
        let Some(section) = path.first() else {
            return true;
        };
        if depth == 0 || skipped(path) {
            return true;
        }
        let item = match block.link {
            Some((url, text)) => {
                let url = clean_url(&url);
                let Some(url) = url else { return true };
                let repo = github_repo(&url);
                ListRef {
                    name: clean(&text, MAX_NAME),
                    desc: description(&block.after),
                    url: Some(canonical(url.trim_end_matches("#readme"), repo.as_deref())),
                    repo,
                    children: Vec::new(),
                }
            }
            None => {
                // Link-less group, e.g. "Linux" or "macOS - Operating system".
                let (name, desc) = split_dash(&block.before);
                if name.is_empty() {
                    return true;
                }
                ListRef {
                    name: clean(&name, MAX_NAME),
                    desc: desc.map(|d| clean(&d, MAX_DESC)),
                    url: None,
                    repo: None,
                    children: Vec::new(),
                }
            }
        };
        if index.sections.last().map(|s| &s.name) != Some(section) {
            index.sections.push(Section {
                name: section.clone(),
                items: Vec::new(),
            });
        }
        if depth >= 2 {
            pending_children.push(item);
        } else if let Some(section) = index.sections.last_mut() {
            let mut item = item;
            item.children = std::mem::take(&mut pending_children);
            section.items.push(item);
        }
        lists += 1;
        lists < MAX_LISTS
    });
    index.sections.retain(|s| !s.items.is_empty());
    index
}

/// Parse one awesome list README into entries, in README order.
pub fn parse_list(md: &str) -> Vec<Entry> {
    let mut entries: Vec<Entry> = Vec::new();
    walk(bounded(md), |depth, path, block| {
        if skipped(path) {
            return true;
        }
        let Some((url, text)) = block.link.clone() else {
            return true;
        };
        let Some(url) = clean_url(&url) else {
            return true; // anchors, relative links, mailto, ...
        };
        let name = clean(&text, MAX_NAME);
        if name.is_empty() {
            return true;
        }
        let desc = if depth == 0 {
            // Table row: every other non-empty cell, except the one holding
            // the name.
            let other: Vec<String> = block
                .cells
                .iter()
                .map(|c| clean(c, MAX_DESC))
                .filter(|c| !c.is_empty() && c != "-" && *c != name && !c.contains(&name))
                .collect();
            (!other.is_empty()).then(|| clean(&other.join(" · "), MAX_DESC))
        } else {
            description(&block.after)
        };
        let repo = github_repo(&url);
        entries.push(Entry {
            url: canonical(url.trim_end_matches("#readme"), repo.as_deref()),
            repo,
            name,
            desc,
            path: path.to_vec(),
            rank: entries.len() as u32,
        });
        entries.len() < MAX_ENTRIES
    });
    entries
}

// ── Storage and sync ────────────────────────────────────────────────────────
//
// Files, all written with `cache::write_atomic`:
//   <data>/awesome/index.json                 the parsed index + ETag
//   <data>/awesome/lists/<owner>__<repo>.json  one list's entries + metadata
//   <data>/awesome/state.json                 the last chosen list
//
// No lock: the index has a single writer (the startup/refresh sync) and a
// list file is only written by fetches of that same list, whose racing
// writes carry the same data and replace each other atomically.

/// The repository holding the awesome index.
pub const INDEX_REPO: &str = "sindresorhus/awesome";
/// Size cap for the index README (it is ~80 KB).
const INDEX_MAX_BYTES: usize = 1024 * 1024;
const STORE_VERSION: u32 = 1;

/// Cached, parsed index.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct IndexCache {
    pub version: u32,
    pub etag: Option<String>,
    pub fetched_at: Option<DateTime<Utc>>,
    pub index: AwesomeIndex,
}

/// One list entry plus its GitHub metadata, when known.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ListItem {
    pub entry: Entry,
    #[serde(default)]
    pub meta: Option<RepoMeta>,
}

/// Cached, parsed list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ListCache {
    pub version: u32,
    /// The list's own repository, `owner/repo`.
    pub repo: String,
    pub etag: Option<String>,
    pub fetched_at: Option<DateTime<Utc>>,
    /// Metadata was fetched successfully (false when anonymous or it failed),
    /// so the next open retries it.
    pub meta_complete: bool,
    /// Repositories (lowercase `owner/repo`) GitHub reported missing or
    /// inaccessible; not looked up again until a full refresh.
    #[serde(default)]
    pub missing: Vec<String>,
    pub items: Vec<ListItem>,
}

/// The list chosen in the picker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Selection {
    /// The list's repository, `owner/repo`.
    pub repo: String,
    /// Display label, e.g. `Node.js › Cross-Platform`.
    pub label: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
struct State {
    last: Option<Selection>,
}

/// File locations for awesome data.
#[derive(Debug, Clone)]
pub struct Store {
    dir: PathBuf,
}

impl Store {
    /// Awesome data lives next to the star cache file.
    pub fn beside(cache_path: &Path) -> Self {
        let parent = cache_path.parent().unwrap_or_else(|| Path::new("."));
        Self {
            dir: parent.join("awesome"),
        }
    }

    /// Directory holding the awesome index, lists and state.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn index_path(&self) -> PathBuf {
        self.dir.join("index.json")
    }

    fn list_path(&self, repo: &str) -> Option<PathBuf> {
        validate_full_name(repo).ok()?;
        Some(
            self.dir
                .join("lists")
                .join(format!("{}.json", repo.replace('/', "__"))),
        )
    }

    fn state_path(&self) -> PathBuf {
        self.dir.join("state.json")
    }

    fn load<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
        // A missing or unreadable file is simply "nothing cached"; it is
        // rebuilt by the next fetch.
        serde_json::from_slice(&std::fs::read(path).ok()?).ok()
    }

    fn save<T: Serialize>(path: &Path, value: &T) -> anyhow::Result<()> {
        crate::cache::write_atomic(path, &serde_json::to_vec(value)?)
    }

    pub fn load_index(&self) -> Option<IndexCache> {
        Self::load(&self.index_path())
    }

    pub fn load_list(&self, repo: &str) -> Option<ListCache> {
        Self::load::<ListCache>(&self.list_path(repo)?).filter(|c| c.repo == repo)
    }

    pub fn last_selection(&self) -> Option<Selection> {
        Self::load::<State>(&self.state_path())?.last
    }

    pub fn save_selection(&self, sel: &Selection) -> anyhow::Result<()> {
        Self::save(
            &self.state_path(),
            &State {
                last: Some(sel.clone()),
            },
        )
    }
}

/// Fetched within the last `stale_days` (0 = never stale).
pub fn is_fresh(fetched_at: Option<DateTime<Utc>>, stale_days: u32) -> bool {
    match fetched_at {
        None => false,
        Some(_) if stale_days == 0 => true,
        Some(t) => Utc::now() - t <= chrono::Duration::days(i64::from(stale_days)),
    }
}

/// Refresh the index. A conditional request when cached (unless `full`);
/// on failure the error is returned and the cache is left untouched. An
/// index that parses to no sections is treated as an error rather than
/// replacing a good cache with an empty one.
pub async fn sync_index(
    client: &GitHubClient,
    store: &Store,
    full: bool,
) -> Result<IndexCache, GitHubError> {
    let cached = store.load_index();
    let etag = if full {
        None
    } else {
        cached.as_ref().and_then(|c| c.etag.clone())
    };
    let readme = client
        .fetch_readme_text(INDEX_REPO, etag.as_deref(), INDEX_MAX_BYTES)
        .await?;
    let mut cache = match (readme.text, cached) {
        (None, Some(cached)) => cached,
        (None, None) => return Err(GitHubError::Parse("index unchanged but not cached".into())),
        (Some(text), _) => {
            let index = parse_index(&text);
            if index.sections.is_empty() {
                return Err(GitHubError::Parse("awesome index has no sections".into()));
            }
            IndexCache {
                version: STORE_VERSION,
                etag: readme.etag,
                fetched_at: None,
                index,
            }
        }
    };
    cache.fetched_at = Some(Utc::now());
    let _ = Store::save(&store.index_path(), &cache);
    Ok(cache)
}

/// Metadata already known from the star cache, keyed by lowercase
/// `owner/repo`, so awesome entries you have starred cost no lookup.
pub fn known_from_stars(repos: &[StarredRepo]) -> HashMap<String, RepoMeta> {
    repos
        .iter()
        .map(|r| {
            (
                r.full_name.to_lowercase(),
                RepoMeta {
                    name_with_owner: r.full_name.clone(),
                    database_id: r.id,
                    stargazer_count: r.stargazers_count,
                    description: r.description.clone(),
                    language: r.language.clone(),
                    topics: r.topics.clone(),
                    pushed_at: r.pushed_at,
                    updated_at: r.updated_at,
                    archived: r.archived,
                    fork: r.fork,
                    url: r.html_url.clone(),
                },
            )
        })
        .collect()
}

/// Fetch (or refresh) one list: its README (conditionally, unless `force`),
/// then GitHub metadata for its repository entries when a token is
/// available.
///
/// Lookups are the scarce resource (GitHub allows roughly 2,000 repository
/// lookups per minute of GraphQL server time), so only repositories not
/// already known are looked up: not in `known` (the star cache), and,
/// unless `force`, not in this list's previous cache, either with metadata
/// or confirmed missing. A metadata failure part-way keeps what was
/// fetched, is reported through `progress`, and leaves the list
/// incomplete so the next open fetches only the rest.
pub async fn fetch_list(
    client: &GitHubClient,
    store: &Store,
    repo: &str,
    force: bool,
    known: &HashMap<String, RepoMeta>,
    progress: &mut Progress<'_>,
) -> Result<ListCache, GitHubError> {
    validate_full_name(repo)?;
    let cached = store.load_list(repo);
    let etag = if force {
        None
    } else {
        cached.as_ref().and_then(|c| c.etag.clone())
    };
    progress(FetchEvent::Progress {
        fetched: 0,
        message: format!("Fetching {repo}"),
    });
    let readme = client
        .fetch_readme_text(repo, etag.as_deref(), MAX_INPUT_BYTES)
        .await?;
    let entries: Vec<Entry> = match (readme.text, &cached) {
        (Some(text), _) => parse_list(&text),
        (None, Some(c)) => c.items.iter().map(|i| i.entry.clone()).collect(),
        (None, None) => Vec::new(),
    };

    // What is already known, by lowercase owner/repo.
    let mut by_repo: HashMap<String, RepoMeta> = HashMap::new();
    let mut missing: HashSet<String> = HashSet::new();
    if !force && let Some(c) = &cached {
        for item in &c.items {
            if let (Some(r), Some(m)) = (&item.entry.repo, &item.meta) {
                by_repo.insert(r.to_lowercase(), m.clone());
            }
        }
        missing.extend(c.missing.iter().cloned());
    }
    for (k, m) in known {
        by_repo.insert(k.clone(), m.clone());
    }

    let mut wanted: Vec<String> = entries
        .iter()
        .filter_map(|e| e.repo.clone())
        .filter(|r| {
            let k = r.to_lowercase();
            !by_repo.contains_key(&k) && !missing.contains(&k)
        })
        .collect();
    wanted.sort_by_key(|r| r.to_lowercase());
    wanted.dedup_by_key(|r| r.to_lowercase());

    let mut meta_complete = true;
    if !wanted.is_empty() {
        if client.has_token() {
            let fetch = client.fetch_repos_meta(&wanted, progress).await?;
            for (name, meta) in wanted.iter().zip(&fetch.metas) {
                match meta {
                    Some(m) => {
                        by_repo.insert(name.to_lowercase(), m.clone());
                    }
                    None => {
                        missing.insert(name.to_lowercase());
                    }
                }
            }
            if let Some(e) = fetch.error {
                meta_complete = false;
                progress(FetchEvent::Warning(format!(
                    "star counts for {} of {} repositories in {repo} not loaded ({e}); \
                     they load the next time you open the list",
                    wanted.len() - fetch.metas.len(),
                    wanted.len()
                )));
            }
        } else {
            meta_complete = false;
        }
    }

    let mut missing: Vec<String> = missing.into_iter().collect();
    missing.sort();
    let cache = ListCache {
        version: STORE_VERSION,
        repo: repo.to_string(),
        etag: readme.etag,
        fetched_at: Some(Utc::now()),
        meta_complete,
        missing,
        items: entries
            .into_iter()
            .map(|entry| ListItem {
                meta: entry
                    .repo
                    .as_ref()
                    .and_then(|r| by_repo.get(&r.to_lowercase()).cloned()),
                entry,
            })
            .collect(),
    };
    if let Some(path) = store.list_path(repo) {
        let _ = Store::save(&path, &cache);
    }
    Ok(cache)
}

/// Stable synthetic id for a row without GitHub metadata (FNV-1a of the
/// URL, top bit set so it can never equal a GitHub database id).
fn synthetic_id(url: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in url.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h | (1 << 63)
}

/// Rows for the repo pane. GitHub entries with metadata show their current
/// `owner/name` and star count; everything else is an `external` row named
/// as in the list. `category` carries the heading path inside the list.
pub fn to_rows(cache: &ListCache) -> Vec<StarredRepo> {
    let mut seen = HashSet::new();
    cache
        .items
        .iter()
        .filter_map(|item| {
            let e = &item.entry;
            let row = match &item.meta {
                Some(m) => StarredRepo {
                    id: if m.database_id != 0 {
                        m.database_id
                    } else {
                        synthetic_id(&m.url)
                    },
                    full_name: m.name_with_owner.clone(),
                    name: e.name.clone(),
                    description: e.desc.clone().or_else(|| m.description.clone()),
                    html_url: m.url.clone(),
                    stargazers_count: m.stargazer_count,
                    language: m.language.clone(),
                    topics: m.topics.clone(),
                    updated_at: m.updated_at,
                    pushed_at: m.pushed_at,
                    fork: m.fork,
                    archived: m.archived,
                    starred_at: None,
                    category: e.path.join(" › "),
                    external: false,
                    rank: e.rank,
                },
                None => StarredRepo {
                    id: synthetic_id(&e.url),
                    full_name: e.repo.clone().unwrap_or_else(|| e.name.clone()),
                    name: e.name.clone(),
                    description: e.desc.clone(),
                    html_url: e.url.clone(),
                    stargazers_count: 0,
                    language: None,
                    topics: Vec::new(),
                    updated_at: DateTime::<Utc>::UNIX_EPOCH,
                    pushed_at: None,
                    fork: false,
                    archived: false,
                    starred_at: None,
                    category: e.path.join(" › "),
                    external: true,
                    rank: e.rank,
                },
            };
            // A list may name the same repository more than once; show it
            // once, at its first position.
            seen.insert(row.id).then_some(row)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_repo_detection() {
        assert_eq!(
            github_repo("https://github.com/o/r").as_deref(),
            Some("o/r")
        );
        assert_eq!(
            github_repo("https://github.com/o/r#readme").as_deref(),
            Some("o/r")
        );
        assert_eq!(
            github_repo("https://github.com/o/r/").as_deref(),
            Some("o/r")
        );
        assert_eq!(
            github_repo("https://github.com/o/r.git").as_deref(),
            Some("o/r")
        );
        assert_eq!(
            github_repo("http://github.com/o/r?tab=x").as_deref(),
            Some("o/r")
        );
        for not_repo in [
            "https://github.com/o",
            "https://github.com/o/r/blob/main/x.md",
            "https://github.com/o/r/wiki",
            "https://github.com/topics/rust",
            "https://github.com/orgs/rust-lang",
            "https://github.com/marketplace/actions",
            "https://gitlab.com/o/r",
            "https://github.com.evil.example/o/r",
        ] {
            assert_eq!(github_repo(not_repo), None, "{not_repo}");
        }
    }

    #[test]
    fn urls_must_be_absolute_http() {
        assert!(clean_url("https://x.y/z").is_some());
        assert!(clean_url("#anchor").is_none());
        assert!(clean_url("docs/readme.md").is_none());
        assert!(clean_url("javascript:alert(1)").is_none());
        assert!(clean_url("mailto:a@b.c").is_none());
        assert!(clean_url("https://x.y/a b").is_none());
    }

    #[test]
    fn index_sections_lists_groups_and_children() {
        let md = include_str!("../tests/fixtures/awesome/index.md");
        let idx = parse_index(md);
        let names: Vec<&str> = idx.sections.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(
            names,
            ["Platforms", "Programming Languages"],
            "Contents is skipped"
        );

        let platforms = &idx.sections[0].items;
        let node = &platforms[0];
        assert_eq!(node.name, "Node.js");
        assert_eq!(node.repo.as_deref(), Some("sindresorhus/awesome-nodejs"));
        assert_eq!(
            node.url.as_deref(),
            Some("https://github.com/sindresorhus/awesome-nodejs")
        );
        assert!(
            node.desc
                .as_deref()
                .unwrap()
                .starts_with("Async non-blocking")
        );
        assert_eq!(node.children.len(), 1);
        assert_eq!(node.children[0].name, "Cross-Platform");
        assert_eq!(
            node.children[0].repo.as_deref(),
            Some("bcoe/awesome-cross-platform-nodejs")
        );

        let linux = platforms.iter().find(|l| l.name == "Linux").unwrap();
        assert_eq!(linux.url, None, "group without its own link");
        assert_eq!(linux.children.len(), 2);
        let macos = platforms.iter().find(|l| l.name == "macOS").unwrap();
        assert_eq!(
            macos.desc.as_deref(),
            Some("Operating system for Apple's Mac computers.")
        );
        assert_eq!(macos.children.len(), 1);

        let frontend = platforms
            .iter()
            .find(|l| l.name == "Frontend Development")
            .unwrap();
        assert_eq!(frontend.desc, None, "entry without a description");
        let langs = &idx.sections[1].items;
        assert_eq!(langs[0].name, "JavaScript");
        assert_eq!(langs[0].children.len(), 3);
    }

    #[test]
    fn list_entry_forms() {
        let md = include_str!("../tests/fixtures/awesome/list.md");
        let e = parse_list(md);
        let by_name = |n: &str| {
            e.iter()
                .find(|x| x.name == n)
                .unwrap_or_else(|| panic!("{n} missing"))
        };

        // Table of contents and housekeeping sections are skipped.
        assert!(e.iter().all(|x| !x.url.starts_with('#')));
        assert!(e.iter().all(|x| x.name != "Contribution guidelines"));
        assert!(
            e.iter().all(|x| x.name != "Become a sponsor"),
            "HTML block ignored"
        );

        let woxi = by_name("ad-si/Woxi");
        assert_eq!(woxi.repo.as_deref(), Some("ad-si/Woxi"));
        assert_eq!(
            woxi.desc.as_deref(),
            Some("An interpreter for the Wolfram Language powered by Rust.")
        );
        assert_eq!(woxi.path, ["Applications", "Interpreters"]);

        let jnv = by_name("ynqa/jnv");
        assert_eq!(
            jnv.desc.as_deref(),
            Some("Interactive JSON filter using jq"),
            "badge dropped"
        );

        let star = by_name("cosmos-sdk");
        assert_eq!(star.repo.as_deref(), Some("cosmos/cosmos-sdk"), "* bullets");
        let bold = by_name("Vulkan SDK");
        assert_eq!(
            bold.url, "https://vulkan.lunarg.com/",
            "bold names, non-GitHub link"
        );
        assert_eq!(bold.repo, None);

        let nested = by_name("child-tool");
        assert_eq!(nested.path, ["Libraries"]);

        let onfido = by_name("Onfido");
        assert_eq!(onfido.url, "https://onfido.com");
        assert_eq!(onfido.path, ["Services"]);
        let desc = onfido.desc.as_deref().unwrap();
        assert!(desc.contains("Individual ID verifications"), "{desc}");
        assert!(desc.contains("$2/verification"), "{desc}");

        let ranks: Vec<u32> = e.iter().map(|x| x.rank).collect();
        assert_eq!(
            ranks,
            (0..e.len() as u32).collect::<Vec<_>>(),
            "README order"
        );
    }

    #[test]
    fn github_links_are_stored_in_canonical_form() {
        let md = "## T\n\n- [a](https://github.com/o/r?x;curl|sh) - q\n- [b](https://github.com/o/s.git#readme)\n- [c](https://example.com/p?q=1)\n";
        let urls: Vec<String> = parse_list(md).into_iter().map(|e| e.url).collect();
        assert_eq!(
            urls,
            [
                "https://github.com/o/r",
                "https://github.com/o/s",
                "https://example.com/p?q=1"
            ]
        );
    }

    #[test]
    fn hostile_text_is_sanitized() {
        let md = "## Tools\n\n- [bad\u{202E}name\u{FE0F}](https://github.com/o/r) - desc\u{200D}\u{1F527} with\ttab\n";
        let e = parse_list(md);
        assert_eq!(e[0].name, "badname");
        assert!(!e[0].desc.as_deref().unwrap().contains('\t'));
    }

    #[test]
    fn urls_with_invisible_or_non_ascii_characters_are_rejected() {
        // Security review: a bidi override would make the URL display
        // differently from what is opened.
        assert!(clean_url("https://github.com/o/r\u{202E}gpj.exe").is_none());
        assert!(clean_url("https://example.com/\u{200B}x").is_none());
        assert!(clean_url("https://bücher.example/").is_none());
        assert!(
            clean_url("https://example.com/%C3%BC?q=1#frag").is_some(),
            "percent-encoded is fine"
        );
        let md = "## X\n\n- [ok](https://example.com/a)\n- [spoof](https://example.com/\u{202E}moc.live)\n";
        let e = parse_list(md);
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].name, "ok");
    }

    #[test]
    fn oversized_strings_are_capped() {
        // Security review: entry count alone did not bound memory, since
        // every entry clones its heading path.
        let huge = "h".repeat(50_000);
        let md = format!("## {huge}\n\n### {huge}\n\n- [{huge}](https://example.com/x) - {huge}\n");
        let e = parse_list(&md);
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].name.chars().count(), MAX_NAME);
        assert_eq!(e[0].desc.as_deref().unwrap().chars().count(), MAX_DESC);
        assert!(e[0].path.iter().all(|h| h.chars().count() == MAX_HEADING));

        let idx_md = format!("## S\n\n- [{huge}](https://github.com/o/r) - {huge}\n");
        let idx = parse_index(&idx_md);
        assert_eq!(idx.sections[0].items[0].name.chars().count(), MAX_NAME);
    }

    #[test]
    fn input_is_bounded_at_a_char_boundary() {
        let s = "é".repeat(MAX_INPUT_BYTES); // 2 bytes each, twice the cap
        let b = bounded(&s);
        assert!(b.len() <= MAX_INPUT_BYTES && b.len() >= MAX_INPUT_BYTES - 1);
        assert!(b.chars().all(|c| c == 'é'));
    }

    #[test]
    fn index_list_cap_is_enforced() {
        let md: String = (0..MAX_LISTS + 50)
            .map(|i| format!("- [l{i}](https://github.com/o/l{i})\n"))
            .collect();
        let idx = parse_index(&format!("## S\n\n{md}"));
        assert_eq!(idx.sections[0].items.len(), MAX_LISTS);
    }

    #[test]
    fn entry_cap_is_enforced() {
        let md: String = (0..MAX_ENTRIES + 50)
            .map(|i| format!("- [e{i}](https://example.com/{i})\n"))
            .collect();
        assert_eq!(parse_list(&format!("## X\n\n{md}")).len(), MAX_ENTRIES);
    }
}
