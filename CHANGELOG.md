# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- **Source artwork** (`assets/source/agy/`): the original Antigravity generations behind the
  README banner and the logo (raw banner art, raw emblem, the first titled composite, review
  images), kept so they can be re-edited from the originals; described in `assets/README.md`.
- **Salvage manifest** (`docs/SALVAGE_MANIFEST.md`): a record of files rescued from temporary
  directories into the repository.
- **CONTRIBUTING: images and artwork:** how to add or change images.

### Changed

- CI: `cargo deny` ignores RUSTSEC-2026-0543 (`chrono` is unmaintained; informational, no
  vulnerability) until the migration to `jiff`, tracked in
  [#12](https://github.com/doublegate/StarGazer/issues/12).

## [0.3.1] - 2026-11-16

### Changed

- **The title bar shows the sort of the focused pane.** With the sidebar focused it reads
  `[Categories: Name A→Z]`, otherwise the repo list's sort as before. In v0.3.0 it always
  showed the repo list's sort, even after sorting the sidebar.
- **The Awesome list picker is a collapsible tree.** It opens with only the sections; `Enter`,
  `→` or a click on the selected row expands a section into its lists and a list into its sub-lists, with the list
  itself first (`Python (all)`). `←` collapses or moves to the parent. Filtering with `/` shows
  every match with its path, and clearing it returns to the tree as you left it. It always opens
  with every section collapsed, the cursor on the section that holds the chosen list.
- **The pop-ups look like the rest of the app:** expander markers, counts, the same selection
  and scrollbar as the panes, the filter in the status line like the main search, and the
  pop-up's keys in the hint bar.

### Added

- **Mouse in pop-ups.** In the Awesome picker and the language picker the wheel moves the
  selection, a click selects and a second click activates; the unstar question answers to a
  click on `y unstar` or `n cancel`; a click outside any pop-up closes it.
- **Antigravity PR reviewer** (CI): pull requests are reviewed by a self-hosted reviewer with a
  StarGazer-specific style guide (`.github/agy-review.md`).

### Fixed

- A pop-up's left border could be missing on rows where a two-column icon sat just left of it
  (the sidebar's emoji), leaving half an emoji where the border belongs.
- Awesome lists: "List order" put a nested entry before its parent, and a repository listed
  twice kept the later copy. Entries now follow the README.
- Awesome lists: a transient GitHub error during a star-count lookup was remembered as
  "repository missing" and not retried until a full refresh; only GitHub's not-found or
  forbidden answer counts as missing now, and anything else is retried on the next open.
- `--refresh` now also refreshes the awesome list restored at startup, as `r` does.
- A cache without a recorded full sync (one carried over from v0.1) could postpone the periodic
  full sync forever, because each incremental sync moved the date it was measured from. It now
  gets one full sync, which sets the date.
- Search: a typo or abbreviation in a repo name no longer outranks a real word in its
  description; `C++` and `C#` are searchable as such; one letter already narrows the list to
  words starting with it; repeated words count once; a search of only punctuation no longer
  claims "by relevance".
- README previews: a linked badge (`[![CI](...)](...)`) no longer shows as `![CI](...)`.
- A repository URL from the cache or API is sanitized before it is shown or copied, and only
  plain `https://github.com/` URLs are opened for starred repos.
- README downloads stop at the size cap instead of being read whole, and never end in a broken
  character.
- A REST ETag is no longer kept across GraphQL syncs, where it could later make an anonymous run
  accept a stale "not modified".
- The cached awesome index is sanitized and version-checked when loaded; a list skipped in the
  index no longer hands its sub-lists to the next one.
- Release notes: the CHANGELOG section is matched literally (the version's dots were regex
  wildcards).
- README: the macOS quarantine command takes the binary's path; Windows has its own note.
- `stargazer export` exported the cached account's stars even when `--user`,
  `STARGAZER_USER` or the config asked for another account. It now downloads that account's
  stars into a temporary cache for the export, leaving your own cache (and what a plain
  `stargazer` opens) untouched.

## [0.3.0] - 2026-11-16

### Added

- **Awesome lists.** A new **Awesome** row, pinned between All and your own repos, browses
  the lists indexed by [sindresorhus/awesome](https://github.com/sindresorhus/awesome).
  `Enter` (or a click) on it opens a picker of sections, lists and sub-lists; `/` filters
  it. The chosen list is shown under Awesome as `→ Node.js › Cross-Platform` while Awesome
  is selected, and its entries fill the repo pane with star counts, search, filters,
  sorting, the details pane and README previews, like your stars.
  - Each row shows the section of the list it came from; the starred-date sort reads
    **List order** there and keeps the list's own order.
  - Entries that are not GitHub repositories are included without a star count (`⭐ -`)
    or README; `Enter` opens their link (http and https only).
  - Repos already in your stars are marked `starred`, and only those can be unstarred
    from a list.
  - The index syncs at startup and on every full refresh. A list is fetched when first
    opened and then cached; it is fetched again after `stale_days` or on `r`. The last
    chosen list is restored at startup, from the cache, without waiting for the network.
  - Star counts need a token. GitHub allows roughly 2,000 repository lookups a minute,
    so a big list (awesome-rust, ~1,800 repos) takes 15-20 seconds; repos you have
    starred cost no lookup, and if the limit is reached the counts already fetched are
    kept and the rest load the next time you open the list.
  - Awesome data is stored next to the star cache in `awesome/`, shown by `stargazer paths`.
- **Category sorting.** `s` now sorts whichever pane has focus, and the Categories pane
  has its own sort, independent of the repo list: Default order (the fixed category
  order, and still the default), Stars (most repos first), Recently Active, Name, and
  Starred. All, Awesome and your own repos always stay on top. Set it with
  `category_sort` in `config.toml` or `--category-sort`.

### Changed

- `s` / `S` with the sidebar focused now cycles the category sort; previously it cycled the
  repo-list sort from any pane. With the repo list or details focused it works as before.
- `Enter` on the Awesome row opens the list picker; on every other row it still opens the
  selected repo, as in v0.1.

## [0.2.1] - 2026-11-16

### Changed

- **Search ranks by relevance.** Each query word is matched against the individual words of
  a repo (exact word, word prefix, inside a word for 4+ letters, typos, in-order letters
  within one word), weighted by where it matched: repo name, then owner, then topics and
  language, then description. Every word must match; the best matches come first and weak
  typo or abbreviation matches last; equally relevant repos keep the current sort order.
  The repo pane title shows "by relevance" while searching. With an empty search the list
  is sorted exactly as before.
- This replaces the whole-text fuzzy filter carried over from v0.1, which matched far too
  much: on a 645-star account "ratatui" matched 369 repos (now 1), "terminal" 310 (now 24),
  "rust tui" 455 (now 6). It also reverses the v0.1 rule that search results keep the
  active sort order.

### Added

- `stargazer.exe` now has the StarGazer icon (16 to 256 px) and version information
  embedded, via a Windows-only build step (`build.rs`, `winresource`).

### Removed

- The `nucleo-matcher` dependency (search no longer uses it) and the unused `insta`
  development dependency.

### Documentation

- README: how to verify downloads against `SHA256SUMS` on Linux, macOS and Windows; a
  `cargo install --git` option; an "Upgrading from v0.1" section (automatic cache upgrade,
  removed default user, rollback); and a note on the unsigned macOS and Windows binaries
  with the Gatekeeper workaround. Signing and notarization are tracked in
  [#3](https://github.com/doublegate/StarGazer/issues/3).
- README banner and screenshots of the v0.2.0 TUI (main view, details pane, key reference).
- Project artwork and icons in `assets/`: banner and emblem generated with the Antigravity
  CLI, a matching vector logo (`logo.svg`, plus a small-size variant), PNG icons from 16 to
  1024 px, a multi-size `favicon.ico`, and a GitHub social-preview card. Provenance and
  regeneration steps are in `assets/README.md`.

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

[Unreleased]: https://github.com/doublegate/StarGazer/compare/v0.3.1...HEAD
[0.3.1]: https://github.com/doublegate/StarGazer/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/doublegate/StarGazer/compare/v0.2.1...v0.3.0
[0.2.1]: https://github.com/doublegate/StarGazer/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/doublegate/StarGazer/compare/a874e20...v0.2.0
[0.1.0]: https://github.com/doublegate/StarGazer/commit/a874e20
