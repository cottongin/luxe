//! Pull request draft — a description a reviewer can read, from the review that was read.
//!
//! A branch's Guided Review already says what changed and why, chapter by chapter, in the order it
//! makes sense. That is the description a pull request usually lacks, so it becomes the draft: the
//! review's intent, each chapter with its reasons, and `Closes #n` when the session took up an
//! issue. Anyone reading it on GitHub gets the story, with or without Luxe.
//!
//! It is a draft. The reader edits it in a preview before anything is sent, and nothing is added
//! that they did not see: no signature, no marker, nothing about Luxe.

use super::{ChapterKind, GuidedReview};

/// Longest a pull request title runs, in characters, before it is cut.
const TITLE_MAX_CHARS: usize = 72;

/// The issue a pull request closes: its number, and its title when it could be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClosedIssue {
    /// Number within its repository
    pub number: u64,
    /// Title, filtered; `None` when it could not be read
    pub title: Option<String>,
}

/// A pull request's title and body, before the reader edits them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequestDraft {
    /// Title, one line
    pub title: String,
    /// Description, markdown
    pub body: String,
}

/// Drafts a pull request from the review of its branch, closing `issue` when there is one.
///
/// The title is the issue's when there is one — it names the thing being fixed, where the review's
/// title is often the prompt that started the work ("fix it") — else the review's, or the lead
/// chapter's when nothing named the review.
///
/// Generated content — lockfiles and the like — is named in one line rather than given a section:
/// it is part of the change, and not what a reviewer came to read about.
pub fn pull_request_draft(review: &GuidedReview, issue: Option<&ClosedIssue>) -> PullRequestDraft {
    let mut body = String::new();
    if let Some(intent) = review.brief.as_ref().map(|brief| brief.intent.trim()) {
        if !intent.is_empty() {
            body.push_str(intent);
            body.push_str("\n\n");
        }
    }
    let (generated, told): (Vec<_>, Vec<_>) =
        review.chapters.iter().partition(|chapter| chapter.kind == ChapterKind::Generated);
    for chapter in &told {
        body.push_str(&format!("### {}\n\n{}\n", chapter.title.trim(), chapter.rationale.trim()));
        if let Some(impact) =
            chapter.impact.as_deref().map(str::trim).filter(|impact| !impact.is_empty())
        {
            body.push_str(&format!("\n{impact}\n"));
        }
        body.push('\n');
    }
    if !generated.is_empty() {
        let names: Vec<&str> = generated.iter().map(|chapter| chapter.title.trim()).collect();
        body.push_str(&format!("Also regenerated: {}.\n\n", names.join(", ")));
    }
    if let Some(issue) = issue {
        body.push_str(&format!("Closes #{}\n", issue.number));
    }
    // A review nothing named is called "Guided Review", which names the tool, not the change: the
    // lead chapter's title says what it does
    let lead = told.first().map(|chapter| chapter.title.as_str());
    let title = match (issue.and_then(|issue| issue.title.as_deref()), lead) {
        (Some(named), _) if !named.trim().is_empty() => title(named),
        (_, Some(lead)) if review.context.title.trim() == super::UNTITLED_REVIEW => title(lead),
        _ => title(&review.context.title),
    };
    PullRequestDraft { title, body: body.trim_end().to_string() }
}

/// One pull request of a stack drafted from a review: the chapters it carries, and its words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StackLayerDraft {
    /// Its chapters, in review order
    pub chapters: Vec<super::ChapterId>,
    /// Title, the first of its chapters' that is not generated content
    pub title: String,
    /// Description: each chapter with its reasons, as a single pull request's would be
    pub body: String,
}

/// Drafts a review as a stack, one pull request per chapter in the order the review told them —
/// which is the order they make sense in — the top one closing `issue`.
///
/// Generated content is not a layer of its own, since nobody reviews a lockfile for its own sake:
/// it rides with the chapter before it, or the first one after when it leads. `None` below two
/// layers, which is one pull request, not a stack.
pub fn stack_draft(
    review: &GuidedReview,
    issue: Option<&ClosedIssue>,
) -> Option<Vec<StackLayerDraft>> {
    let mut groups: Vec<Vec<&super::ReviewChapter>> = Vec::new();
    let mut leading = Vec::new();
    for chapter in &review.chapters {
        if chapter.kind == ChapterKind::Generated {
            match groups.last_mut() {
                Some(group) => group.push(chapter),
                None => leading.push(chapter),
            }
        } else {
            let mut group = std::mem::take(&mut leading);
            group.push(chapter);
            groups.push(group);
        }
    }
    if groups.len() < 2 {
        return None;
    }
    let count = groups.len();
    let layers = groups
        .into_iter()
        .enumerate()
        .map(|(index, group)| {
            let told = group.iter().find(|chapter| chapter.kind != ChapterKind::Generated);
            let mut body = String::new();
            for chapter in &group {
                if chapter.kind == ChapterKind::Generated {
                    body.push_str(&format!("Also regenerated: {}.\n\n", chapter.title.trim()));
                    continue;
                }
                body.push_str(&format!("{}\n", chapter.rationale.trim()));
                if let Some(impact) =
                    chapter.impact.as_deref().map(str::trim).filter(|impact| !impact.is_empty())
                {
                    body.push_str(&format!("\n{impact}\n"));
                }
                body.push('\n');
            }
            body.push_str(&format!("Part {} of {count}.\n", index + 1));
            if index + 1 == count {
                if let Some(issue) = issue {
                    body.push_str(&format!("\nCloses #{}\n", issue.number));
                }
            }
            StackLayerDraft {
                chapters: group.iter().map(|chapter| chapter.id.clone()).collect(),
                title: title(told.map_or("Changes", |chapter| chapter.title.as_str())),
                body: body.trim_end().to_string(),
            }
        })
        .collect();
    Some(layers)
}

fn title(context_title: &str) -> String {
    let line =
        context_title.lines().map(str::trim).find(|line| !line.is_empty()).unwrap_or("Changes");
    if line.chars().count() <= TITLE_MAX_CHARS {
        return line.to_string();
    }
    // A review named for the request that started it carries the whole request, and its first
    // clause is the title: "add a `todo overdue` command: open tasks past…" is "Add a `todo
    // overdue` command". Cut mid-sentence instead, it read as a prompt pasted into a title
    if let Some(clause) = first_clause(line) {
        let mut letters = clause.chars();
        let first = letters.next().map(|c| c.to_uppercase().collect::<String>());
        return format!("{}{}", first.unwrap_or_default(), letters.as_str());
    }
    let kept: String = line.chars().take(TITLE_MAX_CHARS - 1).collect();
    format!("{}…", kept.trim_end())
}

/// The line up to its first `: `, `. ` or `; `, when that is a few words and fits a title.
fn first_clause(line: &str) -> Option<&str> {
    let at = [": ", ". ", "; "].iter().filter_map(|mark| line.find(mark)).min()?;
    let clause = line[..at].trim();
    let words = clause.split_whitespace().count();
    (words >= 3 && clause.chars().count() <= TITLE_MAX_CHARS).then_some(clause)
}

#[cfg(test)]
mod tests;
