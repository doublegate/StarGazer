# ✦ StarGazer

A fast, keyboard-driven TUI for browsing your GitHub starred repositories — organized, searchable, and one keypress from your browser.

![Rust](https://img.shields.io/badge/built_with-Rust-orange?logo=rust)
![License](https://img.shields.io/badge/license-MIT-blue)
![Platform](https://img.shields.io/badge/platform-Linux%20%7C%20macOS-lightgrey)

---

## Features

- **Instant startup** — loads from a local cache; syncs new stars in the background while you browse
- **Smart categorization** — 11 categories automatically assigned from GitHub topics, language, and description keywords
- **Fuzzy search** — `/` searches across repo name, description, and topics simultaneously
- **Multiple sort modes** — by stars, recently active, name A→Z, or when you starred it
- **Open in browser** — `Enter` launches the selected repo in your default browser via `xdg-open`
- **Incremental sync** — after the first import, only newly starred repos are fetched (fast and API-friendly)
- **No config file needed** — works out of the box with your existing `gh` CLI auth

## Categories

| Icon | Category | Detected via |
|------|----------|--------------|
| 🦀 | Rust | topics, language |
| 🤖 | AI / ML | topics, keywords |
| 🔐 | Security | topics, keywords |
| ⚙️ | Systems / OS | topics, language, keywords |
| 🎮 | Games / Emulation | topics, keywords |
| 🔌 | Hardware / FPGA | topics, language |
| 🖥 | TUI / GUI | topics, keywords |
| 🔧 | DevTools | topics, keywords |
| 🌐 | Networking | topics, keywords |
| 🗄 | Databases | topics, language |
| 🌍 | Web / Frontend | topics, language |
| 📦 | Other | fallback |

## Installation

### Prerequisites

- Rust toolchain (stable) — [rustup.rs](https://rustup.rs)
- [`gh` CLI](https://cli.github.com/) authenticated with your GitHub account (recommended for higher API rate limits)

### Build from source

```bash
git clone https://github.com/doublegate/StarGazer
cd StarGazer
cargo build --release
# Install to ~/.local/bin (or anywhere on your PATH)
cp target/release/stargazer ~/.local/bin/
```

### Set your GitHub username

StarGazer reads your username from the `STARGAZER_USER` environment variable. Add it to your shell profile:

```bash
# ~/.bashrc / ~/.zshrc / ~/.config/fish/config.fish
export STARGAZER_USER=your_github_username
```

Or run it inline:

```bash
STARGAZER_USER=your_github_username stargazer
```

> **Default fallback:** The binary ships with `doublegate` as the compiled-in default. Set `STARGAZER_USER` to use your own account.

## Usage

```bash
stargazer
```

On first launch, StarGazer performs a full import of all your starred repositories. This takes a few seconds depending on how many stars you have. Subsequent launches load the cache instantly and sync new stars in the background.

## Keybindings

| Key | Action |
|-----|--------|
| `↑` / `k` | Move up in repo list |
| `↓` / `j` | Move down in repo list |
| `←` / `Tab` | Focus category sidebar |
| `→` / `Shift+Tab` | Focus repo list |
| `Enter` | Open selected repo in browser |
| `/` | Enter search mode |
| `Esc` | Clear search / exit search mode |
| `s` | Cycle sort mode |
| `g` / `Home` | Jump to top |
| `G` / `End` | Jump to bottom |
| `PgUp` / `PgDn` | Scroll 15 items |
| `q` | Quit |

## Sort modes

Cycle through sort modes with `s`:

1. **Stars ↓** — most-starred repos first (default)
2. **Recently Active** — by last push date
3. **Name A→Z** — alphabetical by `owner/repo`
4. **Starred At ↓** — by when you starred it, newest first

## Cache

Stars are cached at:

```
~/.local/share/stargazer/stars.json
```

Delete this file to force a full re-import on next launch.

## GitHub API authentication

StarGazer automatically uses your `gh` CLI token (`gh auth token`) if available. Without auth, GitHub allows 60 API requests/hour — enough for most incremental syncs. With auth, the limit is 5,000/hour.

You can also set a token manually via the `GITHUB_TOKEN` environment variable.

## Dependencies

| Crate | Purpose |
|-------|---------|
| [ratatui](https://github.com/ratatui-org/ratatui) | TUI framework |
| [crossterm](https://github.com/crossterm-rs/crossterm) | Terminal backend |
| [tokio](https://tokio.rs) | Async runtime |
| [reqwest](https://github.com/seanmonstar/reqwest) | GitHub API HTTP client |
| [fuzzy-matcher](https://github.com/lotabout/fuzzy-matcher) | Fuzzy search (skim algorithm) |
| [open](https://github.com/Byron/open-rs) | Open URLs in browser |
| [directories](https://github.com/dirs-dev/directories-rs) | XDG data directory resolution |
| [serde](https://serde.rs) / [serde_json](https://github.com/serde-rs/json) | Serialization |
| [chrono](https://github.com/chronotope/chrono) | Date/time handling |
| [anyhow](https://github.com/dtolnay/anyhow) / [thiserror](https://github.com/dtolnay/thiserror) | Error handling |

## License

MIT — see [LICENSE](LICENSE)
