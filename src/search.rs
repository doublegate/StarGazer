//! Word-based relevance search.
//!
//! v0.1 and v0.2.0 fuzzy-matched the whole query as one character
//! subsequence against a long haystack (name + description + topics), so
//! almost anything matched: "ratatui" hit 369 of 645 stars because its
//! letters occur, in order, somewhere in most descriptions. This module
//! matches each query *term* against individual *words* instead.
//!
//! For every term, the best match over all words of all fields counts:
//!
//! | match (term vs one word)                  | quality |
//! |-------------------------------------------|---------|
//! | exact word                                | 10      |
//! | word starts with the term (2+ chars)      | 7       |
//! | term inside the word (4+ chars)           | 5       |
//! | typo: 1 edit (5+ chars), 2 edits (8+)     | 4       |
//! | fuzzy: in-order letters within the word   | 2       |
//!
//! multiplied by where the word is:
//!
//! | field                    | weight |
//! |--------------------------|--------|
//! | repo name (title)        | 10     |
//! | owner                    | 6      |
//! | topics and language      | 5      |
//! | description              | 3      |
//!
//! Every term must match somewhere (AND). A repo's score is the sum of its
//! terms' scores plus a bonus when the whole multi-word query appears as a
//! phrase in the name or description. Results sort by score, highest
//! first; equal scores keep the current sort order. Title matches therefore
//! come first, then real words in descriptions, and repos that only match
//! fuzzily come last.

use crate::cache::StarredRepo;

/// Quality of a term-vs-word match.
const EXACT: u32 = 10;
const PREFIX: u32 = 7;
const INFIX: u32 = 5;
const TYPO: u32 = 4;
const FUZZY: u32 = 2;

/// Field weights.
const NAME: u32 = 10;
const OWNER: u32 = 6;
const LABEL: u32 = 5;
const DESC: u32 = 3;

/// Added to exact, prefix and infix matches so that a real word in any
/// field outranks a typo or fuzzy match in any field: weak matches really
/// come last, as documented (agy review of PR #6: a name typo, 10 * 4,
/// used to beat an exact description word, 3 * 10).
const STRONG: u32 = 100;

const PHRASE_IN_NAME: u32 = 40;
const PHRASE_IN_DESC: u32 = 20;

/// Lowercased words of `s`, split on anything that is not alphanumeric,
/// except that `+` and `#` stay inside a word so `C++` and `C#` are not
/// both just `c` (agy review of PR #6); leading ones (`#rust`) are dropped.
pub fn words(s: &str) -> Vec<String> {
    s.split(|c: char| !(c.is_alphanumeric() || c == '+' || c == '#'))
        .map(|w| w.trim_start_matches(['+', '#']))
        .filter(|w| !w.is_empty())
        .map(|w| w.to_lowercase())
        .collect()
}

/// Words joined by single spaces, padded, for phrase checks.
fn phrase_text(ws: &[String]) -> String {
    format!(" {} ", ws.join(" "))
}

/// Pre-tokenized searchable fields of one repo.
#[derive(Debug, Clone, PartialEq)]
pub struct Doc {
    name: Vec<String>,
    owner: Vec<String>,
    labels: Vec<String>,
    desc: Vec<String>,
    name_phrase: String,
    desc_phrase: String,
}

impl Doc {
    pub fn new(r: &StarredRepo) -> Self {
        let (owner, name) = r.full_name.split_once('/').unwrap_or(("", &r.full_name));
        let name = words(name);
        let desc = words(r.description.as_deref().unwrap_or(""));
        let mut labels: Vec<String> = r.topics.iter().flat_map(|t| words(t)).collect();
        if let Some(lang) = &r.language {
            labels.extend(words(lang));
        }
        labels.sort();
        labels.dedup();
        Self {
            name_phrase: phrase_text(&name),
            desc_phrase: phrase_text(&desc),
            owner: words(owner),
            name,
            labels,
            desc,
        }
    }
}

/// A parsed query.
#[derive(Debug, Clone, PartialEq)]
pub struct Query {
    terms: Vec<String>,
    phrase: Option<String>,
}

impl Query {
    pub fn parse(q: &str) -> Self {
        // A repeated word counts once ("rust rust" is "rust").
        let mut terms = words(q);
        let mut seen = std::collections::HashSet::new();
        terms.retain(|t| seen.insert(t.clone()));
        let phrase = (terms.len() > 1).then(|| phrase_text(&terms));
        Self { terms, phrase }
    }

    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }

    /// Relevance of `doc`, or `None` when some term matches nothing.
    pub fn score(&self, doc: &Doc) -> Option<u32> {
        if self.terms.is_empty() {
            return None;
        }
        let mut total = 0;
        for term in &self.terms {
            let best = [
                (NAME, &doc.name),
                (OWNER, &doc.owner),
                (LABEL, &doc.labels),
                (DESC, &doc.desc),
            ]
            .iter()
            .filter_map(|(weight, ws)| {
                best_quality(term, ws).map(|q| weight * q + if q >= INFIX { STRONG } else { 0 })
            })
            .max()?;
            total += best;
        }
        if let Some(p) = &self.phrase {
            if doc.name_phrase.contains(p.as_str()) {
                total += PHRASE_IN_NAME;
            } else if doc.desc_phrase.contains(p.as_str()) {
                total += PHRASE_IN_DESC;
            }
        }
        Some(total)
    }
}

/// Best match quality of `term` against any of `ws`.
fn best_quality(term: &str, ws: &[String]) -> Option<u32> {
    ws.iter().filter_map(|w| quality(term, w)).max()
}

/// Match quality of one term against one word.
pub fn quality(term: &str, word: &str) -> Option<u32> {
    if term == word {
        return Some(EXACT);
    }
    let tl = term.chars().count();
    let wl = word.chars().count();
    // One letter already narrows to words starting with it, so the list
    // does not empty out on the first key typed (agy review of PR #6).
    if tl >= 1 && word.starts_with(term) {
        return Some(PREFIX);
    }
    if tl >= 4 && word.contains(term) {
        return Some(INFIX);
    }
    let max_edits = match tl {
        0..=4 => 0,
        5..=7 => 1,
        _ => 2,
    };
    if max_edits > 0 && wl.abs_diff(tl) <= max_edits && edit_distance(term, word) <= max_edits {
        return Some(TYPO);
    }
    if tl >= 3 && wl <= tl * 2 && is_fuzzy(term, word) {
        return Some(FUZZY);
    }
    None
}

/// In-order subsequence within one word, anchored on the first letter (so
/// "rtui" finds "ratatui" but "nes" never finds "kubernetes").
fn is_fuzzy(term: &str, word: &str) -> bool {
    let mut t = term.chars();
    let mut w = word.chars();
    if t.next() != w.next() {
        return false;
    }
    let mut w = w.peekable();
    'terms: for tc in t {
        for wc in w.by_ref() {
            if wc == tc {
                continue 'terms;
            }
        }
        return false;
    }
    true
}

/// Optimal string alignment distance (Levenshtein plus transpositions).
fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut d = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in d[0].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            d[i][j] = (d[i - 1][j] + 1)
                .min(d[i][j - 1] + 1)
                .min(d[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                d[i][j] = d[i][j].min(d[i - 2][j - 2] + 1);
            }
        }
    }
    d[a.len()][b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(full_name: &str, desc: &str, lang: Option<&str>, topics: &[&str]) -> StarredRepo {
        StarredRepo {
            full_name: full_name.into(),
            description: Some(desc.into()),
            language: lang.map(str::to_string),
            topics: topics.iter().map(|s| s.to_string()).collect(),
            ..StarredRepo::test_default()
        }
    }

    fn score(q: &str, r: &StarredRepo) -> Option<u32> {
        Query::parse(q).score(&Doc::new(r))
    }

    /// Repos ranked for `q`, best first, ties in input order; non-matches dropped.
    fn rank<'a>(q: &str, repos: &'a [StarredRepo]) -> Vec<&'a str> {
        let query = Query::parse(q);
        let mut hits: Vec<(u32, usize)> = repos
            .iter()
            .enumerate()
            .filter_map(|(i, r)| query.score(&Doc::new(r)).map(|s| (s, i)))
            .collect();
        hits.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        hits.into_iter()
            .map(|(_, i)| repos[i].full_name.as_str())
            .collect()
    }

    #[test]
    fn unrelated_long_descriptions_do_not_match() {
        // The v0.2.0 failure: this matched "ratatui" by scattered letters.
        let r = repo(
            "sindresorhus/awesome",
            "Awesome lists about all kinds of interesting topics",
            None,
            &["awesome", "lists", "unicorns"],
        );
        assert_eq!(score("ratatui", &r), None);
        assert_eq!(score("terminal", &r), None);
    }

    #[test]
    fn word_level_rules() {
        assert_eq!(quality("ratatui", "ratatui"), Some(EXACT));
        assert_eq!(quality("rata", "ratatui"), Some(PREFIX));
        assert_eq!(quality("chat", "chatgpt"), Some(PREFIX));
        assert_eq!(quality("diff", "stablediffusion"), Some(INFIX));
        assert_eq!(quality("tui", "ratatui"), None, "infix needs 4+ chars");
        assert_eq!(quality("ratatiu", "ratatui"), Some(TYPO), "transposition");
        assert_eq!(quality("emulater", "emulator"), Some(TYPO));
        assert_eq!(quality("rtui", "ratatui"), Some(FUZZY));
        assert_eq!(quality("nes", "kubernetes"), None, "fuzzy is anchored");
        assert_eq!(quality("rs", "rust"), None, "too loose for 2 chars");
        assert_eq!(quality("abc", "abcdefghijklmn"), Some(PREFIX));
        assert_eq!(quality("acn", "abcdefghijklmn"), None, "fuzzy length bound");
    }

    #[test]
    fn a_real_word_anywhere_beats_a_typo_in_the_name() {
        // agy review of PR #6: "crates" ranked a repo named "crate" (a name
        // typo, 10 * 4 = 40) above one with "crates" in its description (30).
        let repos = [
            repo("o/crate", "something else", None, &[]),
            repo("o/registry", "Browse crates and versions", None, &[]),
        ];
        assert_eq!(rank("crates", &repos), ["o/registry", "o/crate"]);
    }

    #[test]
    fn plus_and_hash_stay_in_words() {
        let repos = [
            repo("o/plain-c", "a tool", Some("C"), &[]),
            repo("o/cpp", "a tool", Some("C++"), &[]),
            repo("o/cs", "a tool", Some("C#"), &[]),
        ];
        assert_eq!(rank("c++", &repos), ["o/cpp"]);
        assert_eq!(rank("c#", &repos), ["o/cs"]);
        assert_eq!(words("#rust C++17, x"), ["rust", "c++17", "x"]);
    }

    #[test]
    fn one_letter_and_repeated_terms() {
        let repos = [
            repo("o/ratatui", "tui", None, &[]),
            repo("o/zed", "editor", None, &[]),
        ];
        assert_eq!(rank("r", &repos), ["o/ratatui"], "one letter is a prefix");
        assert_eq!(score("rust rust", &repos[0]), score("rust", &repos[0]));
        assert!(Query::parse("++ .. --").is_empty(), "punctuation only");
    }

    #[test]
    fn title_beats_description_beats_fuzzy() {
        let repos = [
            repo("x/near-miss", "an rtatui-ish widget", None, &[]), // typo-level only
            repo("x/in-desc", "A widget library built on ratatui", None, &[]),
            repo(
                "ratatui/ratatui",
                "Rust library for terminal UIs",
                Some("Rust"),
                &[],
            ),
            repo("x/rtui", "Something else", None, &[]), // query longer than any word
        ];
        assert_eq!(
            rank("ratatui", &repos),
            ["ratatui/ratatui", "x/in-desc", "x/near-miss"],
            "title, then real description word, weak matches last, non-matches dropped"
        );
        // An abbreviation still finds the title word, ranked below exact hits.
        let abbrev = [
            repo("x/in-desc", "the rtui shortcut", None, &[]),
            repo("ratatui/ratatui", "Rust library", None, &[]),
        ];
        assert_eq!(rank("rtui", &abbrev), ["x/in-desc", "ratatui/ratatui"]);
    }

    #[test]
    fn topics_and_language_rank_between_title_and_description() {
        let repos = [
            repo("a/one", "a terminal emulator", None, &[]),
            repo("a/two", "nothing relevant", None, &["terminal"]),
            repo("a/terminal", "nothing relevant", None, &[]),
        ];
        assert_eq!(rank("terminal", &repos), ["a/terminal", "a/two", "a/one"]);
    }

    #[test]
    fn all_terms_must_match() {
        let r = repo("o/tool", "A rust command line tool", None, &[]);
        assert!(score("rust tool", &r).is_some());
        assert_eq!(score("rust python", &r), None);
    }

    #[test]
    fn phrase_order_breaks_ties() {
        let repos = [
            repo("o/a", "line command for everything", None, &[]),
            repo("o/b", "the command line for everything", None, &[]),
        ];
        assert_eq!(rank("command line", &repos), ["o/b", "o/a"]);
    }

    #[test]
    fn case_and_punctuation_are_ignored() {
        let r = repo("o/x", "Next.JS starter", None, &[]);
        assert!(score("NEXT", &r).is_some());
        assert!(score("next.js", &r).is_some());
    }

    #[test]
    fn owner_and_language_match() {
        let r = repo("doublegate/ProRT-IP", "Port scanner", Some("Rust"), &[]);
        assert!(score("doublegate", &r).is_some());
        assert!(score("rust", &r).is_some());
        let c = repo("o/k", "kernel", Some("C"), &[]);
        assert!(
            score("c", &c).is_some(),
            "single-letter exact language match"
        );
    }

    #[test]
    fn empty_query_matches_nothing() {
        let r = repo("o/x", "anything", None, &[]);
        assert!(Query::parse("  ").is_empty());
        assert_eq!(score("", &r), None);
    }

    #[test]
    fn edit_distance_basics() {
        assert_eq!(edit_distance("kitten", "sitting"), 3);
        assert_eq!(edit_distance("abcd", "abdc"), 1);
        assert_eq!(edit_distance("", "abc"), 3);
    }
}
