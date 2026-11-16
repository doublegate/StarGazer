//! Display-safe text handling.
//!
//! Terminals and `unicode-width` disagree about the width of several classes of
//! grapheme: text-default emoji promoted by VS16 (`⚙️`), text-default emoji in the
//! pictograph planes (`🖥`), ZWJ sequences (`🧑‍🔧`), skin-tone modifiers, flags and
//! tag sequences. When ratatui believes a cell is one column wide and the terminal
//! draws two (or the reverse), every following cell in that write run lands in the
//! wrong column. The run's trailing space then overwrites the pane border, and
//! because the border cell never changes in ratatui's buffer it is never repainted:
//! a persistent "gap".
//!
//! [`sanitize`] rewrites untrusted text so that every grapheme it emits has a
//! width every common terminal agrees on, and [`truncate_width`] cuts by display
//! width on grapheme boundaries instead of by `char` count.

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Characters that are removed outright: variation selectors, joiners, bidi
/// controls and other invisible format characters.
fn is_invisible_format(c: char) -> bool {
    matches!(
        c,
        '\u{00AD}'                  // soft hyphen
        | '\u{034F}'                // combining grapheme joiner
        | '\u{061C}'                // arabic letter mark
        | '\u{180E}'                // mongolian vowel separator
        | '\u{200B}'..='\u{200F}'   // zero-width space/joiners, LRM/RLM
        | '\u{202A}'..='\u{202E}'   // bidi embeddings and overrides
        | '\u{2060}'..='\u{2064}'   // word joiner, invisible operators
        | '\u{2066}'..='\u{2069}'   // bidi isolates
        | '\u{FE00}'..='\u{FE0F}'   // variation selectors 1-16
        | '\u{FEFF}'                // BOM / zero-width no-break space
        | '\u{1F3FB}'..='\u{1F3FF}' // emoji skin-tone modifiers
        | '\u{E0000}'..='\u{E007F}' // tag characters (subdivision flags)
        | '\u{E0100}'..='\u{E01EF}' // variation selectors supplement
    )
}

/// Pictographs whose rendered width is unreliable across terminals: regional
/// indicators (flags) and text-default symbols in the supplementary pictograph
/// planes, which `unicode-width` reports as one column but many terminals draw
/// as a two-column colour emoji.
fn is_unreliable_pictograph(c: char) -> bool {
    let cp = c as u32;
    if (0x1F1E6..=0x1F1FF).contains(&cp) {
        return true;
    }
    (0x1F000..=0x1FAFF).contains(&cp) && c.width() == Some(1)
}

/// Make untrusted text safe to place in a terminal cell grid.
///
/// - Control characters (including newlines and tabs) become a single space.
/// - Invisible format characters (VS15/VS16, ZWJ, bidi controls, skin tones,
///   tags) are removed.
/// - Pictographs with terminal-dependent widths are removed.
/// - What remains of a multi-codepoint emoji cluster collapses to its first
///   pictograph, so a ZWJ family becomes a single two-column emoji.
/// - Runs of whitespace collapse to one space and the result is trimmed.
pub fn sanitize(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut last_space = true; // suppresses leading whitespace

    for grapheme in input.graphemes(true) {
        let kept: Vec<char> = grapheme
            .chars()
            .filter(|&c| !is_invisible_format(c))
            .map(|c| if c.is_control() { ' ' } else { c })
            .filter(|&c| !is_unreliable_pictograph(c))
            .collect();

        let Some(&first) = kept.first() else {
            continue;
        };

        if first.is_whitespace() {
            if !last_space {
                out.push(' ');
                last_space = true;
            }
            continue;
        }

        // A wide first char followed by more non-combining codepoints is the
        // remains of an emoji sequence: keep only the leading pictograph.
        // Letters with combining marks (width 0) are preserved intact.
        let collapse =
            first.width() == Some(2) && kept[1..].iter().any(|c| c.width().unwrap_or(0) > 0);
        if collapse {
            out.push(first);
        } else {
            for c in kept {
                if !c.is_whitespace() {
                    out.push(c);
                }
            }
        }
        last_space = false;
    }

    while out.ends_with(' ') {
        out.pop();
    }
    out
}

/// Display width of `s` in terminal columns.
pub fn width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

/// Truncate `s` to at most `max` display columns on grapheme boundaries,
/// appending `ellipsis` when anything was cut. The ellipsis counts toward `max`.
pub fn truncate_width(s: &str, max: usize, ellipsis: &str) -> String {
    if width(s) <= max {
        return s.to_string();
    }
    let ell_w = width(ellipsis);
    if max < ell_w {
        // Not even the ellipsis fits: hard cut.
        return take_width(s, max);
    }
    let mut out = take_width(s, max - ell_w);
    out.push_str(ellipsis);
    out
}

/// Longest grapheme-aligned prefix of `s` that fits in `max` columns.
fn take_width(s: &str, max: usize) -> String {
    let mut out = String::new();
    let mut used = 0;
    for g in s.graphemes(true) {
        let w = width(g);
        if used + w > max {
            break;
        }
        used += w;
        out.push_str(g);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_is_unchanged() {
        assert_eq!(sanitize("hello, world"), "hello, world");
    }

    #[test]
    fn strips_vs16_from_text_default_emoji() {
        // U+2699 GEAR + VS16: ratatui counts 2, many terminals draw 1.
        let s = sanitize("\u{2699}\u{FE0F} Systems");
        assert_eq!(s, "\u{2699} Systems");
        assert_eq!(width(&s), 9);
    }

    #[test]
    fn collapses_zwj_sequence_to_first_pictograph() {
        // 🧑‍🔧 (person + ZWJ + wrench)
        let s = sanitize("\u{1F9D1}\u{200D}\u{1F527} Build");
        assert_eq!(s, "\u{1F9D1} Build");
        assert_eq!(width(&s), 8);
    }

    #[test]
    fn strips_skin_tone_modifiers() {
        assert_eq!(sanitize("\u{1F44D}\u{1F3FD}ok"), "\u{1F44D}ok");
    }

    #[test]
    fn removes_text_default_supplementary_pictographs() {
        // 🖥 U+1F5A5 and 🗄 U+1F5C4 are width-1 per unicode-width, 2 in many terminals.
        assert_eq!(sanitize("a\u{1F5A5}b\u{1F5C4}c"), "abc");
    }

    #[test]
    fn removes_flags() {
        assert_eq!(sanitize("hi \u{1F1FA}\u{1F1F8} there"), "hi there");
    }

    #[test]
    fn keeps_emoji_presentation_pictographs() {
        assert_eq!(sanitize("\u{1F4DD} notes"), "\u{1F4DD} notes");
    }

    #[test]
    fn controls_become_single_spaces() {
        assert_eq!(
            sanitize("line one\n\n\tline\x1b two\r"),
            "line one line two"
        );
    }

    #[test]
    fn strips_bidi_overrides() {
        assert_eq!(sanitize("abc\u{202E}def\u{2066}"), "abcdef");
    }

    #[test]
    fn preserves_combining_marks_and_cjk() {
        let decomposed = "cafe\u{0301}";
        assert_eq!(sanitize(decomposed), decomposed);
        assert_eq!(sanitize("日本語"), "日本語");
    }

    #[test]
    fn keycap_sequence_keeps_base_digit() {
        // "1️⃣" = '1' + VS16 + U+20E3 (combining enclosing keycap, width 0)
        assert_eq!(sanitize("1\u{FE0F}\u{20E3}"), "1\u{20E3}");
    }

    #[test]
    fn every_grapheme_after_sanitize_has_consistent_width() {
        let nasty = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467} \u{2764}\u{FE0F} \u{1F3F3}\u{FE0F}\u{200D}\u{1F308} x";
        let s = sanitize(nasty);
        for g in s.graphemes(true) {
            assert!(
                g.chars().count() == 1 || g.chars().skip(1).all(|c| c.width() == Some(0)),
                "multi-codepoint grapheme survived: {g:?}"
            );
        }
    }

    #[test]
    fn truncate_short_string_is_identity() {
        assert_eq!(truncate_width("abc", 5, "…"), "abc");
        assert_eq!(truncate_width("abc", 3, "…"), "abc");
    }

    #[test]
    fn truncate_adds_ellipsis_within_budget() {
        let t = truncate_width("abcdefgh", 5, "…");
        assert_eq!(t, "abcd…");
        assert_eq!(width(&t), 5);
    }

    #[test]
    fn truncate_never_splits_wide_chars() {
        // Each CJK char is 2 columns; budget 6 with a 1-col ellipsis leaves 5,
        // which fits only two whole chars.
        let t = truncate_width("日本語テキスト", 6, "…");
        assert_eq!(t, "日本…");
        assert!(width(&t) <= 6);
    }

    #[test]
    fn truncate_ascii_ellipsis_and_tiny_budgets() {
        assert_eq!(truncate_width("abcdef", 5, "..."), "ab...");
        assert_eq!(truncate_width("abcdef", 2, "..."), "ab");
        assert_eq!(truncate_width("abcdef", 0, "…"), "");
    }
}
