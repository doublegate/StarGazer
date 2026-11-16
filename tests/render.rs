//! Rendering regression tests for the v0.1 "border gap" bug.
//!
//! The gaps came from graphemes whose width ratatui and the terminal disagree
//! on (VS16 emoji, text-default pictographs, ZWJ sequences). A `TestBackend`
//! uses ratatui's own width model, so it cannot reproduce a *terminal's*
//! disagreement directly; instead these tests pin the invariant that prevents
//! it: after rendering and scrolling through hostile data, every cell holds a
//! width-unambiguous grapheme, and the pane borders are intact on every row.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{Terminal, backend::TestBackend, buffer::Buffer};
use stargazer::app::{App, SortMode};
use stargazer::cache::{StarCache, StarredRepo};
use stargazer::config::ThemeName;
use stargazer::text::sanitize;
use stargazer::theme::{Glyphs, Theme};
use stargazer::ui::Ui;

const NASTY: &[&str] = &[
    "\u{1F4DD} A curated list of awesome Raspberry Pi tools, projects, images and resources",
    "\u{1F468}\u{200D}\u{1F527} Build a Minecraft Server using a Raspberry Pi 4! Supports common Servers, SCP, RCON and more",
    "\u{2699}\u{FE0F} Systems thing with a gear and \u{1F5A5} desktop and \u{1F5C4} cabinet",
    "The USRP\u{2122} Hardware Driver Repository",
    "Flags \u{1F1FA}\u{1F1F8}\u{1F1EF}\u{1F1F5} and skin tones \u{1F44D}\u{1F3FD} and family \u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}",
    "CJK 日本語のテキスト mixed with emoji \u{1F680} and \u{2764}\u{FE0F}",
    "bidi \u{202E}override\u{202C} and zero\u{200B}width and tabs\tand\nnewlines",
    "An interface between an Arduino Due or Arduino GIGA and 8088, 8086, V20, V30, 186 or 286 CPUs, very long",
];

fn fixture() -> StarCache {
    let cats = ["Hardware / FPGA", "Systems / OS", "Rust", "Other"];
    let repos = (0..40u64)
        .map(|i| StarredRepo {
            id: i,
            full_name: format!("owner{i}/repo-\u{2728}-{i}"),
            name: format!("repo-{i}"),
            description: Some(NASTY[i as usize % NASTY.len()].to_string()),
            html_url: format!("https://github.com/owner{i}/repo-{i}"),
            stargazers_count: 1000 - i,
            language: Some(if i % 2 == 0 { "Rust" } else { "C++" }.into()),
            topics: vec!["t\u{FE0F}opic".into()],
            category: cats[i as usize % cats.len()].into(),
            archived: i % 7 == 0,
            fork: i % 5 == 0,
            ..StarredRepo::test_default()
        })
        .collect();
    StarCache {
        username: "octocat".into(),
        repos,
        ..Default::default()
    }
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

/// Every cell symbol is already sanitized (single width-stable grapheme).
fn assert_cells_safe(buf: &Buffer, ctx: &str) {
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            let sym = buf[(x, y)].symbol();
            assert_eq!(
                sanitize(sym).trim(),
                sym.trim(),
                "{ctx}: unsafe grapheme {sym:?} at ({x},{y})"
            );
        }
    }
}

/// Left and right border columns of each pane are border glyphs on every
/// inner row (the scrollbar may occupy the right border of the repo pane).
fn assert_borders_intact(buf: &Buffer, cols: &[u16], top: u16, bottom: u16, ctx: &str) {
    const OK: &[&str] = &["│", "┃", "|", "#", "║"];
    for &x in cols {
        for y in top..bottom {
            let sym = buf[(x, y)].symbol();
            assert!(
                OK.contains(&sym),
                "{ctx}: border broken at ({x},{y}): {sym:?}"
            );
        }
    }
}

fn run_case(ascii: bool, width: u16, height: u16, detail: bool) {
    let ui = Ui::new(Theme::preset(ThemeName::Dark), Glyphs::new(ascii));
    let mut app = App::new("octocat", fixture(), SortMode::Stars, true);
    app.ascii = ascii;
    if detail {
        app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
    }
    let mut term = Terminal::new(TestBackend::new(width, height)).unwrap();

    // Scroll through the repo list, then through every category.
    let mut steps: Vec<KeyEvent> = Vec::new();
    steps.extend(std::iter::repeat_n(key(KeyCode::Down), 45));
    steps.push(key(KeyCode::PageUp));
    steps.push(key(KeyCode::Left));
    steps.extend(std::iter::repeat_n(key(KeyCode::Down), 6));

    for (i, k) in std::iter::once(None)
        .chain(steps.into_iter().map(Some))
        .enumerate()
    {
        if let Some(k) = k {
            app.handle_key(k);
        }
        term.draw(|f| ui.render(f, &mut app)).unwrap();
        let buf = term.backend().buffer().clone();
        let ctx = format!("ascii={ascii} {width}x{height} detail={detail} step {i}");
        assert_cells_safe(&buf, &ctx);

        let cats = app.regions.categories;
        let repos = app.regions.repos;
        let mut cols = vec![repos.x, repos.right() - 1];
        if cats.width > 0 {
            cols.extend([cats.x, cats.right() - 1]);
        }
        assert_borders_intact(&buf, &cols, repos.y + 1, repos.bottom() - 1, &ctx);

        // Bottom edges are pure border; top edges carry the title, so check
        // the corners there.
        let mut panes = vec![repos];
        if cats.width > 0 {
            panes.push(cats);
        }
        if let Some(d) = app.regions.detail {
            panes.push(d);
        }
        for p in panes {
            let bottom = p.bottom() - 1;
            assert_eq!(buf[(p.x, bottom)].symbol(), "╰", "{ctx}: {p:?}");
            assert_eq!(buf[(p.right() - 1, bottom)].symbol(), "╯", "{ctx}: {p:?}");
            for x in p.x + 1..p.right() - 1 {
                assert_eq!(
                    buf[(x, bottom)].symbol(),
                    "─",
                    "{ctx}: bottom edge at ({x},{bottom})"
                );
            }
            assert_eq!(buf[(p.x, p.y)].symbol(), "╭", "{ctx}: {p:?}");
            assert_eq!(buf[(p.right() - 1, p.y)].symbol(), "╮", "{ctx}: {p:?}");
        }
    }
}

#[test]
fn borders_survive_scrolling_unicode() {
    run_case(false, 120, 30, false);
}

#[test]
fn borders_survive_scrolling_ascii() {
    run_case(true, 120, 30, false);
}

#[test]
fn borders_survive_narrow_terminal() {
    run_case(false, 60, 20, false);
}

#[test]
fn borders_survive_with_detail_pane() {
    run_case(false, 160, 40, true);
    run_case(false, 100, 40, true);
}

#[test]
fn ascii_mode_renders_only_ascii() {
    let ui = Ui::new(Theme::preset(ThemeName::Mono), Glyphs::ASCII);
    let mut cache = fixture();
    for r in &mut cache.repos {
        r.full_name = format!("o/r{}", r.id);
        r.description = Some("plain".into());
        r.topics.clear();
    }
    let mut app = App::new("octocat", cache, SortMode::Stars, true);
    app.ascii = true;
    let mut term = Terminal::new(TestBackend::new(100, 25)).unwrap();
    term.draw(|f| ui.render(f, &mut app)).unwrap();
    let buf = term.backend().buffer();
    let borders = "│─╭╮╰╯┃|#";
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            let s = buf[(x, y)].symbol();
            assert!(
                s.is_ascii() || borders.contains(s),
                "non-ASCII {s:?} at ({x},{y}) in --ascii mode"
            );
        }
    }
}
