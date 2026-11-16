# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.2.0] - 2026-11-16

### Fixed

- **Border "gaps" while scrolling.** Segments of the right-hand pane borders disappeared
  after scrolling and never came back. Cause: emoji whose width ratatui and the terminal
  disagree on (the gear U+2699 followed by variation selector U+FE0F, text-default
  pictographs such as U+1F5A5 and U+1F5C4, and zero-width-joiner sequences in repo
  descriptions) shifted the rest of each redrawn row by a column, overwriting the border,
  which ratatui then never repainted. All repo text is now
  sanitized to width-unambiguous graphemes, truncated by display width rather than by
  character count, the three offending category icons were replaced, and `Ctrl+L` / terminal
  resize force a full redraw.
- `r` now performs a full refresh (previously it only printed "not available").
- PgUp/PgDn stop at the ends instead of wrapping around.
- The key-hint bar no longer disappears permanently after the first sync.
- A user name changed between runs no longer merges two accounts' stars into one cache.
- A cache file truncated by a crash no longer blocks every future sync: it is moved to
  `stars.json.bad` and rebuilt. Cache writes are atomic, private (0600) and serialized
  between sync and unstar.
- Sync errors now say why (user not found, bad token, rate limit with reset time), stay
  visible until the next successful sync, and never hang: every request has timeouts, and
  network errors and 5xx responses are retried.
- Warnings no longer print to stderr over the TUI.
- The terminal is restored after a panic, and mouse capture is released.
- Keyword categorization matches whole words: "rust" no longer matches "trust", "cli" no
  longer "client", "nes" no longer "kubernetes", "unity" no longer "community".

### Added

- Details pane (`d`) with metadata (URL, language, dates, topics, flags) and a rendered,
  scrollable README preview.
- "<user>'s repos" sidebar group: starred repos owned by the browsed user (matched
  case-insensitively); they remain in their own category as well.
- **Python** category (Python language, Django/Flask/FastAPI/PyPI topics and keywords).
- Language filter (`l`), hide archived (`a`) and forks (`f`), with active filters shown in
  the title bar.
- Copy URL (`y`) or `git clone` command (`Y`) via the system clipboard, with an OSC 52
  fallback for SSH sessions.
- Unstar (`u`) with confirmation; refused when browsing someone else's stars.
- Help overlay (`?`), half-page keys (`Ctrl+U` / `Ctrl+D`), `o` to open, `Ctrl+L` redraw,
  `Ctrl+C` quits from anywhere.
- Mouse support: wheel scrolling and click to select / open (`--no-mouse` to disable).
- Scrollbar and `n/total` position in the repo pane title.
- Command line (`--user`, `--sort`, `--theme`, `--ascii`, `--refresh`, `--no-mouse`,
  `--config`, `--cache`, `--version`) and subcommands `export` (Markdown, CSV, JSON) and
  `paths`.
- Optional config file (`~/.config/stargazer/config.toml`): user, sort, theme (dark, light,
  high-contrast, mono), color overrides, ASCII mode, mouse, full-resync interval.
- `NO_COLOR` support and a plain-ASCII mode (`--ascii` / `STARGAZER_ASCII=1`).
- GraphQL sync when a token is available (one request per 100 stars, topics included);
  anonymous REST sync uses conditional requests that do not count against the rate limit.
- Periodic full resync (default every 7 days, `stale_days`) that drops unstarred repos.
- Automatic, lossless upgrade of v0.1 caches. The first launch shows a one-time notice with
  how many repos changed category; the cache is rewritten in the new format immediately so
  the notice does not repeat. v0.1 can still read the upgraded file.
- Release binaries for Linux (x86_64, aarch64), macOS (Intel, Apple Silicon) and Windows,
  with SHA-256 checksums.
- CI (fmt, clippy, tests on Linux/macOS/Windows, MSRV, cargo-deny, release-target builds)
  and Dependabot for Cargo and GitHub Actions.
- Test suite: 110 unit and integration tests (mock GitHub server, render regression tests
  for the border bug).

### Changed

- **No compiled-in default user.** v0.1 fell back to `doublegate`. The user now comes from
  `--user`, `STARGAZER_USER`, the config file, the user the existing cache belongs to, or
  the owner of your `gh` token, in that order.
- **Categorization rules expanded**, which moves some repos compared with v0.1 (on one
  645-star account, 178 repos moved):
  - Python no longer counts toward AI / ML; Python repos without AI topics or keywords go to
    the new Python category.
  - New topics such as `cybersecurity`, `pentesting`, `osint`, `mcp`, `llms`, `esp32` and
    `wireguard` were added across categories, so for example Rust security tools tagged
    `cybersecurity` now land in Security rather than Rust.
  - The repo owner is no longer part of keyword matching.
  - C/C++ count less toward Systems / OS on their own (language weight 4 to 2).
  - A few generic substring keywords (`model`, `memory`, `render`, `level`, `format`,
    `index`) were dropped; prefix keywords became whole words (`emulator`, `vulnerability`,
    `automation`).
  - Cached repos are recategorized automatically.
- Paging moves one screen of repos (previously a fixed 15).
- `Tab` / `Shift+Tab` cycle focus through the sidebar, repo list and details pane.
- Category icons: Systems / OS is now a penguin, TUI / GUI a laptop, Databases a floppy
  disk (the old icons caused the border bug). The repo-list cursor is `▸`.
- Token lookup also accepts `GH_TOKEN` after `gh auth token` and `GITHUB_TOKEN`.
- Replaced the archived `fuzzy-matcher` crate with `nucleo-matcher`; the unused direct
  dependency on `thiserror` 1 became a real one on `thiserror` 2.
- Rust edition 2024; minimum supported Rust version 1.88. `Cargo.lock` is now committed.

## [0.1.0] - 2026-03-07

### Added

- Initial release: ratatui TUI for browsing GitHub stars with cached startup, background
  incremental sync, 11 categories plus Other, fuzzy search, four sort modes and
  open-in-browser.
- 2026-10-26: dependency update `thiserror` 1 to 2 (#1).

[Unreleased]: https://github.com/doublegate/StarGazer/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/doublegate/StarGazer/compare/a874e20...v0.2.0
[0.1.0]: https://github.com/doublegate/StarGazer/commit/a874e20
