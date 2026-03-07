// github.rs — GitHub API client for fetching and caching starred repositories.
//
// Design:
//   - First run: paginated full import of all starred repos (GET /users/:user/starred)
//   - Subsequent runs: incremental update — fetch newest-first, stop when we hit
//     a repo already in cache (identified by id). This gives O(new) network cost
//     instead of O(total) on every launch.
//   - Cache stored as JSON at $XDG_DATA_HOME/stargazer/stars.json
//   - GitHub token resolved from `gh auth token` subprocess; falls back to
//     GITHUB_TOKEN env var. Auth is optional (60 req/hr anon, 5000/hr with token).

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::PathBuf;
use std::process::Command;

/// A single starred repository as returned by the GitHub API.
/// We store the starred_at timestamp via the Accept header that enables it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StarredRepo {
    pub id: u64,
    pub full_name: String,       // "owner/name"
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub html_url: String,
    pub stargazers_count: u64,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub topics: Vec<String>,
    pub updated_at: DateTime<Utc>,
    pub pushed_at: Option<DateTime<Utc>>,
    pub fork: bool,
    pub archived: bool,
    /// When *we* starred it — present if Accept: application/vnd.github.star+json
    #[serde(default)]
    pub starred_at: Option<DateTime<Utc>>,
    /// Computed category (not from API)
    #[serde(default)]
    pub category: String,
}

/// Persisted cache file format.
#[derive(Debug, Serialize, Deserialize, Default)]
pub struct StarCache {
    pub username: String,
    pub fetched_at: Option<DateTime<Utc>>,
    pub repos: Vec<StarredRepo>,
}

impl StarCache {
    /// Path to the JSON cache file under $XDG_DATA_HOME/stargazer/
    pub fn cache_path() -> Result<PathBuf> {
        let dirs = ProjectDirs::from("", "", "stargazer")
            .ok_or_else(|| anyhow!("Cannot determine data directory"))?;
        let data_dir = dirs.data_dir();
        std::fs::create_dir_all(data_dir)
            .with_context(|| format!("Creating data dir: {}", data_dir.display()))?;
        Ok(data_dir.join("stars.json"))
    }

    /// Load cache from disk. Returns default (empty) if file doesn't exist.
    pub fn load() -> Result<Self> {
        let path = Self::cache_path()?;
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = std::fs::read_to_string(&path)
            .with_context(|| format!("Reading cache: {}", path.display()))?;
        serde_json::from_str(&raw).with_context(|| "Parsing cache JSON")
    }

    /// Persist cache to disk.
    pub fn save(&self) -> Result<()> {
        let path = Self::cache_path()?;
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, json)
            .with_context(|| format!("Writing cache: {}", path.display()))
    }
}

/// Resolve a GitHub personal access token.
/// Priority: `gh auth token` → GITHUB_TOKEN env var → None.
pub fn resolve_token() -> Option<String> {
    // Try gh CLI first
    if let Ok(output) = Command::new("gh").args(["auth", "token"]).output() {
        if output.status.success() {
            let token = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !token.is_empty() {
                return Some(token);
            }
        }
    }
    // Fall back to environment variable
    std::env::var("GITHUB_TOKEN").ok()
}

/// GitHub API client (async, reqwest-based).
pub struct GitHubClient {
    client: reqwest::Client,
    token: Option<String>,
    username: String,
}

impl GitHubClient {
    pub fn new(username: impl Into<String>) -> Result<Self> {
        let token = resolve_token();
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::USER_AGENT,
            reqwest::header::HeaderValue::from_static("stargazer/0.1"),
        );
        // Request starred_at timestamps via the star JSON media type
        headers.insert(
            reqwest::header::ACCEPT,
            reqwest::header::HeaderValue::from_static(
                "application/vnd.github.star+json",
            ),
        );
        let client = reqwest::Client::builder()
            .default_headers(headers)
            .build()?;
        Ok(Self {
            client,
            token,
            username: username.into(),
        })
    }

    /// Build an authorized request, attaching the token if available.
    fn get(&self, url: &str) -> reqwest::RequestBuilder {
        let req = self.client.get(url);
        if let Some(ref token) = self.token {
            req.bearer_auth(token)
        } else {
            req
        }
    }

    /// Fetch one page of starred repos. Returns (repos, has_next_page).
    ///
    /// When `accept_star_json` is true, the response is wrapped in
    /// `{ "starred_at": "...", "repo": { ... } }` objects.
    async fn fetch_page(&self, page: u32) -> Result<(Vec<StarredRepo>, bool)> {
        let url = format!(
            "https://api.github.com/users/{}/starred?per_page=100&page={}&sort=created&direction=desc",
            self.username, page
        );

        let resp = self
            .get(&url)
            .send()
            .await
            .with_context(|| format!("GET {url}"))?;

        // Check for rate-limit
        if resp.status() == reqwest::StatusCode::FORBIDDEN {
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("GitHub API error (403): {body}"));
        }

        let has_next = resp
            .headers()
            .get("link")
            .and_then(|v| v.to_str().ok())
            .map(|link| link.contains(r#"rel="next""#))
            .unwrap_or(false);

        // The star+json media type wraps each item: { "starred_at": ..., "repo": {...} }
        let raw: serde_json::Value = resp.json().await?;
        let items = raw
            .as_array()
            .ok_or_else(|| anyhow!("Expected JSON array from starred API"))?;

        let mut repos = Vec::with_capacity(items.len());
        for item in items {
            // Handle both wrapped (star+json) and flat responses
            let (starred_at, repo_val) = if item.get("repo").is_some() {
                let starred_at = item["starred_at"]
                    .as_str()
                    .and_then(|s| s.parse::<DateTime<Utc>>().ok());
                (starred_at, &item["repo"])
            } else {
                (None, item)
            };

            match serde_json::from_value::<StarredRepo>(repo_val.clone()) {
                Ok(mut r) => {
                    r.starred_at = starred_at;
                    repos.push(r);
                }
                Err(e) => {
                    // Skip malformed entries rather than failing the whole fetch
                    eprintln!("Warning: skipping repo (parse error): {e}");
                }
            }
        }

        Ok((repos, has_next))
    }

    /// Full import: fetch ALL starred repos from scratch, reporting progress via callback.
    pub async fn fetch_all<F>(&self, mut progress: F) -> Result<Vec<StarredRepo>>
    where
        F: FnMut(usize, &str),
    {
        let mut all: Vec<StarredRepo> = Vec::new();
        let mut page = 1u32;

        loop {
            progress(all.len(), &format!("Fetching page {page}…"));
            let (repos, has_next) = self.fetch_page(page).await?;
            if repos.is_empty() {
                break;
            }
            all.extend(repos);
            if !has_next {
                break;
            }
            page += 1;
            // Small delay to be a good API citizen
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        }

        Ok(all)
    }

    /// Incremental update: fetch newest stars first, stop when we hit a known id.
    /// Returns newly discovered repos (prepended to existing).
    pub async fn fetch_incremental<F>(
        &self,
        known_ids: &HashSet<u64>,
        mut progress: F,
    ) -> Result<Vec<StarredRepo>>
    where
        F: FnMut(usize, &str),
    {
        let mut new_repos: Vec<StarredRepo> = Vec::new();
        let mut page = 1u32;

        'outer: loop {
            progress(new_repos.len(), &format!("Checking for new stars (page {page})…"));
            let (repos, has_next) = self.fetch_page(page).await?;
            if repos.is_empty() {
                break;
            }
            for repo in repos {
                if known_ids.contains(&repo.id) {
                    // Hit a repo we already have — all newer ones collected
                    break 'outer;
                }
                new_repos.push(repo);
            }
            if !has_next {
                break;
            }
            page += 1;
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        }

        Ok(new_repos)
    }
}

/// High-level entry point: load cache, decide full vs incremental fetch,
/// apply categorization, save. Returns updated cache.
pub async fn sync_stars<F>(username: &str, mut progress: F) -> Result<StarCache>
where
    F: FnMut(usize, &str),
{
    use crate::categories::categorize;

    let mut cache = StarCache::load()?;
    let client = GitHubClient::new(username)?;

    let repos = if cache.repos.is_empty() {
        // First run — full import
        progress(0, "First run — importing all starred repositories…");
        let mut repos = client.fetch_all(&mut progress).await?;
        for r in &mut repos {
            r.category = categorize(r);
        }
        repos
    } else {
        // Incremental — only fetch what's new
        let known_ids: HashSet<u64> = cache.repos.iter().map(|r| r.id).collect();
        let mut new_repos = client
            .fetch_incremental(&known_ids, &mut progress)
            .await?;

        if new_repos.is_empty() {
            progress(cache.repos.len(), "Already up to date.");
            // Re-categorize in case categorization logic changed
            for r in &mut cache.repos {
                if r.category.is_empty() {
                    r.category = categorize(r);
                }
            }
            cache.fetched_at = Some(Utc::now());
            cache.save()?;
            return Ok(cache);
        }

        progress(
            new_repos.len(),
            &format!("Found {} new star(s) — merging…", new_repos.len()),
        );
        for r in &mut new_repos {
            r.category = categorize(r);
        }
        // Prepend new repos (newest first)
        new_repos.extend(cache.repos.drain(..));
        new_repos
    };

    cache.username = username.to_string();
    cache.repos = repos;
    cache.fetched_at = Some(Utc::now());
    cache.save()?;
    Ok(cache)
}
