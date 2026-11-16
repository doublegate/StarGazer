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

// ── Awesome ─────────────────────────────────────────────────────────────────

/// An app showing a hostile awesome list, with the Awesome row selected.
fn awesome_app(ascii: bool) -> App {
    use stargazer::app::BgMsg;
    use stargazer::awesome::{Selection, parse_index};
    let mut app = App::new("octocat", fixture(), SortMode::Stars, true);
    app.ascii = ascii;
    let rows: Vec<StarredRepo> = fixture()
        .repos
        .into_iter()
        .enumerate()
        .map(|(i, mut r)| {
            r.rank = i as u32;
            r.external = i % 3 == 0;
            r.category = format!("Sec\u{FE0F}tion \u{1F527} › {}", NASTY[i % NASTY.len()]);
            if r.external {
                r.html_url = format!("https://example.com/{i}");
            }
            r
        })
        .collect();
    let index = parse_index(include_str!("fixtures/awesome/index.md"));
    let label = format!("Node.js › {}", NASTY[1]);
    app.set_awesome(
        Some(index),
        Some(Selection {
            repo: "o/awesome-x".into(),
            label,
        }),
        false,
    );
    app.apply(BgMsg::AwesomeList {
        repo: "o/awesome-x".into(),
        rows,
        done: true,
    });
    app.drain_effects();
    app.handle_key(key(KeyCode::Left));
    app.handle_key(key(KeyCode::Down)); // Awesome
    assert!(app.in_awesome());
    app
}

fn check_frame(term: &Terminal<TestBackend>, app: &App, ctx: &str) {
    let buf = term.backend().buffer().clone();
    assert_cells_safe(&buf, ctx);
    let cats = app.regions.categories;
    let repos = app.regions.repos;
    let mut cols = vec![repos.x, repos.right() - 1];
    if cats.width > 0 {
        cols.extend([cats.x, cats.right() - 1]);
    }
    assert_borders_intact(&buf, &cols, repos.y + 1, repos.bottom() - 1, ctx);
}

fn run_awesome_case(ascii: bool, width: u16, height: u16) {
    let ui = Ui::new(Theme::preset(ThemeName::Dark), Glyphs::new(ascii));
    let mut app = awesome_app(ascii);
    let mut term = Terminal::new(TestBackend::new(width, height)).unwrap();
    let ctx = |s: &str| format!("awesome ascii={ascii} {width}x{height} {s}");

    // The expanded line under Awesome, then the rows, then details.
    term.draw(|f| ui.render(f, &mut app)).unwrap();
    check_frame(&term, &app, &ctx("sidebar"));
    app.handle_key(key(KeyCode::Right));
    app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
    for i in 0..45 {
        app.handle_key(key(KeyCode::Down));
        term.draw(|f| ui.render(f, &mut app)).unwrap();
        check_frame(&term, &app, &ctx(&format!("row {i}")));
    }

    // The picker over everything: collapsed, then every expander opened
    // while walking down (so it scrolls), then filtered to nothing.
    app.handle_key(key(KeyCode::Left));
    app.handle_key(key(KeyCode::Enter));
    for i in 0..40 {
        app.handle_key(key(KeyCode::Right)); // expands, or steps in
        app.handle_key(key(KeyCode::Down));
        term.draw(|f| ui.render(f, &mut app)).unwrap();
        let ctx = ctx(&format!("picker {i}"));
        assert_cells_safe(term.backend().buffer(), &ctx);
        let p = app.regions.popup.expect("picker region recorded");
        assert_borders_intact(
            term.backend().buffer(),
            &[p.x, p.right() - 1],
            p.y + 1,
            p.bottom() - 1,
            &ctx,
        );
    }
    app.handle_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
    for c in "\u{1F527}zz".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        term.draw(|f| ui.render(f, &mut app)).unwrap();
        assert_cells_safe(term.backend().buffer(), &ctx("picker filter"));
    }
}

#[test]
fn awesome_view_and_picker_keep_cells_and_borders_safe() {
    run_awesome_case(false, 120, 30);
    run_awesome_case(false, 60, 20);
    run_awesome_case(true, 100, 25);
}

#[test]
fn awesome_ascii_mode_renders_only_ascii() {
    let ui = Ui::new(Theme::preset(ThemeName::Mono), Glyphs::ASCII);
    let mut app = awesome_app(true);
    for r in app.awesome.repos.iter_mut() {
        r.full_name = format!("o/r{}", r.id);
        r.description = Some("plain".into());
        r.category = "Tools > CLI".into();
        r.topics.clear();
    }
    app.awesome.selection.as_mut().unwrap().label = "Node.js".into();
    let mut term = Terminal::new(TestBackend::new(100, 25)).unwrap();
    term.draw(|f| ui.render(f, &mut app)).unwrap();
    let buf = term.backend().buffer();
    let borders = "│─╭╮╰╯┃|#";
    let mut saw_line = false;
    for y in 0..buf.area.height {
        let mut row = String::new();
        for x in 0..buf.area.width {
            let s = buf[(x, y)].symbol();
            row.push_str(s);
            assert!(
                s.is_ascii() || borders.contains(s),
                "non-ASCII {s:?} at ({x},{y}) in --ascii mode"
            );
        }
        saw_line |= row.contains("-> Node.js");
    }
    assert!(saw_line, "the chosen list's line is drawn under Awesome");
}

// ── Pop-up mouse ────────────────────────────────────────────────────────────

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use stargazer::app::Overlay;

fn mouse(kind: MouseEventKind, x: u16, y: u16) -> MouseEvent {
    MouseEvent {
        kind,
        column: x,
        row: y,
        modifiers: KeyModifiers::NONE,
    }
}

fn click(app: &mut App, x: u16, y: u16) {
    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), x, y));
}

/// Screen position of list row `i` (counted from the top of the visible
/// list) inside a bordered pop-up `p`.
fn row_pos(p: Rect, i: u16) -> (u16, u16) {
    (p.x + 4, p.y + 1 + i)
}

fn draw(term: &mut Terminal<TestBackend>, ui: &Ui, app: &mut App) {
    term.draw(|f| ui.render(f, app)).unwrap();
}

#[test]
fn awesome_picker_mouse_selects_activates_scrolls_and_closes() {
    let ui = Ui::new(Theme::preset(ThemeName::Dark), Glyphs::UNICODE);
    let mut app = awesome_app(false);
    let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
    app.handle_key(key(KeyCode::Enter)); // picker, sidebar focused on Awesome
    draw(&mut term, &ui, &mut app);
    let names = |a: &App| {
        a.picker_view()
            .iter()
            .map(|r| r.name.clone())
            .collect::<Vec<_>>()
    };
    let sel = |a: &App| match a.overlay {
        Overlay::AwesomePicker { selected, .. } => selected,
        _ => panic!("picker closed"),
    };
    // set_awesome revealed the chosen list's section (Platforms); collapse
    // everything for a known start.
    let p = app.regions.popup.unwrap();

    // Click a row: selects it. Click it again: toggles it.
    let target = names(&app)
        .iter()
        .position(|n| n == "Programming Languages")
        .unwrap() as u16;
    let (x, y) = row_pos(p, target);
    click(&mut app, x, y);
    assert_eq!(sel(&app), target as usize);
    let before = names(&app).len();
    click(&mut app, x, y);
    draw(&mut term, &ui, &mut app);
    assert!(names(&app).len() > before, "second click expanded it");
    assert!(names(&app).contains(&"JavaScript".to_string()));

    // The wheel moves the selection, clamped at the last row.
    for _ in 0..2 {
        app.handle_mouse(mouse(MouseEventKind::ScrollDown, x, y));
    }
    assert_eq!(sel(&app), target as usize + 2);
    for _ in 0..50 {
        app.handle_mouse(mouse(MouseEventKind::ScrollDown, x, y));
    }
    assert_eq!(sel(&app), names(&app).len() - 1);
    for _ in 0..50 {
        app.handle_mouse(mouse(MouseEventKind::ScrollUp, x, y));
    }
    assert_eq!(sel(&app), 0);

    // Click a list leaf twice: chooses it and closes the picker.
    draw(&mut term, &ui, &mut app);
    let rust = names(&app).iter().position(|n| n == "Rust").unwrap() as u16;
    let (x, y) = row_pos(
        app.regions.popup.unwrap(),
        rust - app.popup_list.offset() as u16,
    );
    click(&mut app, x, y);
    click(&mut app, x, y);
    assert_eq!(app.overlay, Overlay::None);
    assert_eq!(app.awesome.selection.as_ref().unwrap().label, "Rust");

    // Reopen and click outside: closes without choosing anything.
    app.handle_key(key(KeyCode::Left));
    app.handle_key(key(KeyCode::Enter));
    draw(&mut term, &ui, &mut app);
    app.drain_effects();
    click(&mut app, 0, 0);
    assert_eq!(app.overlay, Overlay::None);
    assert!(app.drain_effects().is_empty());
}

#[test]
fn language_picker_and_unstar_confirmation_take_mouse_clicks() {
    let ui = Ui::new(Theme::preset(ThemeName::Dark), Glyphs::UNICODE);
    let mut app = App::new("octocat", fixture(), SortMode::Stars, true);
    let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();

    // Language picker: the second row is the most common language.
    app.handle_key(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE));
    draw(&mut term, &ui, &mut app);
    let (x, y) = row_pos(app.regions.popup.unwrap(), 1);
    click(&mut app, x, y);
    assert!(app.language.is_none(), "first click only selects");
    click(&mut app, x, y);
    assert_eq!(app.overlay, Overlay::None);
    assert!(app.language.is_some());

    // Unstar: clicking "n cancel" cancels; clicking "y unstar" confirms.
    app.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::NONE));
    draw(&mut term, &ui, &mut app);
    let no = app.regions.confirm_no.unwrap();
    click(&mut app, no.x, no.y);
    assert_eq!(app.overlay, Overlay::None);
    assert!(app.drain_effects().is_empty());

    app.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::NONE));
    draw(&mut term, &ui, &mut app);
    let b = app.regions.popup_box.unwrap();
    click(&mut app, b.x + 2, b.y + 1); // the question text: nothing happens
    assert!(matches!(app.overlay, Overlay::ConfirmUnstar { .. }));
    let yes = app.regions.confirm_yes.unwrap();
    assert_eq!(term.backend().buffer()[(yes.x, yes.y)].symbol(), "y");
    click(&mut app, yes.x + 3, yes.y);
    assert!(matches!(
        app.drain_effects().as_slice(),
        [stargazer::app::Effect::Unstar { .. }]
    ));

    // Help closes on any click.
    app.handle_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
    draw(&mut term, &ui, &mut app);
    click(&mut app, 60, 15);
    assert_eq!(app.overlay, Overlay::None);
}

#[test]
fn awesome_picker_ascii_renders_only_ascii() {
    let ui = Ui::new(Theme::preset(ThemeName::Mono), Glyphs::ASCII);
    let mut app = awesome_app(true);
    app.awesome.selection.as_mut().unwrap().label = "Node.js".into();
    app.handle_key(key(KeyCode::Enter));
    app.handle_key(key(KeyCode::Right)); // open the first section
    let mut term = Terminal::new(TestBackend::new(100, 25)).unwrap();
    draw(&mut term, &ui, &mut app);
    let buf = term.backend().buffer();
    let borders = "│─╭╮╰╯┃|#";
    let mut text = String::new();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            let s = buf[(x, y)].symbol();
            text.push_str(s);
            // Fixture list names come from the CC0 index and are ASCII.
            assert!(
                s.is_ascii() || borders.contains(s),
                "non-ASCII {s:?} at ({x},{y})"
            );
        }
    }
    assert!(text.contains("- Platforms"), "open section marker");
    assert!(
        text.contains("+ Programming Languages"),
        "closed section marker"
    );
}
