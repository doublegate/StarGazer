//! Terminal lifecycle and the async event loop.
//!
//! `ratatui::try_init` enables raw mode and the alternate screen and installs
//! a panic hook that restores the terminal; mouse capture is layered on top
//! with its own hook. The loop `select!`s over terminal events, background
//! results and a 100 ms tick, and redraws only when something changed.
//!
//! Cache writes are serialized by one async mutex: a sync holds it from
//! loading the cache until its result has been sent to the UI, and an unstar
//! takes it only after the API call succeeded. So an unstar that lands while
//! a sync is running edits the file *after* the sync saved its snapshot (the
//! repo cannot come back), and its `Unstarred` message reaches the UI after
//! the sync's `SyncDone` (channel order), removing the repo from the view too.
//!
//! Awesome data (index and lists, see [`crate::awesome`]) lives in separate
//! files and does not take that lock: a list fetch can take seconds and must
//! not hold up an unstar. After each star sync, once the lock is released,
//! the index is revalidated; an awesome list is shown from its cache at once
//! and re-fetched when stale, incomplete, or on a full refresh.

use crate::app::{App, BgMsg, Effect, Level};
use crate::awesome::{self, Store};
use crate::cache::StarCache;
use crate::clipboard::Clipboard;
use crate::github::{FetchEvent, GitHubClient};
use crate::sync::{SyncMode, same_user, sync};
use crate::ui::Ui;
use anyhow::Result;
use crossterm::event::{DisableMouseCapture, EnableMouseCapture, Event, EventStream};
use crossterm::execute;
use futures::StreamExt;
use ratatui::DefaultTerminal;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};

type CacheLock = Arc<Mutex<()>>;

/// Everything the loop needs besides the app and renderer.
pub struct Context {
    pub client: Arc<GitHubClient>,
    /// GitHub login being browsed.
    pub user: String,
    pub cache_path: PathBuf,
    pub stale_days: u32,
    pub mouse: bool,
    /// Force a full sync at startup (`--refresh`).
    pub refresh: bool,
}

/// Run the TUI until the user quits.
pub async fn run(app: App, ui: Ui, ctx: Context) -> Result<()> {
    let mut terminal = ratatui::try_init()?;
    if ctx.mouse {
        let _ = execute!(std::io::stdout(), EnableMouseCapture);
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let _ = execute!(std::io::stdout(), DisableMouseCapture);
            prev(info);
        }));
    }
    let result = event_loop(&mut terminal, app, ui, &ctx).await;
    if ctx.mouse {
        let _ = execute!(std::io::stdout(), DisableMouseCapture);
    }
    ratatui::restore();
    result
}

async fn event_loop(
    terminal: &mut DefaultTerminal,
    mut app: App,
    ui: Ui,
    ctx: &Context,
) -> Result<()> {
    let (tx, mut rx) = unbounded_channel::<BgMsg>();
    let mut events = EventStream::new();
    let mut tick = tokio::time::interval(Duration::from_millis(100));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut clipboard = Clipboard::new();
    let lock: CacheLock = Arc::new(Mutex::new(()));

    let mode = if ctx.refresh {
        SyncMode::Full
    } else {
        SyncMode::Auto
    };
    let store = Store::beside(&ctx.cache_path);
    app.set_awesome(store.load_index().map(|c| c.index), store.last_selection());
    app.syncing = true;
    spawn_sync(ctx, mode, tx.clone(), Arc::clone(&lock));

    let mut dirty = true;
    loop {
        for effect in app.drain_effects() {
            run_effect(effect, &mut app, terminal, &mut clipboard, ctx, &tx, &lock)?;
            dirty = true;
        }
        if dirty {
            terminal.draw(|f| ui.render(f, &mut app))?;
            dirty = false;
        }
        if app.should_quit {
            return Ok(());
        }

        tokio::select! {
            ev = events.next() => match ev {
                Some(Ok(Event::Key(key))) => { app.handle_key(key); dirty = true; }
                Some(Ok(Event::Mouse(m))) if ctx.mouse => { app.handle_mouse(m); dirty = true; }
                Some(Ok(Event::Resize(_, _))) => { terminal.clear()?; dirty = true; }
                Some(Ok(_)) => {}
                Some(Err(e)) => return Err(e.into()),
                None => return Ok(()),
            },
            Some(msg) = rx.recv() => { app.apply(msg); dirty = true; }
            _ = tick.tick() => { if app.tick(Instant::now()) { dirty = true; } }
        }
    }
}

fn run_effect(
    effect: Effect,
    app: &mut App,
    terminal: &mut DefaultTerminal,
    clipboard: &mut Clipboard,
    ctx: &Context,
    tx: &UnboundedSender<BgMsg>,
    lock: &CacheLock,
) -> Result<()> {
    match effect {
        Effect::Sync { full } => {
            let mode = if full { SyncMode::Full } else { SyncMode::Auto };
            spawn_sync(ctx, mode, tx.clone(), Arc::clone(lock));
        }
        Effect::Open(url) => {
            let result = if url.starts_with("https://github.com/") {
                open::that_detached(&url).map_err(|e| e.to_string())
            } else {
                Err(format!("refusing to open non-GitHub URL {url}"))
            };
            app.apply(BgMsg::Opened(result));
        }
        Effect::OpenExternal(url) => {
            // Awesome entries link anywhere, but only absolute http(s) URLs
            // of visible ASCII reach the opener (no file:, javascript:, or
            // spoofing characters); the parser enforced this too.
            let result = match awesome::clean_url(&url) {
                Some(url) => open::that_detached(&url).map_err(|e| e.to_string()),
                None => Err("refusing to open a non-http(s) URL".to_string()),
            };
            app.apply(BgMsg::Opened(result));
        }
        Effect::OpenAwesomeList { selection, force } => {
            spawn_awesome_list(ctx, selection, force, tx.clone());
        }
        Effect::Copy(text) => {
            let result = clipboard.copy(&text).map(|_| text);
            app.apply(BgMsg::Copied(result));
        }
        Effect::FetchReadme { id, full_name } => {
            let client = Arc::clone(&ctx.client);
            let tx = tx.clone();
            tokio::spawn(async move {
                let result = client
                    .fetch_readme(&full_name)
                    .await
                    .map_err(|e| e.to_string());
                let _ = tx.send(BgMsg::Readme { id, result });
            });
        }
        Effect::Unstar { id, full_name } => {
            app.set_status(Level::Info, format!("Unstarring {full_name}..."));
            let client = Arc::clone(&ctx.client);
            let path = ctx.cache_path.clone();
            let user = ctx.user.clone();
            let tx = tx.clone();
            let lock = Arc::clone(lock);
            tokio::spawn(async move {
                // DELETE /user/starred acts on the *token owner's* stars, so
                // refuse when browsing someone else's list.
                let result = match client.viewer_login().await {
                    Ok(me) if same_user(&me, &user) => {
                        client.unstar(&full_name).await.map_err(|e| e.to_string())
                    }
                    Ok(me) => Err(format!(
                        "you are browsing {user}'s stars; your token belongs to {me}"
                    )),
                    Err(e) => Err(e.to_string()),
                };
                let _guard = lock.lock().await;
                if result.is_ok()
                    && let Ok(loaded) = StarCache::load_from(&path)
                {
                    let mut cache = loaded.cache;
                    cache.repos.retain(|r| r.id != id);
                    let _ = cache.save_to(&path);
                }
                let _ = tx.send(BgMsg::Unstarred {
                    id,
                    full_name,
                    result,
                });
            });
        }
        Effect::ClearScreen => terminal.clear()?,
    }
    Ok(())
}

fn spawn_sync(ctx: &Context, mode: SyncMode, tx: UnboundedSender<BgMsg>, lock: CacheLock) {
    let client = Arc::clone(&ctx.client);
    let path = ctx.cache_path.clone();
    let user = ctx.user.clone();
    let stale_days = ctx.stale_days;
    let full = mode == SyncMode::Full;
    let store = Store::beside(&ctx.cache_path);
    tokio::spawn(async move {
        star_sync(&client, &user, &path, mode, stale_days, &tx, &lock).await;
        // The awesome index after the stars, outside the cache lock. A
        // failure keeps the cached index and is only a warning.
        match awesome::sync_index(&client, &store, full).await {
            Ok(c) => {
                let _ = tx.send(BgMsg::AwesomeIndex(Box::new(c.index)));
            }
            Err(e) => {
                let _ = tx.send(BgMsg::SyncWarning(format!(
                    "awesome index not refreshed: {e}"
                )));
            }
        }
    });
}

async fn star_sync(
    client: &GitHubClient,
    user: &str,
    path: &std::path::Path,
    mode: SyncMode,
    stale_days: u32,
    tx: &UnboundedSender<BgMsg>,
    lock: &CacheLock,
) {
    {
        // Held until the result is sent; see the module docs.
        let _guard = lock.lock().await;
        let loaded = match StarCache::load_from(path) {
            Ok(l) => l,
            Err(e) => {
                let _ = tx.send(BgMsg::SyncFailed {
                    error: format!("{e:#}"),
                    partial: None,
                });
                return;
            }
        };
        if let Some(w) = loaded.warning {
            let _ = tx.send(BgMsg::SyncWarning(w));
        }
        let progress_tx = tx.clone();
        let mut progress = move |ev: FetchEvent| {
            let msg = match ev {
                FetchEvent::Progress { fetched, message } if fetched > 0 => {
                    BgMsg::SyncProgress(format!("{message} ({fetched} so far)"))
                }
                FetchEvent::Progress { message, .. } | FetchEvent::Finished(message) => {
                    BgMsg::SyncProgress(message)
                }
                FetchEvent::Warning(w) => BgMsg::SyncWarning(w),
            };
            let _ = progress_tx.send(msg);
        };
        let msg = match sync(
            client,
            user,
            loaded.cache,
            path,
            mode,
            stale_days,
            &mut progress,
        )
        .await
        {
            Ok(cache) => BgMsg::SyncDone(Box::new(cache)),
            Err(f) => BgMsg::SyncFailed {
                error: f.error.to_string(),
                partial: f.partial.map(Box::new),
            },
        };
        let _ = tx.send(msg);
    }
}

/// Show an awesome list: cached rows at once, then a fetch unless the
/// cache is fresh and complete (or no token could complete it).
fn spawn_awesome_list(
    ctx: &Context,
    selection: awesome::Selection,
    force: bool,
    tx: UnboundedSender<BgMsg>,
) {
    let client = Arc::clone(&ctx.client);
    let store = Store::beside(&ctx.cache_path);
    let stars_path = ctx.cache_path.clone();
    let stale_days = ctx.stale_days;
    tokio::spawn(async move {
        let repo = selection.repo.clone();
        let _ = store.save_selection(&selection);
        let cached = store.load_list(&repo);
        if let Some(c) = &cached {
            let fresh = awesome::is_fresh(c.fetched_at, stale_days);
            let done = !force && fresh && (c.meta_complete || !client.has_token());
            let _ = tx.send(BgMsg::AwesomeList {
                repo: repo.clone(),
                rows: awesome::to_rows(c),
                done,
            });
            if done {
                return;
            }
        }
        // Starred repos already carry metadata: no lookup for those.
        let known = StarCache::load_from(&stars_path)
            .map(|l| awesome::known_from_stars(&l.cache.repos))
            .unwrap_or_default();
        let progress_tx = tx.clone();
        let progress_repo = repo.clone();
        let mut progress = move |ev: FetchEvent| {
            let msg = match ev {
                FetchEvent::Progress { message, .. } | FetchEvent::Finished(message) => {
                    BgMsg::AwesomeProgress {
                        repo: progress_repo.clone(),
                        text: message,
                    }
                }
                FetchEvent::Warning(w) => BgMsg::SyncWarning(w),
            };
            let _ = progress_tx.send(msg);
        };
        let msg =
            match awesome::fetch_list(&client, &store, &repo, force, &known, &mut progress).await {
                Ok(list) => BgMsg::AwesomeList {
                    repo,
                    rows: awesome::to_rows(&list),
                    done: true,
                },
                Err(e) => BgMsg::AwesomeFailed {
                    repo,
                    error: e.to_string(),
                },
            };
        let _ = tx.send(msg);
    });
}
