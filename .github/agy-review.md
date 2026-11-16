# StarGazer Review Style Guide

These are the project rules the Antigravity reviewer enforces on StarGazer pull requests.
StarGazer is a Rust (edition 2024, MSRV 1.88) terminal UI for browsing a GitHub user's starred
repositories and the awesome lists indexed by sindresorhus/awesome. It renders with ratatui and
crossterm, talks to the GitHub GraphQL and REST APIs with reqwest, and keeps a local cache.

Authoritative context:

- `CONTRIBUTING.md`: module map, quality gates, release steps.
- `CHANGELOG.md`: behavior that was changed or deliberately kept. Several v0.1 behaviors are
  intentional (case-insensitive search, Enter in search opens, j/k wrap, uppercase Q/S/R); do not
  flag them as bugs.

## Priorities (in order)

1. Correctness of what the user sees (the right repos, counts, order and account).
2. Security of untrusted input: GitHub API data and awesome-list Markdown are attacker-controlled.
3. The terminal-width invariant (below).
4. Not wasting the user's GitHub rate limit.
5. Tests accompany behavior changes; docs and CHANGELOG change with behavior.

## What to flag as BLOCKING

### Untrusted text and the terminal

- **Unsanitized external text reaching the screen.** Every string from GitHub or an awesome list
  (names, descriptions, topics, headings, labels, list categories) must pass through
  `text::sanitize` before display, and every rendered line must be cut with
  `text::truncate_width` / `fit_spans`. A width disagreement shifts a row into the pane border,
  which is never repainted: the "border gap" bug this project exists to have fixed.
- **Icons and glyphs** that are not single emoji-presentation code points (two columns) or plain
  width-1 symbols. `theme.rs` tests enforce this; new glyphs need to be added to them.
- **Wide glyphs next to overlays.** Anything drawn over another pane must not leave a two-column
  glyph straddling its border (see `Ui::popup`).

### URLs, clipboard and the shell

- A URL opened or copied without validation. Starred repos open only `https://github.com/` URLs
  (`Effect::Open`); awesome entries go through `awesome::clean_url` (absolute http(s), visible
  ASCII only) and `Effect::OpenExternal`.
- A shell command (for example `git clone`) built from URL text instead of a validated
  `owner/repo` (`github::validate_full_name`, `awesome::github_repo`).
- GraphQL queries built by string interpolation of repository data instead of variables.

### Architecture

- I/O inside `App` (`src/app.rs`). `App` only mutates state and queues `Effect`s; `tui.rs` runs
  them and returns `BgMsg`s. This is what makes key handling unit-testable.
- Star-cache writes outside the cache lock in `tui.rs`, or a sync that releases it before its
  result is sent (an unstar could be resurrected).
- Non-atomic cache writes (always `cache::write_atomic`).
- GitHub login comparisons that are case-sensitive (use `sync::same_user`).
- Unstar acting on an account other than the token owner's, or on a repo not in the user's stars.

### GitHub API

- Startup that blocks on the network when a cache exists.
- Unbounded or uncached API use. Repository metadata lookups are limited to roughly 2,000 per
  minute of GraphQL server time; known repositories (star cache, a list's previous fetch,
  confirmed missing) must not be looked up again, and partial results must be kept.
- Treating a transient GraphQL error as "repository missing" (only `NOT_FOUND` / `FORBIDDEN`
  mean missing).

### Panics

- `unwrap`/`expect`/indexing that can panic on API data, cache contents, Markdown, terminal size
  or user input. Tests may unwrap.

## What to keep as SUGGESTION / NITPICK

- Naming, structure and readability; smaller functions.
- Missing tests for non-critical paths.
- Performance ideas without a measurement (profile first; the hot paths are rendering and
  search scoring).

## Conventions

- Conventional Commits (`feat|fix|docs|refactor|test|chore|perf|build|ci`), with a body that says
  why.
- Gates: `cargo fmt --all --check`, `cargo clippy --all-targets --all-features --locked -- -D
  warnings`, `cargo test --all-targets --locked`, `cargo deny check`.
- No emojis in code comments, commits or docs (emoji glyphs in `theme.rs` are UI, not prose).
- Smallest correct change; reuse existing helpers (`text`, `search`, `cache::write_atomic`).
