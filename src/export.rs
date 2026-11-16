//! `stargazer export`: write the cached stars as JSON, CSV or a Markdown
//! "awesome list" grouped by category.

use crate::app::{SortMode, sort_repos};
use crate::cache::StarredRepo;
use crate::categories::CATEGORIES;
use crate::theme::Glyphs;
use anyhow::Result;
use std::io::Write;

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Format {
    Json,
    Csv,
    Md,
}

/// Export `repos` (sanitized, sorted by `sort`) to `out`.
pub fn export(
    repos: &[StarredRepo],
    format: Format,
    sort: SortMode,
    category: Option<&str>,
    glyphs: &Glyphs,
    out: &mut dyn Write,
) -> Result<usize> {
    let mut repos: Vec<StarredRepo> = repos
        .iter()
        .filter(|r| category.is_none_or(|c| r.category.eq_ignore_ascii_case(c)))
        .cloned()
        .map(StarredRepo::sanitized)
        .collect();
    sort_repos(&mut repos, sort);

    match format {
        Format::Json => {
            serde_json::to_writer_pretty(&mut *out, &repos)?;
            writeln!(out)?;
        }
        Format::Csv => {
            let mut w = csv::Writer::from_writer(out);
            w.write_record([
                "full_name",
                "url",
                "stars",
                "language",
                "category",
                "topics",
                "archived",
                "fork",
                "starred_at",
                "pushed_at",
                "description",
            ])?;
            for r in &repos {
                w.write_record([
                    r.full_name.as_str(),
                    r.html_url.as_str(),
                    &r.stargazers_count.to_string(),
                    r.language.as_deref().unwrap_or(""),
                    r.category.as_str(),
                    &r.topics.join(";"),
                    &r.archived.to_string(),
                    &r.fork.to_string(),
                    &r.starred_at.map(|d| d.to_rfc3339()).unwrap_or_default(),
                    &r.pushed_at.map(|d| d.to_rfc3339()).unwrap_or_default(),
                    r.description.as_deref().unwrap_or(""),
                ])?;
            }
            w.flush()?;
        }
        Format::Md => write_markdown(&repos, glyphs, out)?,
    }
    Ok(repos.len())
}

/// Escape characters that would change Markdown structure inside link text
/// or list items.
fn md_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(c, '[' | ']' | '<' | '>' | '*' | '_' | '`' | '|' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

fn write_markdown(repos: &[StarredRepo], glyphs: &Glyphs, out: &mut dyn Write) -> Result<()> {
    writeln!(out, "# Starred repositories\n")?;
    let mut cats: Vec<&str> = CATEGORIES
        .iter()
        .copied()
        .filter(|c| repos.iter().any(|r| r.category == *c))
        .collect();
    for r in repos {
        if !cats.contains(&r.category.as_str()) && !CATEGORIES.contains(&r.category.as_str()) {
            cats.push(&r.category);
        }
    }
    for cat in cats {
        let icon = glyphs.category_icon(cat);
        if glyphs.ascii {
            writeln!(out, "## {cat}\n")?;
        } else {
            writeln!(out, "## {icon} {cat}\n")?;
        }
        for r in repos.iter().filter(|r| r.category == cat) {
            let mut meta = vec![format!("{} {}", glyphs.star, r.stargazers_count)];
            if let Some(l) = &r.language {
                meta.push(md_escape(l));
            }
            if r.archived {
                meta.push("archived".into());
            }
            let desc = r
                .description
                .as_deref()
                .map(|d| format!(" - {}", md_escape(d)))
                .unwrap_or_default();
            writeln!(
                out,
                "- [{}]({}){desc} ({})",
                md_escape(&r.full_name),
                r.html_url,
                meta.join(", ")
            )?;
        }
        writeln!(out)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repos() -> Vec<StarredRepo> {
        vec![
            StarredRepo {
                id: 1,
                full_name: "o/a".into(),
                html_url: "https://github.com/o/a".into(),
                stargazers_count: 5,
                category: "Rust".into(),
                language: Some("Rust".into()),
                description: Some("A [weird] *desc*\u{FE0F}".into()),
                ..StarredRepo::test_default()
            },
            StarredRepo {
                id: 2,
                full_name: "o/b".into(),
                html_url: "https://github.com/o/b".into(),
                stargazers_count: 50,
                category: "Other".into(),
                ..StarredRepo::test_default()
            },
        ]
    }

    fn run(format: Format, cat: Option<&str>) -> String {
        let mut buf = Vec::new();
        export(
            &repos(),
            format,
            SortMode::Stars,
            cat,
            &Glyphs::ASCII,
            &mut buf,
        )
        .unwrap();
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn markdown_groups_and_escapes() {
        let md = run(Format::Md, None);
        assert!(md.contains(
            "## Rust\n\n- [o/a](https://github.com/o/a) - A \\[weird\\] \\*desc\\* (* 5, Rust)"
        ));
        let rust = md.find("## Rust").unwrap();
        let other = md.find("## Other").unwrap();
        assert!(rust < other, "categories follow CATEGORIES order");
    }

    #[test]
    fn csv_has_header_and_rows() {
        let csv = run(Format::Csv, None);
        let lines: Vec<&str> = csv.lines().collect();
        assert!(lines[0].starts_with("full_name,url,stars"));
        assert!(lines[1].starts_with("o/b,"), "sorted by stars");
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn json_round_trips_and_filters() {
        let json = run(Format::Json, Some("rust"));
        let parsed: Vec<StarredRepo> = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].full_name, "o/a");
    }
}
