//! Sync orchestration: decide full vs incremental, fetch, categorize, persist.
//!
//! - A **full** sync replaces the cached list, which is also how unstarred
//!   repos disappear. It runs on first launch, when the cache belongs to a
//!   different user, when a previous import never finished, when the last full
//!   sync is older than `stale_days`, or on request (`r` / `--refresh`).
//! - An **incremental** sync fetches newest-first and stops at the first
//!   already-cached repo.
//! - A failed import keeps what it fetched: the partial list is saved with
//!   `complete = false`, so it is shown next launch and the next sync is full.

use crate::cache::{CACHE_VERSION, StarCache, dedupe, merge};
use crate::categories::{RULES_VERSION, categorize};
use crate::github::{FetchEvent, GitHubClient, GitHubError, Progress};
use chrono::{Duration, Utc};
use std::collections::HashSet;
use std::path::Path;

/// What the caller asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncMode {
    /// Incremental unless a full sync is due.
    Auto,
    /// Always a full sync.
    Full,
}

/// A failed sync, with whatever was salvaged.
#[derive(Debug)]
pub struct SyncFailure {
    pub error: GitHubError,
    /// Partial cache saved from an interrupted import, if any.
    pub partial: Option<StarCache>,
}

/// GitHub logins are case-insensitive: "DoubleGate" and "doublegate" are the
/// same account, so a cache must not be discarded over letter case.
pub fn same_user(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

/// Whether a full sync is required for `cache` and `user`.
pub fn needs_full(cache: &StarCache, user: &str, mode: SyncMode, stale_days: u32) -> bool {
    if mode == SyncMode::Full
        || !same_user(&cache.username, user)
        || !cache.complete
        || cache.repos.is_empty()
    {
        return true;
    }
    if stale_days == 0 {
        return false;
    }
    match cache.full_synced_at {
        Some(t) => Utc::now() - t > Duration::days(i64::from(stale_days)),
        // v0.1 caches never recorded a full sync; use fetched_at as a proxy.
        None => cache
            .fetched_at
            .is_none_or(|t| Utc::now() - t > Duration::days(i64::from(stale_days))),
    }
}

/// Recompute categories when the rules changed since the cache was written.
/// Returns how many repos ended up in a different category.
pub fn refresh_categories(cache: &mut StarCache) -> usize {
    let mut changed = 0;
    if cache.rules_version != RULES_VERSION {
        for r in &mut cache.repos {
            let cat = categorize(r);
            if cat != r.category {
                changed += 1;
                r.category = cat;
            }
        }
        cache.rules_version = RULES_VERSION;
    }
    changed
}

/// One-time status message shown after an older cache format was upgraded.
pub fn upgrade_notice(from_version: u32, changed: usize, total: usize) -> String {
    let from = match from_version {
        0 | 1 => "v0.1".to_string(),
        v => format!("format {v}"),
    };
    format!(
        "Upgraded {from} cache: {changed} of {total} repos changed category under the new rules (see CHANGELOG)"
    )
}

/// Run one sync for `user`, starting from `cache`, saving to `path`.
pub async fn sync(
    client: &GitHubClient,
    user: &str,
    mut cache: StarCache,
    path: &Path,
    mode: SyncMode,
    stale_days: u32,
    progress: &mut Progress<'_>,
) -> Result<StarCache, Box<SyncFailure>> {
    let full = needs_full(&cache, user, mode, stale_days);
    if !same_user(&cache.username, user) {
        cache = StarCache::default();
    }
    cache.username = user.to_string();
    cache.version = CACHE_VERSION;

    let known: HashSet<u64> = if full {
        HashSet::new()
    } else {
        cache.known_ids()
    };
    let etag = if full { None } else { cache.rest_etag.clone() };

    progress(FetchEvent::Progress {
        fetched: 0,
        message: if full {
            "Importing all starred repositories".into()
        } else {
            "Checking for new stars".into()
        },
    });

    let mut acc = Vec::new();
    let fetched = client
        .fetch_starred(user, &known, etag.as_deref(), &mut acc, progress)
        .await;

    let result = match fetched {
        Ok(r) => r,
        Err(error) => {
            // Keep partial progress only when there was nothing better cached.
            let partial = if full && cache.repos.is_empty() && !acc.is_empty() {
                let mut partial = cache;
                partial.repos = dedupe(acc);
                for r in &mut partial.repos {
                    r.category = categorize(r);
                }
                partial.rules_version = RULES_VERSION;
                partial.complete = false;
                partial.save_to(path).ok().map(|_| partial)
            } else {
                None
            };
            return Err(Box::new(SyncFailure { error, partial }));
        }
    };

    let mut new = result.repos;
    for r in &mut new {
        r.category = categorize(r);
    }
    let new_count = new.len();

    if full {
        cache.repos = dedupe(new);
        cache.complete = true;
        cache.full_synced_at = Some(Utc::now());
    } else if !result.not_modified {
        cache.repos = merge(new, std::mem::take(&mut cache.repos));
    }
    if result.etag.is_some() {
        cache.rest_etag = result.etag;
    }
    let _ = refresh_categories(&mut cache);
    cache.fetched_at = Some(Utc::now());

    if let Err(e) = cache.save_to(path) {
        progress(FetchEvent::Warning(format!("could not save cache: {e:#}")));
    }
    progress(FetchEvent::Finished(if full {
        format!("Imported {} stars", cache.repos.len())
    } else if new_count == 0 {
        "Already up to date".into()
    } else {
        format!("Found {new_count} new star(s)")
    }));
    Ok(cache)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::StarredRepo;

    fn cache(user: &str, n: u64, complete: bool) -> StarCache {
        StarCache {
            version: CACHE_VERSION,
            username: user.into(),
            fetched_at: Some(Utc::now()),
            full_synced_at: Some(Utc::now()),
            complete,
            rules_version: RULES_VERSION,
            rest_etag: None,
            repos: (0..n)
                .map(|id| StarredRepo {
                    id,
                    ..StarredRepo::test_default()
                })
                .collect(),
        }
    }

    #[test]
    fn full_sync_decisions() {
        let c = cache("me", 3, true);
        assert!(!needs_full(&c, "me", SyncMode::Auto, 7));
        assert!(
            !needs_full(&c, "ME", SyncMode::Auto, 7),
            "logins are case-insensitive"
        );
        assert!(needs_full(&c, "me", SyncMode::Full, 7));
        assert!(needs_full(&c, "someone-else", SyncMode::Auto, 7));
        assert!(needs_full(&cache("me", 3, false), "me", SyncMode::Auto, 7));
        assert!(needs_full(&cache("me", 0, true), "me", SyncMode::Auto, 7));

        let mut stale = cache("me", 3, true);
        stale.full_synced_at = Some(Utc::now() - Duration::days(8));
        assert!(needs_full(&stale, "me", SyncMode::Auto, 7));
        assert!(!needs_full(&stale, "me", SyncMode::Auto, 0));
    }

    #[test]
    fn categories_recomputed_when_rules_change() {
        let mut c = cache("me", 1, true);
        c.repos[0].language = Some("Rust".into());
        c.repos[0].category = "Other".into();
        c.rules_version = 1;
        assert_eq!(refresh_categories(&mut c), 1);
        assert_eq!(c.repos[0].category, "Rust");
        assert_eq!(c.rules_version, RULES_VERSION);
        assert_eq!(refresh_categories(&mut c), 0, "no-op once current");
    }

    #[test]
    fn upgrade_notice_text() {
        assert_eq!(
            upgrade_notice(0, 178, 645),
            "Upgraded v0.1 cache: 178 of 645 repos changed category under the new rules (see CHANGELOG)"
        );
        assert!(upgrade_notice(2, 0, 1).starts_with("Upgraded format 2 cache"));
    }
}
