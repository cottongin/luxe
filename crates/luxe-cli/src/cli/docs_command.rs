//! `luxe docs` — the user guide, read from inside the binary.
//!
//! The guide ships in the binary, so it works offline and always describes the version installed:
//! `scripts/guide-bundle.sh` writes every page of `docs/guide/` into `src/guide.md`, and this reads
//! that bundle. The agent is told the command exists, so asking luxe how to configure luxe gets an
//! answer from the manual rather than a guess.

use crate::cli::args::DocsArgs;

/// Every page of `docs/guide/`, as `scripts/guide-bundle.sh` wrote them.
const BUNDLE: &str = include_str!("../guide.md");

/// The sidebar's groups, in reading order. `web/src/content.config.ts` holds the same list.
const GROUPS: [&str; 5] = ["Get started", "Using luxe", "Customize", "Extend", "Reference"];

/// The most lines `--search` prints, so a common word cannot flood a tool result.
const SEARCH_LIMIT: usize = 40;

/// One page of the guide.
#[derive(Debug)]
struct Page {
    id: String,
    title: String,
    summary: String,
    group: String,
    order: u32,
    body: String,
}

/// The guide's pages, in reading order: group by group, then by `order`.
fn pages() -> Vec<Page> {
    let mut pages: Vec<Page> = BUNDLE
        .split("\n@@@ page: ")
        .skip(1)
        .filter_map(|chunk| {
            let (id, text) = chunk.split_once('\n')?;
            let rest = text.strip_prefix("---\n")?;
            let (frontmatter, body) = rest.split_once("\n---\n")?;
            let field = |name: &str| {
                frontmatter
                    .lines()
                    .find_map(|line| line.strip_prefix(&format!("{name}:")))
                    .map(|value| value.trim().trim_matches('"').to_string())
                    .unwrap_or_default()
            };
            Some(Page {
                id: id.trim().to_string(),
                title: field("title"),
                summary: field("summary"),
                group: field("group"),
                order: field("order").parse().unwrap_or(u32::MAX),
                body: body.trim().to_string(),
            })
        })
        .collect();
    let rank =
        |page: &Page| GROUPS.iter().position(|group| *group == page.group).unwrap_or(GROUPS.len());
    pages.sort_by_key(|page| (rank(page), page.order));
    pages
}

/// Runs `luxe docs`: the list of pages, one page, or a search across all of them.
pub(crate) fn run(args: &DocsArgs) -> anyhow::Result<()> {
    let pages = pages();
    let output = match (&args.page, &args.search) {
        (_, Some(query)) => search(&pages, query),
        (Some(id), None) => page(&pages, id)?,
        (None, None) => index(&pages),
    };
    crate::output::write(&output);
    Ok(())
}

/// Every page, under its group, with its summary and how to read it.
fn index(pages: &[Page]) -> String {
    let mut out = String::from(
        "luxe's user guide, for this version. `luxe docs <page>` prints a page; \
         `luxe docs --search <words>` finds lines across all of them.\n",
    );
    for group in GROUPS {
        out.push_str(&format!("\n{group}\n"));
        for page in pages.iter().filter(|page| page.group == group) {
            out.push_str(&format!("  {:<18} {}\n", page.id, page.summary));
        }
    }
    out
}

/// One page as markdown: its title, its summary, then its body.
fn page(pages: &[Page], id: &str) -> anyhow::Result<String> {
    let id = id.trim().trim_end_matches(".md");
    let found = pages.iter().find(|page| page.id == id || page.title.eq_ignore_ascii_case(id));
    match found {
        Some(page) => Ok(format!("# {}\n\n> {}\n\n{}\n", page.title, page.summary, page.body)),
        None => {
            let close: Vec<&str> = pages
                .iter()
                .filter(|page| page.id.contains(id) || id.contains(page.id.as_str()))
                .map(|page| page.id.as_str())
                .collect();
            let hint = if close.is_empty() {
                "`luxe docs` lists every page".to_string()
            } else {
                format!("did you mean {}?", close.join(", "))
            };
            anyhow::bail!("no page named `{id}`: {hint}")
        }
    }
}

/// Lines across every page that contain all the words of `query`, case-insensitively.
fn search(pages: &[Page], query: &str) -> String {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    if words.is_empty() {
        return index(pages);
    }
    let mut hits = Vec::new();
    for page in pages {
        for line in page.body.lines() {
            let lower = line.to_lowercase();
            if words.iter().all(|word| lower.contains(word.as_str())) {
                hits.push(format!("{}: {}", page.id, line.trim()));
            }
        }
    }
    let total = hits.len();
    match total {
        0 => format!("Nothing in the guide matches `{query}`. `luxe docs` lists every page."),
        _ => {
            hits.truncate(SEARCH_LIMIT);
            let mut out = hits.join("\n");
            if total > SEARCH_LIMIT {
                out.push_str(&format!(
                    "\n… {} more. Narrow the search, or read a page with `luxe docs <page>`.",
                    total - SEARCH_LIMIT
                ));
            }
            out.push('\n');
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bundle is the guide: the same pages, byte for byte. Run `scripts/guide-bundle.sh` after
    /// editing a page.
    #[test]
    fn the_bundle_matches_the_guide() {
        let guide = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/guide");
        let mut names: Vec<String> = std::fs::read_dir(&guide)
            .expect("docs/guide")
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".md"))
            .collect();
        names.sort();
        let mut expected = String::from(
            "<!-- Generated by scripts/guide-bundle.sh from docs/guide/. Edit the guide, not this file. -->\n",
        );
        for name in names {
            expected.push_str(&format!("@@@ page: {}\n", name.trim_end_matches(".md")));
            expected.push_str(&std::fs::read_to_string(guide.join(&name)).expect("a page"));
        }
        assert!(
            BUNDLE == expected,
            "src/guide.md is out of date with docs/guide/: run scripts/guide-bundle.sh"
        );
    }

    #[test]
    fn every_page_is_read_with_its_frontmatter() {
        let pages = pages();
        assert!(pages.len() > 20, "{} pages", pages.len());
        for page in &pages {
            assert!(!page.title.is_empty() && !page.summary.is_empty(), "{}", page.id);
            assert!(
                GROUPS.contains(&page.group.as_str()),
                "{} has group {:?}",
                page.id,
                page.group
            );
            assert!(!page.body.starts_with("---"), "{} kept its frontmatter", page.id);
        }
        assert_eq!(pages[0].id, "index", "the overview leads");
    }

    #[test]
    fn a_page_is_found_by_id_or_title_and_a_miss_says_what_to_try() {
        let pages = pages();
        assert!(page(&pages, "install").unwrap().starts_with("# Install\n"));
        assert!(page(&pages, "install.md").is_ok());
        assert!(page(&pages, "Git and GitHub").unwrap().contains("worktree"));
        let miss = page(&pages, "hook").unwrap_err().to_string();
        assert!(miss.contains("hooks"), "{miss}");
        assert!(page(&pages, "zzz").unwrap_err().to_string().contains("luxe docs"));
    }

    #[test]
    fn a_search_finds_lines_with_every_word_and_stops_at_the_limit() {
        let pages = pages();
        let hits = search(&pages, "mcp login");
        assert!(hits.lines().any(|line| line.starts_with("mcp: ")), "{hits}");
        assert!(hits
            .lines()
            .all(|line| line.starts_with('…') || line.to_lowercase().contains("login")));
        assert!(search(&pages, "qqqzzz").starts_with("Nothing"));
        let common = search(&pages, "the");
        assert_eq!(common.lines().count(), SEARCH_LIMIT + 1, "capped, with a note");
        assert!(index(&pages).contains("Get started\n  index"));
    }
}
