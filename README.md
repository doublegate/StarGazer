# StarGazer

A fast, keyboard-driven TUI for browsing your GitHub starred repositories: organized,
searchable, and one keypress from your browser.

![Rust](https://img.shields.io/badge/built_with-Rust-orange?logo=rust)
![License](https://img.shields.io/badge/license-MIT-blue)
![Platform](https://img.shields.io/badge/platform-Linux%20%7C%20macOS%20%7C%20Windows-lightgrey)
[![CI](https://github.com/doublegate/StarGazer/actions/workflows/ci.yml/badge.svg)](https://github.com/doublegate/StarGazer/actions/workflows/ci.yml)

---

## Features

- **Instant startup**: loads from a local cache and works offline; syncs new stars in the
  background while you browse
- **Smart categorization**: 13 categories assigned from GitHub topics, language, and
  whole-word description keywords
- **Your own repos**: starred repos you own get their own sidebar group (and stay in their
  category too)
- **Fuzzy search**: `/` searches name, description, topics and language
- **Filters**: by language (`l`), hide archived (`a`) or forks (`f`)
- **Details pane**: `d` shows metadata and a rendered README preview
- **Sort modes**: by stars, recently active, name, or when you starred it
- **Open, copy, unstar**: `Enter` opens in the browser, `y` copies the URL, `u` unstars
  (with confirmation)
- **Incremental sync**: after the first import only new stars are fetched; a periodic full
  sync also drops repos you have unstarred
- **Export**: `stargazer export` writes Markdown (awesome-list style), CSV or JSON
- **Mouse support**, themes, `NO_COLOR`, and a plain-ASCII mode
- **No config file needed**: works out of the box with your `gh` CLI login

## Categories

| ASCII tag | Category | Detected via |
|-----------|----------|--------------|
| `rs` | Rust | topics, language |
| `py` | Python | topics, language |
| `ai` | AI / ML | topics, keywords |
| `sc` | Security | topics, keywords |
| `os` | Systems / OS | topics, language, keywords |
| `gm` | Games / Emulation | topics, keywords |
| `hw` | Hardware / FPGA | topics, language |
| `ui` | TUI / GUI | topics, keywords |
| `dt` | DevTools | topics, language, keywords |
| `nw` | Networking | topics, keywords |
| `db` | Databases | topics, language |
| `wb` | Web / Frontend | topics, language |
| `--` | Other | fallback |

In the default Unicode mode each category has an emoji icon instead of the tag. Every icon
is a single emoji-presentation code point, which every terminal draws two columns wide;
`--ascii` uses the tags above.

Keywords match whole words in the repository *name* and description (the owner is ignored),
so "rust" does not match "trust" and "cli" does not match "client". The highest score wins;
ties go to the category listed first.

## Installation

### Prebuilt binaries

Download the archive for your platform from the
[latest release](https://github.com/doublegate/StarGazer/releases/latest) (Linux x86_64 and
aarch64, macOS Intel and Apple Silicon, Windows x86_64), check it against `SHA256SUMS`, and
put `stargazer` on your `PATH`.

> **macOS:** the binaries are not yet signed or notarized
> ([#3](https://github.com/doublegate/StarGazer/issues/3)), so Gatekeeper blocks a
> browser-downloaded copy with "cannot be verified". After checking the checksum, clear the
> quarantine flag with `xattr -d com.apple.quarantine stargazer`, or build from source
> (below). On Windows, SmartScreen may warn on first run; choose "More info", then
> "Run anyway".

### Build from source

Requires Rust 1.88 or newer ([rustup.rs](https://rustup.rs)).

```bash
git clone https://github.com/doublegate/StarGazer
cd StarGazer
cargo build --release --locked
cp target/release/stargazer ~/.local/bin/
```

### Prerequisites

- [`gh` CLI](https://cli.github.com/) signed in (recommended): higher API rate limits, faster
  GraphQL sync, unstar support, and your login is detected automatically

## Usage

```bash
stargazer                       # your own stars (via gh login, or the cached user)
stargazer --user octocat        # someone else's stars
STARGAZER_USER=octocat stargazer
stargazer --refresh             # full resync now (also drops unstarred repos)
stargazer export -f md -o stars.md
stargazer paths                 # where the config and cache live
```

The GitHub user is resolved in this order: `--user`, `STARGAZER_USER`, `user` in the config
file, the user the existing cache belongs to, and finally the owner of your GitHub token.
Without any of these, StarGazer prints how to set one.

On first launch StarGazer imports all your stars, with progress in the status line.
Subsequent launches show the cache immediately and sync in the background.

### Options

| Flag | Meaning |
|------|---------|
| `-u, --user <LOGIN>` | GitHub user whose stars to browse |
| `--sort <stars\|recent\|name\|starred>` | Initial sort order |
| `--theme <dark\|light\|high-contrast\|mono>` | Color theme |
| `--ascii` | Plain-ASCII icons and glyphs (also `STARGAZER_ASCII=1`) |
| `--refresh` | Full resync at startup |
| `--no-mouse` | Do not capture the mouse (keeps terminal text selection) |
| `--config <PATH>` / `--cache <PATH>` | Override file locations |

### Export

```bash
stargazer export                         # Markdown to stdout, grouped by category
stargazer export -f csv -o stars.csv
stargazer export -f json --category Rust
stargazer export --sync                  # sync first
```

## Keybindings

| Key | Action |
|-----|--------|
| `↑` / `k`, `↓` / `j` | Move in the focused pane (wraps at the ends) |
| `←` / `h`, `→` | Focus sidebar / repo list / details |
| `Tab` / `Shift+Tab` | Cycle focus |
| `PgUp` / `PgDn`, `Ctrl+U` / `Ctrl+D` | Page / half page through the repo list (README when details are focused) |
| `g` / `Home`, `G` / `End` | Jump to the top / bottom of the repo list |
| `Enter` / `o` | Open selected repo in browser |
| `/` | Search (`Enter` opens the selected match, `Esc` clears) |
| `Esc` | Clear search, or close the details pane |
| `d` | Toggle details + README pane |
| `s` | Cycle sort mode |
| `l` | Filter by language |
| `a` / `f` | Hide or show archived repos / forks |
| `y` / `Y` | Copy URL / `git clone` command |
| `r` | Full refresh (also drops unstarred repos) |
| `u` | Unstar the selected repo (asks first; needs a token, own stars only) |
| `?` | Help |
| `Ctrl+L` | Redraw the screen |
| `q` / `Ctrl+C` | Quit |

Mouse: the wheel scrolls the pane under the pointer; clicking selects, clicking the selected
repo again opens it.

## Sort modes

Cycle with `s`:

1. **Stars ↓**: most-starred first (default)
2. **Recently Active**: by last push date
3. **Name A→Z**: alphabetical by `owner/repo`
4. **Starred ↓**: by when you starred it, newest first

Search filters the current sort order rather than re-ranking it.

## Configuration

Optional. Run `stargazer paths` to see the location (`~/.config/stargazer/config.toml` on
Linux). Every key is optional:

```toml
user = "octocat"
sort = "stars"          # stars | recent | name | starred
ascii = false
theme = "dark"          # dark | light | high-contrast | mono
stale_days = 7          # full resync after N days (drops unstarred repos); 0 = never
mouse = true

[colors]                # any ratatui color name or #rrggbb
accent = "cyan"
selected_bg = "#303040"
```

Precedence: command-line flags, then environment variables (`STARGAZER_USER`,
`STARGAZER_ASCII`, `NO_COLOR`), then this file, then defaults.

## Cache

Stars are cached at `~/.local/share/stargazer/stars.json` on Linux (`stargazer paths` shows
the location elsewhere). Writes are atomic and the file is private (mode 0600). A corrupt
cache is moved aside to `stars.json.bad` and rebuilt. Delete the file to force a full
re-import. Caches from v0.1 are read and upgraded automatically.

## GitHub API authentication

StarGazer uses your `gh` CLI token (`gh auth token`) if available, then the `GITHUB_TOKEN`
or `GH_TOKEN` environment variables.

- **With a token** stars are fetched through the GraphQL API (100 per request, topics and
  star dates included) and the limit is 5,000 requests per hour.
- **Without a token** the REST API is used (60 requests per hour). Unchanged star lists cost
  a conditional request that does not count against the limit. README previews also use
  this budget.

## Dependencies

| Crate | Purpose |
|-------|---------|
| [ratatui](https://github.com/ratatui/ratatui) / [crossterm](https://github.com/crossterm-rs/crossterm) | TUI framework and terminal backend |
| [tokio](https://tokio.rs) / [futures](https://github.com/rust-lang/futures-rs) | Async runtime and event stream |
| [reqwest](https://github.com/seanmonstar/reqwest) | GitHub API HTTP client (rustls) |
| [nucleo-matcher](https://github.com/helix-editor/nucleo) | Fuzzy search |
| [unicode-width](https://github.com/unicode-rs/unicode-width) / [unicode-segmentation](https://github.com/unicode-rs/unicode-segmentation) | Display-width-correct text |
| [clap](https://github.com/clap-rs/clap) | Command line |
| [serde](https://serde.rs) / [serde_json](https://github.com/serde-rs/json) / [toml](https://github.com/toml-rs/toml) / [csv](https://github.com/BurntSushi/rust-csv) | Serialization, config, export |
| [open](https://github.com/Byron/open-rs) / [arboard](https://github.com/1Password/arboard) | Browser launching, clipboard |
| [directories](https://github.com/dirs-dev/directories-rs) | XDG / platform directories |
| [chrono](https://github.com/chronotope/chrono) | Date/time handling |
| [anyhow](https://github.com/dtolnay/anyhow) / [thiserror](https://github.com/dtolnay/thiserror) | Error handling |

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Changes are listed in [CHANGELOG.md](CHANGELOG.md).

## License

MIT, see [LICENSE](LICENSE)
