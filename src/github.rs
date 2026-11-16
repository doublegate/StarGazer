//! GitHub API client.
//!
//! Two transports fetch the starred list, both newest-star-first:
//! - **GraphQL** (`user.starredRepositories`, ordered by `STARRED_AT`) when a
//!   token is available: one request per 100 stars, topics and star time
//!   included.
//! - **REST** (`/users/{user}/starred` with the `star+json` media type) when
//!   anonymous, since GraphQL requires authentication. Page one carries an
//!   ETag so an unchanged list costs a `304` that does not count against the
//!   rate limit.
//!
//! Every request has connect and total timeouts, sends the pinned API version
//! header, and retries transient failures (network errors and 5xx responses)
//! up to three attempts with exponential backoff. Rate limiting is not retried:
//! it is reported with its reset time instead. Errors are typed
//! ([`GitHubError`]) so the UI can say *why* a sync failed.

use crate::cache::StarredRepo;
use chrono::{DateTime, TimeZone, Utc};
use futures::StreamExt;
use reqwest::{RequestBuilder, Response, StatusCode, header};
use serde::Deserialize;
use serde_json::json;
use std::collections::HashSet;
use std::time::Duration;

pub const DEFAULT_API_URL: &str = "https://api.github.com";
const API_VERSION: &str = "2022-11-28";
const PER_PAGE: u32 = 100;
const MAX_ATTEMPTS: u32 = 3;
const MAX_README_BYTES: usize = 256 * 1024;
const POLITE_DELAY: Duration = Duration::from_millis(100);

/// Why a GitHub request failed.
#[derive(Debug, thiserror::Error)]
pub enum GitHubError {
    #[error("invalid GitHub username {0:?} (1-39 letters, digits or hyphens)")]
    InvalidUsername(String),
    #[error("invalid repository name {0:?}")]
    InvalidRepo(String),
    #[error("GitHub user {0:?} not found")]
    UserNotFound(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("GitHub rejected the token (401); run `gh auth login` or fix GITHUB_TOKEN")]
    Unauthorized,
    #[error("this action needs a GitHub token; run `gh auth login` or set GITHUB_TOKEN")]
    TokenRequired,
    #[error("GitHub rate limit exceeded{}", reset_suffix(.reset))]
    RateLimited { reset: Option<DateTime<Utc>> },
    #[error("GitHub API error {status}: {message}")]
    Api { status: u16, message: String },
    #[error("unexpected response from GitHub: {0}")]
    Parse(String),
    #[error("network error: {0}")]
    Network(#[from] reqwest::Error),
}

fn reset_suffix(reset: &Option<DateTime<Utc>>) -> String {
    match reset {
        Some(t) => format!("; resets at {} UTC", t.format("%H:%M")),
        None => String::new(),
    }
}

impl GitHubError {
    /// Worth retrying after a short pause.
    fn is_transient(&self) -> bool {
        match self {
            Self::Network(e) => e.is_timeout() || e.is_connect() || e.is_request(),
            Self::Api { status, .. } => *status >= 500,
            _ => false,
        }
    }
}

/// GitHub logins: ASCII alphanumerics or hyphens, not starting with a hyphen;
/// underscores are also accepted for Enterprise Managed Users
/// (`login_shortcode`). The length cap leaves room for the EMU suffix.
/// This exists to keep the value from injecting path segments into API URLs.
pub fn validate_username(user: &str) -> Result<(), GitHubError> {
    let ok = (1..=64).contains(&user.len())
        && !user.starts_with('-')
        && user
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if ok {
        Ok(())
    } else {
        Err(GitHubError::InvalidUsername(user.to_string()))
    }
}

/// `owner/repo` with the character sets GitHub permits.
pub fn validate_full_name(full_name: &str) -> Result<(), GitHubError> {
    let bad = || GitHubError::InvalidRepo(full_name.to_string());
    let (owner, repo) = full_name.split_once('/').ok_or_else(bad)?;
    validate_username(owner).map_err(|_| bad())?;
    let repo_ok = (1..=100).contains(&repo.len())
        && repo != "."
        && repo != ".."
        && repo
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if repo_ok { Ok(()) } else { Err(bad()) }
}

/// Resolve a token, in the v0.1 order: `gh auth token` first (bounded by a
/// timeout so a hung `gh` cannot stall startup), then `GITHUB_TOKEN`, then
/// `GH_TOKEN`. Empty values are ignored.
pub async fn resolve_token() -> Option<String> {
    let child = tokio::process::Command::new("gh")
        .args(["auth", "token"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .output();
    if let Ok(Ok(out)) = tokio::time::timeout(Duration::from_secs(3), child).await
        && out.status.success()
    {
        let t = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !t.is_empty() {
            return Some(t);
        }
    }
    ["GITHUB_TOKEN", "GH_TOKEN"].iter().find_map(|var| {
        std::env::var(var)
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    })
}

/// Progress callback events emitted while fetching.
#[derive(Debug, Clone, PartialEq)]
pub enum FetchEvent {
    Progress {
        fetched: usize,
        message: String,
    },
    Warning(String),
    /// Final summary of a successful sync.
    Finished(String),
}

/// Progress sink; `Send` so syncs can run on spawned tasks. The lifetime lets
/// callers pass closures that borrow local state (a bare alias would imply
/// `'static`).
pub type Progress<'a> = dyn FnMut(FetchEvent) + Send + 'a;

/// Outcome of one starred-list fetch.
#[derive(Debug, Default)]
pub struct FetchResult {
    /// Newly seen repos, newest star first.
    pub repos: Vec<StarredRepo>,
    /// Page-one ETag from the REST transport, for the next conditional request.
    pub etag: Option<String>,
    /// The REST transport got `304 Not Modified`: nothing changed.
    pub not_modified: bool,
}

/// GitHub API client.
#[derive(Debug, Clone)]
pub struct GitHubClient {
    http: reqwest::Client,
    api_url: String,
    token: Option<String>,
}

impl GitHubClient {
    /// Build a client against `api_url` (normally [`DEFAULT_API_URL`]).
    pub fn new(api_url: impl Into<String>, token: Option<String>) -> Result<Self, GitHubError> {
        let mut headers = header::HeaderMap::new();
        headers.insert(
            "X-GitHub-Api-Version",
            header::HeaderValue::from_static(API_VERSION),
        );
        let http = reqwest::Client::builder()
            .user_agent(concat!("stargazer/", env!("CARGO_PKG_VERSION")))
            .default_headers(headers)
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()?;
        Ok(Self {
            http,
            api_url: api_url.into().trim_end_matches('/').to_string(),
            token: token.filter(|t| !t.trim().is_empty()),
        })
    }

    pub fn has_token(&self) -> bool {
        self.token.is_some()
    }

    fn auth(&self, req: RequestBuilder) -> RequestBuilder {
        match &self.token {
            Some(t) => req.bearer_auth(t),
            None => req,
        }
    }

    /// Send with retries for transient failures. `build` is called per attempt.
    async fn send(&self, build: impl Fn() -> RequestBuilder) -> Result<Response, GitHubError> {
        let mut attempt = 0;
        loop {
            attempt += 1;
            let result = match self.auth(build()).send().await {
                Ok(resp) => check_status(resp).await,
                Err(e) => Err(GitHubError::Network(e)),
            };
            match result {
                Ok(resp) => return Ok(resp),
                Err(e) if attempt < MAX_ATTEMPTS && e.is_transient() => {
                    tokio::time::sleep(Duration::from_millis(400 * 2u64.pow(attempt - 1))).await;
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// Login of the token's owner.
    pub async fn viewer_login(&self) -> Result<String, GitHubError> {
        if !self.has_token() {
            return Err(GitHubError::TokenRequired);
        }
        #[derive(Deserialize)]
        struct User {
            login: String,
        }
        let url = format!("{}/user", self.api_url);
        let resp = self.send(|| self.http.get(&url)).await?;
        let user: User = resp
            .json()
            .await
            .map_err(|e| GitHubError::Parse(e.to_string()))?;
        Ok(user.login)
    }

    /// Fetch stars newest-first, stopping at the first id in `known` (an empty
    /// set fetches everything). Repos fetched before an error are left in
    /// `acc` so a caller can keep partial progress.
    pub async fn fetch_starred(
        &self,
        user: &str,
        known: &HashSet<u64>,
        etag: Option<&str>,
        acc: &mut Vec<StarredRepo>,
        progress: &mut Progress<'_>,
    ) -> Result<FetchResult, GitHubError> {
        validate_username(user)?;
        if self.has_token() {
            self.fetch_graphql(user, known, acc, progress).await?;
            Ok(FetchResult {
                repos: std::mem::take(acc),
                ..Default::default()
            })
        } else {
            self.fetch_rest(user, known, etag, acc, progress).await
        }
    }

    async fn fetch_rest(
        &self,
        user: &str,
        known: &HashSet<u64>,
        etag: Option<&str>,
        acc: &mut Vec<StarredRepo>,
        progress: &mut Progress<'_>,
    ) -> Result<FetchResult, GitHubError> {
        let mut page = 1u32;
        let mut first_etag = None;
        loop {
            progress(FetchEvent::Progress {
                fetched: acc.len(),
                message: format!("Fetching page {page}"),
            });
            let url = format!(
                "{}/users/{user}/starred?per_page={PER_PAGE}&page={page}&sort=created&direction=desc",
                self.api_url
            );
            let conditional = if page == 1 && !known.is_empty() {
                etag
            } else {
                None
            };
            let resp = self
                .send(|| {
                    let req = self
                        .http
                        .get(&url)
                        .header(header::ACCEPT, "application/vnd.github.star+json");
                    match conditional {
                        Some(tag) => req.header(header::IF_NONE_MATCH, tag),
                        None => req,
                    }
                })
                .await
                .map_err(|e| user_not_found(e, user))?;

            if resp.status() == StatusCode::NOT_MODIFIED {
                return Ok(FetchResult {
                    repos: Vec::new(),
                    etag: etag.map(str::to_string),
                    not_modified: true,
                });
            }
            if page == 1 {
                first_etag = resp
                    .headers()
                    .get(header::ETAG)
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_string);
            }
            let has_next = has_next_page(
                resp.headers()
                    .get(header::LINK)
                    .and_then(|v| v.to_str().ok()),
            );
            let body = resp.text().await?;
            let (repos, skipped) = parse_rest_page(&body)?;
            if skipped > 0 {
                progress(FetchEvent::Warning(format!(
                    "skipped {skipped} malformed repo(s) on page {page}"
                )));
            }
            // Only a truly empty page ends the list; a page whose entries all
            // failed to parse must not truncate a full import.
            if repos.is_empty() && skipped == 0 {
                break;
            }
            let mut hit_known = false;
            for r in repos {
                if known.contains(&r.id) {
                    hit_known = true;
                    break;
                }
                acc.push(r);
            }
            if hit_known || !has_next {
                break;
            }
            page += 1;
            tokio::time::sleep(POLITE_DELAY).await;
        }
        Ok(FetchResult {
            repos: std::mem::take(acc),
            etag: first_etag,
            not_modified: false,
        })
    }

    async fn fetch_graphql(
        &self,
        user: &str,
        known: &HashSet<u64>,
        acc: &mut Vec<StarredRepo>,
        progress: &mut Progress<'_>,
    ) -> Result<(), GitHubError> {
        let url = format!("{}/graphql", self.api_url);
        let mut cursor: Option<String> = None;
        let mut page = 1u32;
        loop {
            progress(FetchEvent::Progress {
                fetched: acc.len(),
                message: format!("Fetching page {page}"),
            });
            let body = json!({
                "query": STARRED_QUERY,
                "variables": { "login": user, "after": cursor, "first": PER_PAGE },
            });
            let resp = self.send(|| self.http.post(&url).json(&body)).await?;
            let text = resp.text().await?;
            let parsed = parse_graphql_page(&text, user)?;
            if parsed.skipped > 0 {
                progress(FetchEvent::Warning(format!(
                    "skipped {} unreadable repo(s) on page {page}",
                    parsed.skipped
                )));
            }
            if let Some(first) = parsed.warnings.first() {
                progress(FetchEvent::Warning(format!(
                    "GitHub reported {} problem(s) on page {page}: {first}",
                    parsed.warnings.len()
                )));
            }
            let mut hit_known = false;
            for r in parsed.repos {
                if known.contains(&r.id) {
                    hit_known = true;
                    break;
                }
                acc.push(r);
            }
            match parsed.end_cursor {
                Some(c) if parsed.has_next && !hit_known => cursor = Some(c),
                _ => break,
            }
            page += 1;
            tokio::time::sleep(POLITE_DELAY).await;
        }
        Ok(())
    }

    /// Raw README text for `owner/repo`, truncated to a sane size. `Ok(None)`
    /// when the repo has no README.
    pub async fn fetch_readme(&self, full_name: &str) -> Result<Option<String>, GitHubError> {
        validate_full_name(full_name)?;
        let url = format!("{}/repos/{full_name}/readme", self.api_url);
        let resp = match self
            .send(|| {
                self.http
                    .get(&url)
                    .header(header::ACCEPT, "application/vnd.github.raw+json")
            })
            .await
        {
            Ok(r) => r,
            Err(GitHubError::NotFound(_)) => return Ok(None),
            Err(e) => return Err(e),
        };
        let bytes = resp.bytes().await?;
        let cut = &bytes[..bytes.len().min(MAX_README_BYTES)];
        Ok(Some(String::from_utf8_lossy(cut).into_owned()))
    }

    /// README of `owner/repo` for parsing (awesome index and lists): a
    /// conditional request when `etag` is given, truncated to `max_bytes`.
    /// A missing README is an error here, unlike [`Self::fetch_readme`].
    pub async fn fetch_readme_text(
        &self,
        full_name: &str,
        etag: Option<&str>,
        max_bytes: usize,
    ) -> Result<ReadmeText, GitHubError> {
        validate_full_name(full_name)?;
        let url = format!("{}/repos/{full_name}/readme", self.api_url);
        let resp = self
            .send(|| {
                let req = self
                    .http
                    .get(&url)
                    .header(header::ACCEPT, "application/vnd.github.raw+json");
                match etag {
                    Some(tag) => req.header(header::IF_NONE_MATCH, tag),
                    None => req,
                }
            })
            .await?;
        if resp.status() == StatusCode::NOT_MODIFIED {
            return Ok(ReadmeText {
                text: None,
                etag: etag.map(str::to_string),
            });
        }
        let new_etag = resp
            .headers()
            .get(header::ETAG)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let bytes = resp.bytes().await?;
        let cut = &bytes[..bytes.len().min(max_bytes)];
        Ok(ReadmeText {
            text: Some(String::from_utf8_lossy(cut).into_owned()),
            etag: new_etag,
        })
    }

    /// Metadata for many `owner/repo` names, in input order (`None` for a
    /// repo that no longer exists or is inaccessible). GraphQL, batches of
    /// [`META_BATCH`] aliased lookups; requires a token. Renamed repos
    /// resolve and report their current `nameWithOwner`.
    ///
    /// A failure part-way (typically GitHub's secondary limit on GraphQL
    /// server time, about 2,000 repository lookups per minute) does not
    /// discard what was fetched: [`MetaFetch::metas`] covers a prefix of
    /// the input and [`MetaFetch::error`] says why it stopped.
    pub async fn fetch_repos_meta(
        &self,
        full_names: &[String],
        progress: &mut Progress<'_>,
    ) -> Result<MetaFetch, GitHubError> {
        if !self.has_token() {
            return Err(GitHubError::TokenRequired);
        }
        let url = format!("{}/graphql", self.api_url);
        let total = full_names.len();
        // GitHub's cost is per repository, not per request (measured: 100
        // aliases ~3.3 s, 50 ~1.8 s), so throughput comes from a few batches
        // in flight at once, not from larger batches. `buffered` keeps the
        // input order.
        // Owned chunks: a closure over borrowed slices is not general enough
        // for a future that must be Send (tokio::spawn).
        let chunks: Vec<Vec<String>> = full_names
            .chunks(META_BATCH)
            .map(<[String]>::to_vec)
            .collect();
        let mut batches = futures::stream::iter(chunks)
            .map(|chunk: Vec<String>| {
                let url = &url;
                async move {
                    // Invalid names never reach GitHub; they get no metadata.
                    let valid: Vec<(usize, (&str, &str))> = chunk
                        .iter()
                        .enumerate()
                        .filter(|(_, n)| validate_full_name(n).is_ok())
                        .filter_map(|(j, n)| n.split_once('/').map(|p| (j, p)))
                        .collect();
                    let mut batch: Vec<Option<RepoMeta>> = vec![None; chunk.len()];
                    if !valid.is_empty() {
                        let (query, vars) = repos_meta_query(&valid);
                        let body = json!({ "query": query, "variables": vars });
                        let resp = self.send(|| self.http.post(url).json(&body)).await?;
                        let text = resp.text().await?;
                        for (slot, meta) in parse_repos_meta(&text, &valid)? {
                            batch[slot] = meta;
                        }
                    }
                    Ok::<_, GitHubError>(batch)
                }
            })
            .buffered(META_CONCURRENCY);
        let mut out = Vec::with_capacity(total);
        progress(FetchEvent::Progress {
            fetched: 0,
            message: format!("Fetching star counts (0/{total})"),
        });
        while let Some(batch) = batches.next().await {
            match batch {
                Ok(batch) => out.extend(batch),
                // Batches still in flight are dropped (cancelled) here.
                Err(e) => {
                    return Ok(MetaFetch {
                        metas: out,
                        error: Some(e),
                    });
                }
            }
            progress(FetchEvent::Progress {
                fetched: out.len(),
                message: format!("Fetching star counts ({}/{total})", out.len()),
            });
        }
        Ok(MetaFetch {
            metas: out,
            error: None,
        })
    }

    /// Remove `owner/repo` from the authenticated user's stars.
    pub async fn unstar(&self, full_name: &str) -> Result<(), GitHubError> {
        validate_full_name(full_name)?;
        if !self.has_token() {
            return Err(GitHubError::TokenRequired);
        }
        let url = format!("{}/user/starred/{full_name}", self.api_url);
        self.send(|| self.http.delete(&url)).await?;
        Ok(())
    }
}

const STARRED_QUERY: &str = r#"
query($login: String!, $after: String, $first: Int!) {
  user(login: $login) {
    starredRepositories(first: $first, after: $after, orderBy: {field: STARRED_AT, direction: DESC}) {
      pageInfo { hasNextPage endCursor }
      edges {
        starredAt
        node {
          databaseId nameWithOwner name description url stargazerCount
          primaryLanguage { name }
          repositoryTopics(first: 20) { nodes { topic { name } } }
          updatedAt pushedAt isFork isArchived
        }
      }
    }
  }
}"#;

/// Map a non-success status to a typed error; pass success and 304 through.
async fn check_status(resp: Response) -> Result<Response, GitHubError> {
    let status = resp.status();
    if status.is_success() || status == StatusCode::NOT_MODIFIED {
        return Ok(resp);
    }
    let headers = resp.headers().clone();
    let url = resp.url().path().to_string();
    let body = resp.text().await.unwrap_or_default();
    Err(classify_error(status, &headers, &body, &url))
}

/// Pure status-to-error mapping (separated for testing).
pub fn classify_error(
    status: StatusCode,
    headers: &header::HeaderMap,
    body: &str,
    path: &str,
) -> GitHubError {
    let header_str = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
    let remaining_zero = header_str("x-ratelimit-remaining") == Some("0");
    let reset = header_str("x-ratelimit-reset")
        .and_then(|v| v.parse::<i64>().ok())
        .and_then(|secs| Utc.timestamp_opt(secs, 0).single());
    let message = api_message(body);

    match status {
        StatusCode::UNAUTHORIZED => GitHubError::Unauthorized,
        StatusCode::NOT_FOUND => GitHubError::NotFound(path.to_string()),
        StatusCode::TOO_MANY_REQUESTS => GitHubError::RateLimited { reset },
        StatusCode::FORBIDDEN
            if remaining_zero
                || header_str("retry-after").is_some()
                || message.to_lowercase().contains("rate limit") =>
        {
            GitHubError::RateLimited { reset }
        }
        _ => GitHubError::Api {
            status: status.as_u16(),
            message,
        },
    }
}

/// The `message` field of a GitHub error body, or a trimmed body.
fn api_message(body: &str) -> String {
    let msg = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| {
            v.get("message")
                .and_then(|m| m.as_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| body.trim().to_string());
    let mut msg: String = msg.chars().take(200).collect();
    if msg.is_empty() {
        msg.push_str("no details");
    }
    msg
}

fn user_not_found(e: GitHubError, user: &str) -> GitHubError {
    match e {
        GitHubError::NotFound(_) => GitHubError::UserNotFound(user.to_string()),
        other => other,
    }
}

/// True when an RFC 8288 `Link` header advertises a `rel="next"` page.
pub fn has_next_page(link: Option<&str>) -> bool {
    link.is_some_and(|l| {
        l.split(',')
            .any(|part| part.split(';').skip(1).any(|p| p.trim() == r#"rel="next""#))
    })
}

/// Parse one REST page (either `star+json` wrapped items or plain repos).
/// Returns the repos and the number of malformed entries skipped.
pub fn parse_rest_page(body: &str) -> Result<(Vec<StarredRepo>, usize), GitHubError> {
    let raw: serde_json::Value =
        serde_json::from_str(body).map_err(|e| GitHubError::Parse(e.to_string()))?;
    let items = raw
        .as_array()
        .ok_or_else(|| GitHubError::Parse("expected a JSON array of starred repos".into()))?;

    let mut repos = Vec::with_capacity(items.len());
    let mut skipped = 0;
    for item in items {
        let (starred_at, repo_val) = match item.get("repo") {
            Some(repo) => (
                item.get("starred_at")
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse::<DateTime<Utc>>().ok()),
                repo.clone(),
            ),
            None => (None, item.clone()),
        };
        match serde_json::from_value::<StarredRepo>(repo_val) {
            Ok(mut r) => {
                r.starred_at = starred_at;
                repos.push(r);
            }
            Err(_) => skipped += 1,
        }
    }
    Ok((repos, skipped))
}

/// Result of [`GitHubClient::fetch_readme_text`].
#[derive(Debug, Clone, PartialEq)]
pub struct ReadmeText {
    /// `None` when the server answered 304 Not Modified.
    pub text: Option<String>,
    pub etag: Option<String>,
}

/// Repositories per GraphQL metadata request. A 3-alias probe cost 1 rate
/// limit point; 100 aliases stay far below GraphQL's node limit. Topics
/// are not requested: they doubled the server time per batch (measured
/// 6.9 s vs 3.3 s for 100 repos).
pub const META_BATCH: usize = 100;
/// Metadata requests in flight at once (well under GitHub's secondary
/// limit of 100 concurrent requests).
pub const META_CONCURRENCY: usize = 4;

/// Result of [`GitHubClient::fetch_repos_meta`].
#[derive(Debug)]
pub struct MetaFetch {
    /// One slot per input name, for a prefix of the input (all of it when
    /// `error` is `None`).
    pub metas: Vec<Option<RepoMeta>>,
    pub error: Option<GitHubError>,
}

/// Repository metadata used for awesome-list entries.
#[derive(Debug, Clone, PartialEq, serde::Serialize, Deserialize)]
pub struct RepoMeta {
    /// Current `owner/name` (differs from the request after a rename).
    pub name_with_owner: String,
    pub database_id: u64,
    pub stargazer_count: u64,
    pub description: Option<String>,
    pub language: Option<String>,
    pub topics: Vec<String>,
    pub pushed_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
    pub archived: bool,
    pub fork: bool,
    pub url: String,
}

/// Build an aliased multi-repository query. Owner and name travel as
/// GraphQL variables ($o0/$n0, ...), never inside the query text, so list
/// content cannot inject query syntax.
pub fn repos_meta_query(repos: &[(usize, (&str, &str))]) -> (String, serde_json::Value) {
    let mut params = Vec::new();
    let mut fields = Vec::new();
    let mut vars = serde_json::Map::new();
    for (k, (_, (owner, name))) in repos.iter().enumerate() {
        params.push(format!("$o{k}: String!, $n{k}: String!"));
        fields.push(format!(
            "r{k}: repository(owner: $o{k}, name: $n{k}) {{ ...M }}"
        ));
        vars.insert(format!("o{k}"), json!(owner));
        vars.insert(format!("n{k}"), json!(name));
    }
    let query = format!(
        "query({}) {{ {} }}\n\
         fragment M on Repository {{ nameWithOwner databaseId stargazerCount description url \
         primaryLanguage {{ name }} pushedAt updatedAt isArchived isFork }}",
        params.join(", "),
        fields.join(" ")
    );
    (query, serde_json::Value::Object(vars))
}

/// Parse a [`repos_meta_query`] response into `(input slot, metadata)`
/// pairs. Missing or inaccessible repositories (`null` plus NOT_FOUND or
/// FORBIDDEN errors) become `None`; rate limiting and responses without
/// data are errors.
pub fn parse_repos_meta(
    body: &str,
    repos: &[(usize, (&str, &str))],
) -> Result<Vec<(usize, Option<RepoMeta>)>, GitHubError> {
    #[derive(Deserialize)]
    struct Resp {
        data: Option<serde_json::Map<String, serde_json::Value>>,
        #[serde(default)]
        errors: Vec<GqlError>,
    }
    #[derive(Deserialize)]
    struct GqlError {
        #[serde(default, rename = "type")]
        kind: Option<String>,
        message: String,
        #[serde(default)]
        path: Vec<serde_json::Value>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Node {
        name_with_owner: String,
        database_id: Option<u64>,
        stargazer_count: u64,
        description: Option<String>,
        url: String,
        primary_language: Option<Named>,
        #[serde(default)]
        repository_topics: Topics,
        pushed_at: Option<DateTime<Utc>>,
        updated_at: DateTime<Utc>,
        is_archived: bool,
        is_fork: bool,
    }
    #[derive(Deserialize)]
    struct Named {
        name: String,
    }
    #[derive(Deserialize, Default)]
    struct Topics {
        nodes: Vec<TopicNode>,
    }
    #[derive(Deserialize)]
    struct TopicNode {
        topic: Named,
    }

    let resp: Resp = serde_json::from_str(body).map_err(|e| GitHubError::Parse(e.to_string()))?;
    if resp
        .errors
        .iter()
        .any(|e| e.kind.as_deref() == Some("RATE_LIMITED"))
    {
        return Err(GitHubError::RateLimited { reset: None });
    }
    let Some(data) = resp.data else {
        let message = resp
            .errors
            .first()
            .map(|e| e.message.chars().take(200).collect())
            .unwrap_or_else(|| "response without data".into());
        return Err(GitHubError::Api {
            status: 200,
            message,
        });
    };
    // Only GitHub's own verdict makes a repository "missing" (the caller
    // caches that and does not ask again until a full refresh). Any other
    // error on an alias, or a node that does not parse, fails the batch so
    // the lookup is retried instead (PR #7 review).
    let mut missing_aliases = std::collections::HashSet::new();
    for e in &resp.errors {
        let missing = matches!(e.kind.as_deref(), Some("NOT_FOUND" | "FORBIDDEN"));
        if !missing {
            let message: String = e.message.chars().take(200).collect();
            return Err(GitHubError::Api {
                status: 200,
                message,
            });
        }
        if let Some(alias) = e.path.first().and_then(|p| p.as_str()) {
            missing_aliases.insert(alias.to_string());
        }
    }
    repos
        .iter()
        .enumerate()
        .map(|(k, (slot, _))| {
            let alias = format!("r{k}");
            let node = match data.get(&alias) {
                // Missing only on GitHub's word for this very alias; a null
                // or absent alias without one is an incomplete response
                // (agy review of PR #8).
                Some(serde_json::Value::Null) if missing_aliases.contains(&alias) => {
                    return Ok((*slot, None));
                }
                None | Some(serde_json::Value::Null) => {
                    return Err(GitHubError::Parse(format!(
                        "repository {alias}: no data and no not-found error"
                    )));
                }
                Some(v) => serde_json::from_value::<Node>(v.clone())
                    .map_err(|e| GitHubError::Parse(format!("repository r{k}: {e}")))?,
            };
            let meta = Some(node).map(|n| RepoMeta {
                name_with_owner: n.name_with_owner,
                database_id: n.database_id.unwrap_or(0),
                stargazer_count: n.stargazer_count,
                description: n.description,
                language: n.primary_language.map(|l| l.name),
                topics: n
                    .repository_topics
                    .nodes
                    .into_iter()
                    .map(|t| t.topic.name)
                    .collect(),
                pushed_at: n.pushed_at,
                updated_at: n.updated_at,
                archived: n.is_archived,
                fork: n.is_fork,
                url: n.url,
            });
            Ok((*slot, meta))
        })
        .collect()
}

/// One parsed GraphQL page.
#[derive(Debug, Default)]
pub struct GraphqlPage {
    pub repos: Vec<StarredRepo>,
    pub has_next: bool,
    pub end_cursor: Option<String>,
    pub skipped: usize,
    /// Non-fatal GraphQL error messages that accompanied the data.
    pub warnings: Vec<String>,
}

/// Parse a GraphQL `starredRepositories` response.
pub fn parse_graphql_page(body: &str, user: &str) -> Result<GraphqlPage, GitHubError> {
    #[derive(Deserialize)]
    struct Resp {
        data: Option<Data>,
        #[serde(default)]
        errors: Vec<GqlError>,
    }
    #[derive(Deserialize)]
    struct GqlError {
        #[serde(default, rename = "type")]
        kind: Option<String>,
        message: String,
    }
    #[derive(Deserialize)]
    struct Data {
        user: Option<User>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct User {
        starred_repositories: Conn,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Conn {
        page_info: PageInfo,
        edges: Vec<serde_json::Value>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct PageInfo {
        has_next_page: bool,
        end_cursor: Option<String>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Edge {
        starred_at: Option<DateTime<Utc>>,
        node: Node,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Node {
        database_id: u64,
        name_with_owner: String,
        name: String,
        description: Option<String>,
        url: String,
        stargazer_count: u64,
        primary_language: Option<Named>,
        repository_topics: Topics,
        updated_at: DateTime<Utc>,
        pushed_at: Option<DateTime<Utc>>,
        is_fork: bool,
        is_archived: bool,
    }
    #[derive(Deserialize)]
    struct Named {
        name: String,
    }
    #[derive(Deserialize)]
    struct Topics {
        nodes: Vec<TopicNode>,
    }
    #[derive(Deserialize)]
    struct TopicNode {
        topic: Named,
    }

    let resp: Resp = serde_json::from_str(body).map_err(|e| GitHubError::Parse(e.to_string()))?;
    if resp
        .errors
        .iter()
        .any(|e| e.kind.as_deref() == Some("RATE_LIMITED"))
    {
        return Err(GitHubError::RateLimited { reset: None });
    }
    // GraphQL returns partial data alongside errors: e.g. stars on repos in
    // SAML-protected orgs come back as null nodes plus FORBIDDEN errors. Only
    // a missing `user` is fatal; anything else is skipped with a warning (the
    // REST transport in v0.1 simply listed the repos it could see).
    let Some(user_data) = resp.data.and_then(|d| d.user) else {
        return Err(match resp.errors.first() {
            None => GitHubError::UserNotFound(user.to_string()),
            Some(e) if e.kind.as_deref() == Some("NOT_FOUND") => {
                GitHubError::UserNotFound(user.to_string())
            }
            Some(e) => GitHubError::Api {
                status: 200,
                message: e.message.chars().take(200).collect(),
            },
        });
    };
    let conn = user_data.starred_repositories;

    let mut page = GraphqlPage {
        has_next: conn.page_info.has_next_page,
        end_cursor: conn.page_info.end_cursor,
        warnings: resp
            .errors
            .iter()
            .map(|e| e.message.chars().take(200).collect())
            .collect(),
        ..Default::default()
    };
    for edge in conn.edges {
        match serde_json::from_value::<Edge>(edge) {
            Ok(e) => {
                let n = e.node;
                page.repos.push(StarredRepo {
                    id: n.database_id,
                    full_name: n.name_with_owner,
                    name: n.name,
                    description: n.description,
                    html_url: n.url,
                    stargazers_count: n.stargazer_count,
                    language: n.primary_language.map(|l| l.name),
                    topics: n
                        .repository_topics
                        .nodes
                        .into_iter()
                        .map(|t| t.topic.name)
                        .collect(),
                    updated_at: n.updated_at,
                    pushed_at: n.pushed_at,
                    fork: n.is_fork,
                    archived: n.is_archived,
                    starred_at: e.starred_at,
                    category: String::new(),
                    external: false,
                    rank: 0,
                });
            }
            Err(_) => page.skipped += 1,
        }
    }
    Ok(page)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usernames() {
        for ok in [
            "octocat",
            "a",
            "DoubleGate",
            "foo-bar",
            "emu_user_acme",
            &"x".repeat(64),
        ] {
            assert!(validate_username(ok).is_ok(), "{ok}");
        }
        for bad in [
            "",
            "-lead",
            "has space",
            "a/b",
            "../x",
            "x?y",
            "a%2Fb",
            &"x".repeat(65),
            "ünï",
        ] {
            assert!(validate_username(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn full_names() {
        assert!(validate_full_name("rust-lang/rust").is_ok());
        assert!(validate_full_name("o/my.repo_name-2").is_ok());
        for bad in ["norepo", "o/..", "o/a/b", "o/a?b", "/r", "o/"] {
            assert!(validate_full_name(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn link_header_next() {
        let l = r#"<https://api.github.com/x?page=2>; rel="next", <https://api.github.com/x?page=9>; rel="last""#;
        assert!(has_next_page(Some(l)));
        let l = r#"<https://api.github.com/x?page=1>; rel="prev", <https://api.github.com/x?page=1>; rel="first""#;
        assert!(!has_next_page(Some(l)));
        assert!(!has_next_page(None));
    }

    #[test]
    fn rest_page_wrapped_and_malformed() {
        let body = r#"[
          {"starred_at":"2026-03-01T00:00:00Z","repo":{
            "id":1,"full_name":"o/a","name":"a","html_url":"https://github.com/o/a",
            "stargazers_count":5,"updated_at":"2026-01-01T00:00:00Z"}},
          {"starred_at":"2026-02-01T00:00:00Z","repo":{"id":"not-a-number"}}
        ]"#;
        let (repos, skipped) = parse_rest_page(body).unwrap();
        assert_eq!(repos.len(), 1);
        assert_eq!(skipped, 1);
        assert_eq!(
            repos[0].starred_at.unwrap().to_rfc3339(),
            "2026-03-01T00:00:00+00:00"
        );
    }

    #[test]
    fn rest_page_rejects_non_array() {
        assert!(matches!(
            parse_rest_page(r#"{"message":"x"}"#),
            Err(GitHubError::Parse(_))
        ));
    }

    #[test]
    fn graphql_page_maps_fields() {
        let body = r#"{"data":{"user":{"starredRepositories":{
          "pageInfo":{"hasNextPage":true,"endCursor":"c1"},
          "edges":[{"starredAt":"2026-03-01T00:00:00Z","node":{
            "databaseId":42,"nameWithOwner":"o/r","name":"r","description":"d",
            "url":"https://github.com/o/r","stargazerCount":9,
            "primaryLanguage":{"name":"Rust"},
            "repositoryTopics":{"nodes":[{"topic":{"name":"tui"}}]},
            "updatedAt":"2026-01-01T00:00:00Z","pushedAt":null,
            "isFork":false,"isArchived":true}}]}}}}"#;
        let page = parse_graphql_page(body, "o").unwrap();
        assert!(page.has_next);
        assert_eq!(page.end_cursor.as_deref(), Some("c1"));
        let r = &page.repos[0];
        assert_eq!(
            (r.id, r.full_name.as_str(), r.language.as_deref()),
            (42, "o/r", Some("Rust"))
        );
        assert_eq!(r.topics, vec!["tui"]);
        assert!(r.archived);
    }

    #[test]
    fn graphql_errors_are_typed() {
        let nf = r#"{"data":{"user":null},"errors":[{"type":"NOT_FOUND","message":"Could not resolve"}]}"#;
        assert!(matches!(
            parse_graphql_page(nf, "ghost"),
            Err(GitHubError::UserNotFound(u)) if u == "ghost"
        ));
        let rl = r#"{"errors":[{"type":"RATE_LIMITED","message":"slow down"}]}"#;
        assert!(matches!(
            parse_graphql_page(rl, "x"),
            Err(GitHubError::RateLimited { .. })
        ));
    }

    #[test]
    fn classify_rate_limit_and_auth() {
        let mut h = header::HeaderMap::new();
        h.insert("x-ratelimit-remaining", "0".parse().unwrap());
        h.insert("x-ratelimit-reset", "1800000000".parse().unwrap());
        let e = classify_error(
            StatusCode::FORBIDDEN,
            &h,
            r#"{"message":"API rate limit exceeded"}"#,
            "/x",
        );
        match e {
            GitHubError::RateLimited { reset: Some(t) } => assert_eq!(t.timestamp(), 1_800_000_000),
            other => panic!("{other:?}"),
        }
        let none = header::HeaderMap::new();
        assert!(matches!(
            classify_error(StatusCode::UNAUTHORIZED, &none, "", "/x"),
            GitHubError::Unauthorized
        ));
        match classify_error(
            StatusCode::FORBIDDEN,
            &none,
            r#"{"message":"Resource not accessible"}"#,
            "/x",
        ) {
            GitHubError::Api {
                status: 403,
                message,
            } => assert_eq!(message, "Resource not accessible"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn repos_meta_query_uses_variables_not_interpolation() {
        let repos = [(0, ("rust-lang", "rust")), (3, ("o", "r\"} evil"))];
        let (query, vars) = repos_meta_query(&repos);
        assert!(
            !query.contains("rust-lang") && !query.contains("evil"),
            "{query}"
        );
        assert!(query.contains("r0: repository(owner: $o0, name: $n0)"));
        assert!(query.contains("$o1: String!, $n1: String!"));
        assert_eq!(vars["o0"], "rust-lang");
        assert_eq!(vars["n1"], "r\"} evil");
    }

    #[test]
    fn repos_meta_partial_data_rename_and_missing() {
        let repos = [
            (0, ("rust-lang-nursery", "rustup.rs")),
            (2, ("ghost", "gone")),
        ];
        let body = r#"{"data":{
            "r0":{"nameWithOwner":"rust-lang/rustup","databaseId":7,"stargazerCount":7125,
                  "description":"installer","url":"https://github.com/rust-lang/rustup",
                  "primaryLanguage":{"name":"Rust"},
                  "repositoryTopics":{"nodes":[{"topic":{"name":"rust"}}]},
                  "pushedAt":"2026-11-01T00:00:00Z","updatedAt":"2026-11-01T00:00:00Z",
                  "isArchived":false,"isFork":false},
            "r1":null},
          "errors":[{"type":"NOT_FOUND","path":["r1"],"message":"Could not resolve"}]}"#;
        let got = parse_repos_meta(body, &repos).unwrap();
        assert_eq!(got.len(), 2);
        let (slot, meta) = &got[0];
        assert_eq!(*slot, 0);
        let meta = meta.as_ref().unwrap();
        assert_eq!(meta.name_with_owner, "rust-lang/rustup", "rename resolved");
        assert_eq!(
            (meta.stargazer_count, meta.language.as_deref()),
            (7125, Some("Rust"))
        );
        assert_eq!(got[1], (2, None), "missing repo keeps its slot");
    }

    #[test]
    fn repos_meta_only_not_found_or_forbidden_means_missing() {
        // PR #7 review (Codex): any other error used to be cached as a
        // missing repository and never retried.
        let repos = [(0, ("o", "r")), (1, ("o", "s"))];
        let node = r#"{"nameWithOwner":"o/r","databaseId":1,"stargazerCount":5,
            "description":null,"url":"https://github.com/o/r","primaryLanguage":null,
            "pushedAt":null,"updatedAt":"2026-11-01T00:00:00Z","isArchived":false,"isFork":false}"#;
        let forbidden = format!(
            r#"{{"data":{{"r0":{node},"r1":null}},"errors":[{{"type":"FORBIDDEN","path":["r1"],"message":"SAML"}}]}}"#
        );
        let ok = parse_repos_meta(&forbidden, &repos).unwrap();
        assert_eq!(ok[1], (1, None), "inaccessible counts as missing");

        let transient = format!(
            r#"{{"data":{{"r0":{node},"r1":null}},"errors":[{{"type":"SERVICE_UNAVAILABLE","path":["r1"],"message":"try later"}}]}}"#
        );
        assert!(matches!(
            parse_repos_meta(&transient, &repos),
            Err(GitHubError::Api { .. })
        ));
        let untyped = format!(
            r#"{{"data":{{"r0":{node},"r1":null}},"errors":[{{"message":"Something went wrong"}}]}}"#
        );
        assert!(parse_repos_meta(&untyped, &repos).is_err());

        // agy review of PR #8: a null or absent alias without GitHub's own
        // not-found error is an incomplete response, not a missing repo.
        let unexplained = format!(r#"{{"data":{{"r0":{node},"r1":null}}}}"#);
        assert!(matches!(
            parse_repos_meta(&unexplained, &repos),
            Err(GitHubError::Parse(_))
        ));
        let absent = format!(r#"{{"data":{{"r0":{node}}}}}"#);
        assert!(parse_repos_meta(&absent, &repos).is_err());

        let malformed = r#"{"data":{"r0":{"nameWithOwner":7},"r1":null}}"#;
        assert!(matches!(
            parse_repos_meta(malformed, &repos),
            Err(GitHubError::Parse(_))
        ));
    }

    #[test]
    fn repos_meta_rate_limit_and_no_data_are_errors() {
        let repos = [(0, ("o", "r"))];
        let rl = r#"{"data":null,"errors":[{"type":"RATE_LIMITED","message":"slow"}]}"#;
        assert!(matches!(
            parse_repos_meta(rl, &repos),
            Err(GitHubError::RateLimited { .. })
        ));
        let none = r#"{"errors":[{"message":"Something broke"}]}"#;
        assert!(matches!(
            parse_repos_meta(none, &repos),
            Err(GitHubError::Api { .. })
        ));
    }

    #[test]
    fn error_display_mentions_reset() {
        let t = Utc.timestamp_opt(1_800_000_000, 0).single();
        let s = GitHubError::RateLimited { reset: t }.to_string();
        assert!(s.contains("resets at"), "{s}");
    }
}
