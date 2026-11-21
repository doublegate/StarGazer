# Contributing

Thanks for helping improve StarGazer.

## Setup

Requires Rust 1.88+ (`rustup` recommended). Optional: [`cargo-deny`](https://github.com/EmbarkStudios/cargo-deny).

```bash
git clone https://github.com/doublegate/StarGazer
cd StarGazer
cargo run -- --cache /tmp/stargazer-dev.json   # keep your real cache out of development runs
```

## Before opening a pull request

CI runs exactly these; please run them locally first:

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-targets --locked
cargo deny check            # licenses, advisories, sources
```

- Add or update tests with every behavior change. Key handling lives in `App`
  (`src/app.rs`) and is tested without a terminal; GitHub interaction is tested against a
  mock server (`tests/github_api.rs`); rendering invariants in `tests/render.rs`.
- Add an entry under `[Unreleased]` in `CHANGELOG.md` for anything user-visible.
- Use [Conventional Commits](https://www.conventionalcommits.org/) (`feat:`, `fix:`,
  `docs:`, `refactor:`, `test:`, `chore:`, `ci:`).

## Architecture

| Module | Role |
|--------|------|
| `src/main.rs` | CLI parsing, config/token/user resolution, subcommands |
| `src/tui.rs` | Terminal lifecycle, async event loop, side effects |
| `src/app.rs` | All UI state and input handling; emits `Effect`s, consumes `BgMsg`s, no I/O |
| `src/ui.rs` | Rendering |
| `src/github.rs` | GitHub client: GraphQL with a token, REST anonymously; typed errors |
| `src/sync.rs` | Full vs incremental sync, categorization, persistence |
| `src/cache.rs` | Repo model and the atomic, versioned on-disk cache |
| `src/categories.rs` | Rule-based categorization (bump `RULES_VERSION` when rules change) |
| `src/awesome.rs` | Awesome index/list parsing (pulldown-cmark), their cache files, and list fetching with star-count reuse |
| `src/search.rs` | Word-level relevance search (match kinds and field weights are documented at the top of the file) |
| `src/text.rs` | Sanitizing and width-correct truncation of untrusted text |
| `build.rs` | Embeds the icon and version info in `stargazer.exe` (Windows builds only) |
| `src/theme.rs`, `src/config.rs`, `src/export.rs`, `src/clipboard.rs` | Themes and glyphs, config file, export, clipboard |

### Terminal width rule

Every string that reaches the screen from outside (repo names, descriptions, topics,
README text, error messages) must pass through `text::sanitize`, and every line must be
truncated with `text::truncate_width`. New icons must be single emoji-presentation code
points (two columns everywhere) or plain text symbols; `src/theme.rs` tests enforce this.
Breaking the rule brings back the "border gap" bug fixed in 0.2.0.

## Images and artwork

`assets/README.md` records how every image was made. When you add or change one, update its
row there in the same pull request. Edit the banner and logo from the original generations in
`assets/source/agy/`, never from the already-edited `assets/banner.png` or the quantized
`assets/logo.png`. Store screenshots losslessly (`optipng`) and artwork palette-quantized
(`pngquant --quality 85-98`), as described there. Files rescued from a temporary directory are
recorded in `docs/SALVAGE_MANIFEST.md`.

## Releases

Bump `version` in `Cargo.toml`, move `[Unreleased]` in `CHANGELOG.md` to the new version,
merge, then push a `vX.Y.Z` tag. `.github/workflows/release.yml` builds the binaries and
publishes the GitHub Release with the changelog section as notes.
