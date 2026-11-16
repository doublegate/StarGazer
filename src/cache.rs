//! Starred-repo model and the on-disk cache.
//!
//! The cache lives at `$XDG_DATA_HOME/stargazer/stars.json` (platform
//! equivalent elsewhere). Writes are atomic (temp file, fsync, rename) and the
//! file is created mode 0600 on Unix. A cache that fails to parse is moved
//! aside to `stars.json.bad` so the next sync can rebuild it instead of
//! failing forever.

use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Utc};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Current on-disk schema version.
pub const CACHE_VERSION: u32 = 2;

/// A single starred repository. Field names follow the GitHub REST API so the
/// v0.1 cache format still deserializes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StarredRepo {
    pub id: u64,
    /// `owner/name`
    pub full_name: String,
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
    #[serde(default)]
    pub pushed_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub fork: bool,
    #[serde(default)]
    pub archived: bool,
    /// When the user starred it.
    #[serde(default)]
    pub starred_at: Option<DateTime<Utc>>,
    /// Computed category (not from the API).
    #[serde(default)]
    pub category: String,
}

impl StarredRepo {
    /// Copy with every display string passed through [`crate::text::sanitize`].
    pub fn sanitized(mut self) -> Self {
        use crate::text::sanitize;
        self.full_name = sanitize(&self.full_name);
        self.name = sanitize(&self.name);
        self.description = self
            .description
            .as_deref()
            .map(sanitize)
            .filter(|d| !d.is_empty());
        self.language = self.language.as_deref().map(sanitize);
        self.topics = self.topics.iter().map(|t| sanitize(t)).collect();
        self
    }

    /// A minimal repo for tests.
    #[doc(hidden)]
    pub fn test_default() -> Self {
        Self {
            id: 0,
            full_name: "owner/repo".into(),
            name: "repo".into(),
            description: None,
            html_url: "https://github.com/owner/repo".into(),
            stargazers_count: 0,
            language: None,
            topics: Vec::new(),
            updated_at: DateTime::<Utc>::UNIX_EPOCH,
            pushed_at: None,
            fork: false,
            archived: false,
            starred_at: None,
            category: String::new(),
        }
    }
}

/// Persisted cache file format.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct StarCache {
    /// Schema version; absent (0) in v0.1 caches.
    #[serde(default)]
    pub version: u32,
    pub username: String,
    /// Last successful sync of any kind.
    pub fetched_at: Option<DateTime<Utc>>,
    /// Last successful *full* sync (reconciles unstars).
    #[serde(default)]
    pub full_synced_at: Option<DateTime<Utc>>,
    /// False while a full import is incomplete; forces the next sync to be full.
    #[serde(default)]
    pub complete: bool,
    /// Categorization rules version the stored categories were computed with.
    #[serde(default)]
    pub rules_version: u32,
    /// ETag of REST page one, for conditional anonymous syncs.
    #[serde(default)]
    pub rest_etag: Option<String>,
    pub repos: Vec<StarredRepo>,
}

/// Result of [`StarCache::load_from`].
#[derive(Debug)]
pub struct Loaded {
    pub cache: StarCache,
    /// Set when the file was unreadable and was moved aside.
    pub warning: Option<String>,
}

impl StarCache {
    /// Default cache location under the platform data directory.
    pub fn default_path() -> Result<PathBuf> {
        let dirs = ProjectDirs::from("", "", "stargazer")
            .ok_or_else(|| anyhow!("cannot determine a data directory for this platform"))?;
        Ok(dirs.data_dir().join("stars.json"))
    }

    /// Load the cache. A missing file yields an empty cache; a corrupt one is
    /// renamed to `<path>.bad` and also yields an empty cache plus a warning.
    pub fn load_from(path: &Path) -> Result<Loaded> {
        let raw = match std::fs::read_to_string(path) {
            Ok(raw) => raw,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Loaded {
                    cache: Self::default(),
                    warning: None,
                });
            }
            Err(e) => return Err(e).with_context(|| format!("reading cache {}", path.display())),
        };

        match serde_json::from_str::<Self>(&raw) {
            Ok(mut cache) => {
                if cache.version < CACHE_VERSION {
                    // v0.1 caches had no completeness flag; a non-empty one was
                    // the product of a finished import.
                    cache.complete = !cache.repos.is_empty();
                    cache.version = CACHE_VERSION;
                }
                Ok(Loaded {
                    cache,
                    warning: None,
                })
            }
            Err(e) => {
                let bad = path.with_extension("json.bad");
                std::fs::rename(path, &bad)
                    .with_context(|| format!("moving corrupt cache to {}", bad.display()))?;
                Ok(Loaded {
                    cache: Self::default(),
                    warning: Some(format!(
                        "cache was unreadable ({e}); moved to {} and rebuilding",
                        bad.display()
                    )),
                })
            }
        }
    }

    /// Atomically persist the cache to `path`.
    pub fn save_to(&self, path: &Path) -> Result<()> {
        let dir = path
            .parent()
            .ok_or_else(|| anyhow!("cache path has no parent: {}", path.display()))?;
        std::fs::create_dir_all(dir)
            .with_context(|| format!("creating data dir {}", dir.display()))?;

        // Unique per process and per call, so concurrent writers (two
        // instances, or the TUI and `export --sync`) never share a temp file;
        // the final rename is atomic, so the last complete write wins.
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut tmp = path.as_os_str().to_owned();
        tmp.push(format!(".{}.{seq}.tmp", std::process::id()));
        let tmp = PathBuf::from(tmp);

        let json = serde_json::to_vec_pretty(self)?;
        let write = || -> Result<()> {
            let mut opts = std::fs::OpenOptions::new();
            // Not create_new: a stale temp file from a crashed process with a
            // reused pid must not block saving.
            opts.write(true).create(true).truncate(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                opts.mode(0o600);
            }
            let mut f = opts
                .open(&tmp)
                .with_context(|| format!("writing {}", tmp.display()))?;
            f.write_all(&json)?;
            f.sync_all()?;
            drop(f);
            std::fs::rename(&tmp, path)
                .with_context(|| format!("replacing cache {}", path.display()))
        };
        let result = write();
        if result.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        result
    }

    /// Ids of every cached repo.
    pub fn known_ids(&self) -> HashSet<u64> {
        self.repos.iter().map(|r| r.id).collect()
    }
}

/// Prepend `new_newest_first` to `existing`, dropping any repo already present
/// earlier in the result (by id). Order: new stars first, then existing order.
pub fn merge(new_newest_first: Vec<StarredRepo>, existing: Vec<StarredRepo>) -> Vec<StarredRepo> {
    let mut seen = HashSet::with_capacity(new_newest_first.len() + existing.len());
    new_newest_first
        .into_iter()
        .chain(existing)
        .filter(|r| seen.insert(r.id))
        .collect()
}

/// Drop duplicate ids, keeping the first occurrence.
pub fn dedupe(repos: Vec<StarredRepo>) -> Vec<StarredRepo> {
    merge(repos, Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(id: u64) -> StarredRepo {
        StarredRepo {
            id,
            full_name: format!("o/r{id}"),
            ..StarredRepo::test_default()
        }
    }

    #[test]
    fn merge_prepends_and_dedupes() {
        let merged = merge(vec![r(5), r(4), r(3)], vec![r(3), r(2), r(1)]);
        let ids: Vec<u64> = merged.iter().map(|r| r.id).collect();
        assert_eq!(ids, vec![5, 4, 3, 2, 1]);
    }

    #[test]
    fn dedupe_keeps_first() {
        let ids: Vec<u64> = dedupe(vec![r(1), r(2), r(1)])
            .iter()
            .map(|r| r.id)
            .collect();
        assert_eq!(ids, vec![1, 2]);
    }

    #[test]
    fn round_trip_and_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/stars.json");
        let cache = StarCache {
            version: CACHE_VERSION,
            username: "octocat".into(),
            complete: true,
            repos: vec![r(1), r(2)],
            ..Default::default()
        };
        cache.save_to(&path).unwrap();
        let loaded = StarCache::load_from(&path).unwrap();
        assert!(loaded.warning.is_none());
        assert_eq!(loaded.cache, cache);
        let leftovers: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "temp files left behind: {leftovers:?}"
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn missing_file_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let loaded = StarCache::load_from(&dir.path().join("nope.json")).unwrap();
        assert!(loaded.cache.repos.is_empty());
        assert!(loaded.warning.is_none());
    }

    #[test]
    fn corrupt_file_is_moved_aside() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stars.json");
        std::fs::write(&path, "{ truncated").unwrap();
        let loaded = StarCache::load_from(&path).unwrap();
        assert!(loaded.cache.repos.is_empty());
        assert!(loaded.warning.unwrap().contains("unreadable"));
        assert!(!path.exists());
        assert!(dir.path().join("stars.json.bad").exists());
    }

    #[test]
    fn v01_cache_still_loads_and_is_marked_complete() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stars.json");
        let v01 = r#"{
          "username": "octocat",
          "fetched_at": "2026-01-01T00:00:00Z",
          "repos": [{
            "id": 7, "full_name": "o/r", "name": "r", "description": "d",
            "html_url": "https://github.com/o/r", "stargazers_count": 3,
            "language": "Rust", "topics": ["rust"],
            "updated_at": "2026-01-01T00:00:00Z", "pushed_at": null,
            "fork": false, "archived": false, "starred_at": null, "category": "Rust"
          }]
        }"#;
        std::fs::write(&path, v01).unwrap();
        let loaded = StarCache::load_from(&path).unwrap().cache;
        assert_eq!(loaded.version, CACHE_VERSION);
        assert!(loaded.complete);
        assert_eq!(loaded.repos[0].id, 7);
    }
}
