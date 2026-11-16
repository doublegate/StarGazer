//! stargazer: TUI browser for GitHub starred repositories.
//!
//! Architecture:
//!   - `tokio::main` for the async runtime; this file only parses the CLI,
//!     resolves config / token / user, and hands off to `stargazer::tui::run`
//!     (or runs the `export` / `paths` subcommands without a TUI).
//!   - Main task: ratatui event loop (`tui.rs`) that `select!`s over terminal
//!     events, background results and a 100 ms tick, redrawing only on change.
//!   - Background tasks: GitHub sync, README fetches and unstar requests run via
//!     `tokio::spawn` and report back as `BgMsg`s over a `tokio::sync::mpsc`
//!     channel; `App` (`app.rs`) owns all UI state and performs no I/O itself.
//!
//! On first launch: full import of all stars, with progress in the status line.
//! Subsequent launches: load the cache instantly, then incrementally sync new
//! stars in the background while the UI is already interactive. A full resync
//! (which also drops unstarred repos) runs every `stale_days`, on `r`, or with
//! `--refresh`.

use anyhow::{Context as _, Result, bail};
use clap::{Parser, Subcommand};
use stargazer::app::{App, CategorySort, Level, SortMode};
use stargazer::cache::{Loaded, StarCache};
use stargazer::config::{Config, ThemeName, env_flag};
use stargazer::export::{Format, export};
use stargazer::github::{
    DEFAULT_API_URL, FetchEvent, GitHubClient, resolve_token, validate_username,
};
use stargazer::sync::{
    SyncMode, export_needs_sync, refresh_categories, same_user, sync, upgrade_notice,
};
use stargazer::theme::{Glyphs, Theme};
use stargazer::tui::{self, Context};
use stargazer::ui::Ui;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    /// GitHub user whose stars to browse [default: the token's owner]
    #[arg(short, long, env = "STARGAZER_USER")]
    user: Option<String>,

    /// Plain-ASCII icons and glyphs (also STARGAZER_ASCII=1)
    #[arg(long)]
    ascii: bool,

    /// Color theme (NO_COLOR selects mono unless this is given)
    #[arg(long, value_enum)]
    theme: Option<ThemeName>,

    /// Initial sort order of the repo list
    #[arg(long, value_enum)]
    sort: Option<SortMode>,

    /// Initial sort order of the Categories pane
    #[arg(long, value_enum)]
    category_sort: Option<CategorySort>,

    /// Full resync at startup (also drops repos you have unstarred)
    #[arg(long)]
    refresh: bool,

    /// Disable mouse capture (keeps terminal text selection working)
    #[arg(long)]
    no_mouse: bool,

    /// Config file [default: platform config dir/stargazer/config.toml]
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,

    /// Cache file [default: platform data dir/stargazer/stars.json]
    #[arg(long, value_name = "PATH")]
    cache: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Export cached stars to stdout or a file
    Export {
        #[arg(short, long, value_enum, default_value = "md")]
        format: Format,
        /// Output file [default: stdout]
        #[arg(short, long, value_name = "PATH")]
        output: Option<PathBuf>,
        /// Only this category (e.g. "Rust", "AI / ML")
        #[arg(long)]
        category: Option<String>,
        /// Sync before exporting
        #[arg(long)]
        sync: bool,
    },
    /// Print the config and cache file locations
    Paths,
}

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("stargazer: {e:#}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let cli = Cli::parse();

    let config_path = match &cli.config {
        Some(p) => p.clone(),
        None => Config::default_path()?,
    };
    let config = Config::load(&config_path)?;
    let cache_path = match &cli.cache {
        Some(p) => p.clone(),
        None => StarCache::default_path()?,
    };

    let ascii = cli.ascii || env_flag("STARGAZER_ASCII") || config.ascii;
    let glyphs = Glyphs::new(ascii);
    let sort = cli.sort.unwrap_or(config.sort);

    match cli.command {
        Some(Command::Paths) => {
            println!("config: {}", config_path.display());
            println!("cache:  {}", cache_path.display());
            println!(
                "awesome: {}",
                stargazer::awesome::Store::beside(&cache_path)
                    .dir()
                    .display()
            );
            Ok(())
        }
        Some(Command::Export {
            format,
            output,
            category,
            sync: do_sync,
        }) => {
            let loaded = load_cache(&cache_path);
            if let Some(w) = loaded.warning {
                eprintln!("warning: {w}");
            }
            let mut cache = loaded.cache;
            let requested = cli.user.as_deref().or(config.user.as_deref());
            if export_needs_sync(requested, &cache, do_sync) {
                let cached_user = cache.username.clone();
                let (client, user) =
                    connect(cli.user.as_deref(), &config, Some(&cached_user)).await?;
                let mut progress = |ev: FetchEvent| match ev {
                    FetchEvent::Progress { message, .. } | FetchEvent::Finished(message) => {
                        eprintln!("{message}")
                    }
                    FetchEvent::Warning(w) => eprintln!("warning: {w}"),
                };
                cache = sync(
                    &client,
                    &user,
                    cache,
                    &cache_path,
                    SyncMode::Auto,
                    config.stale_days,
                    &mut progress,
                )
                .await
                .map_err(|f| f.error)?;
            }
            let _ = refresh_categories(&mut cache);
            match output {
                Some(path) => {
                    let mut file = std::fs::File::create(&path)
                        .with_context(|| format!("creating {}", path.display()))?;
                    let n = export(
                        &cache.repos,
                        format,
                        sort,
                        category.as_deref(),
                        &glyphs,
                        &mut file,
                    )?;
                    file.flush()?;
                    eprintln!("wrote {n} repos to {}", path.display());
                }
                None => {
                    let mut out = std::io::stdout().lock();
                    let result = export(
                        &cache.repos,
                        format,
                        sort,
                        category.as_deref(),
                        &glyphs,
                        &mut out,
                    )
                    .and_then(|_| Ok(out.flush()?));
                    // `stargazer export | head` closes the pipe early; that is
                    // not an error worth reporting.
                    if let Err(e) = result
                        && !is_broken_pipe(&e)
                    {
                        return Err(e);
                    }
                }
            }
            Ok(())
        }
        None => {
            let theme_name = cli.theme.unwrap_or(
                if std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty()) {
                    ThemeName::Mono
                } else {
                    config.theme
                },
            );
            let theme = Theme::preset(theme_name)
                .with_overrides(&config.colors)
                .with_context(|| format!("in [colors] of {}", config_path.display()))?;

            let loaded = load_cache(&cache_path);
            let (client, user) =
                connect(cli.user.as_deref(), &config, Some(&loaded.cache.username)).await?;
            let mut cache = loaded.cache;
            let own_cache = same_user(&cache.username, &user);
            if !own_cache {
                // Never show another account's stars while the new list loads.
                cache = StarCache::default();
            }
            let changed = refresh_categories(&mut cache);

            // An older cache format (e.g. v0.1) was upgraded in memory. Save it
            // in the new format right away so the notice below appears exactly
            // once, even if the first sync fails. A failed save is harmless:
            // the next successful sync writes the file anyway.
            let notice = match loaded.upgraded_from {
                Some(from) if own_cache && !cache.repos.is_empty() => {
                    let _ = cache.save_to(&cache_path);
                    Some(upgrade_notice(from, changed, cache.repos.len()))
                }
                _ => None,
            };

            let mut app = App::new(&user, cache, sort, client.has_token());
            app.ascii = ascii;
            let cat_sort = cli.category_sort.unwrap_or(config.category_sort);
            if cat_sort != CategorySort::Default {
                app.set_cat_sort(cat_sort);
                app.status = None; // no "Category sort" toast at startup
            }
            if let Some(w) = loaded.warning {
                app.set_status(Level::Warn, w);
            } else if let Some(n) = notice {
                app.set_status(Level::Notice, n);
            }
            let ctx = Context {
                client: Arc::new(client),
                user,
                cache_path,
                stale_days: config.stale_days,
                mouse: config.mouse && !cli.no_mouse,
                refresh: cli.refresh,
            };
            tui::run(app, Ui::new(theme, glyphs), ctx).await
        }
    }
}

fn is_broken_pipe(e: &anyhow::Error) -> bool {
    e.chain().any(|c| {
        c.downcast_ref::<std::io::Error>()
            .is_some_and(|io| io.kind() == std::io::ErrorKind::BrokenPipe)
            || c.downcast_ref::<csv::Error>().is_some_and(|ce| {
                matches!(ce.kind(), csv::ErrorKind::Io(io) if io.kind() == std::io::ErrorKind::BrokenPipe)
            })
            || c.downcast_ref::<serde_json::Error>()
                .is_some_and(|je| je.io_error_kind() == Some(std::io::ErrorKind::BrokenPipe))
    })
}

/// Load the cache, degrading to an empty one with a warning when the file
/// cannot be read at all (v0.1 behaved the same way: `unwrap_or_default`).
fn load_cache(path: &std::path::Path) -> Loaded {
    StarCache::load_from(path).unwrap_or_else(|e| Loaded {
        cache: StarCache::default(),
        warning: Some(format!("could not read cache: {e:#}")),
        upgraded_from: None,
    })
}

/// Resolve the token and the user. The user is, in order: `--user` /
/// `STARGAZER_USER`, the config file, the user the existing cache belongs to,
/// and finally the token's owner. Only that last step touches the network,
/// so with a cache startup is instant and works offline, as in v0.1.
async fn connect(
    cli_user: Option<&str>,
    config: &Config,
    cached_user: Option<&str>,
) -> Result<(GitHubClient, String)> {
    let token = resolve_token().await;
    let client = GitHubClient::new(DEFAULT_API_URL, token)?;
    let cached_user = cached_user
        .filter(|u| !u.is_empty())
        .filter(|u| validate_username(u).is_ok());
    let user = match cli_user.or(config.user.as_deref()).or(cached_user) {
        Some(u) => u.trim().to_string(),
        None if client.has_token() => client
            .viewer_login()
            .await
            .context("looking up the authenticated GitHub user")?,
        None => bail!(
            "no GitHub user configured. Pass --user <login>, set STARGAZER_USER, \
             add `user = \"<login>\"` to the config file (see `stargazer paths`), \
             or sign in with `gh auth login` so your own stars are used"
        ),
    };
    validate_username(&user)?;
    Ok((client, user))
}
