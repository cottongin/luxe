//! Skills — discoverable `SKILL.md` playbooks (shared by the prompt builder and the `skill` tool).
//!
//! A skill is a directory containing a `SKILL.md` with YAML-ish frontmatter (`name`, `description`)
//! and a markdown body of instructions. Discovery surfaces the index in the system prompt; the
//! `skill` tool loads a body on demand so heavy instructions only cost context when actually used.
//!
//! # Where skills live
//!
//! luxe's own folders, and the `.agents/skills` folders every other skills-aware agent reads —
//! Codex, OpenCode, Pi and Copilot among them, and where `npx skills` installs — so a skill
//! installed for one of them is already installed here. Broadest first:
//!
//! 1. `~/.agents/skills`
//! 2. `~/.config/luxe/skills`
//! 3. `.agents/skills` from the top of the checkout down to the project
//! 4. `.luxe/skills` in the project
//!
//! Two skills with one name resolve to the later, so a project's skill beats a personal one —
//! the rule every client the Agent Skills guide surveyed agrees on — and the shadowed one is logged.
//! The same folder reached twice, which is what a symlinked install looks like, is one skill.

use std::path::{Path, PathBuf};

/// A discovered skill: its frontmatter identity plus the path to load its body from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillDoc {
    /// Skill name (frontmatter `name`, else the directory name).
    pub name: String,
    /// One-line description (frontmatter `description`).
    pub description: String,
    /// Path to the `SKILL.md` file.
    pub path: PathBuf,
}

/// Discover skills across every skill directory, broadest first, one per name.
///
/// Unreadable or malformed entries are skipped; a missing directory is not an error.
pub fn discover(cwd: &Path) -> Vec<SkillDoc> {
    let mut found = Vec::new();
    for dir in skill_dirs(cwd) {
        discover_in(&dir, &mut found);
    }
    resolve_names(found)
}

/// Keeps the last skill of each name, where the later of two shadows the earlier.
///
/// The survivor takes the earlier one's place in the list, so the index reads in the same order
/// whichever copy won.
fn resolve_names(found: Vec<SkillDoc>) -> Vec<SkillDoc> {
    let mut skills: Vec<SkillDoc> = Vec::with_capacity(found.len());
    for skill in found {
        let Some(earlier) =
            skills.iter_mut().find(|earlier| earlier.name.eq_ignore_ascii_case(&skill.name))
        else {
            skills.push(skill);
            continue;
        };
        if !same_file::is_same_file(&earlier.path, &skill.path).unwrap_or(false) {
            tracing::info!(
                skill = %skill.name,
                used = %skill.path.display(),
                shadowed = %earlier.path.display(),
                "two skills share a name; the more specific one is used",
            );
        }
        *earlier = skill;
    }
    skills
}

/// Load a skill's full `SKILL.md` body (frontmatter stripped) by name, searching the skill dirs.
pub fn load(cwd: &Path, name: &str) -> Option<String> {
    load_with_path(cwd, name).map(|(body, _)| body)
}

/// Load a skill body along with the file it came from.
///
/// Callers that bound the body need the path: telling a reader "the rest is in the file" without
/// saying which file is a dead end.
pub fn load_with_path(cwd: &Path, name: &str) -> Option<(String, PathBuf)> {
    let doc = discover(cwd).into_iter().find(|s| s.name.eq_ignore_ascii_case(name))?;
    let content = std::fs::read_to_string(&doc.path).ok()?;
    let body = crate::instructions::sanitize(strip_frontmatter(&content));
    Some((body.text.trim().to_string(), doc.path))
}

/// The skill search directories, broadest first. See the module docs for the order.
///
/// A session's worktree reads `.luxe/skills` from the project it belongs to: `.luxe/` ignores
/// itself, so a checkout never carries it. `.agents/skills` is committed like any other file, so
/// the worktree's own copy is the one that matches the code it is working on.
pub fn skill_dirs(cwd: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(home) = crate::user_dirs::home_dir() {
        dirs.push(home.join(".agents").join("skills"));
    }
    if let Some(luxe) = crate::user_dirs::luxe_config_home() {
        dirs.push(luxe.join("skills"));
    }
    dirs.extend(
        crate::project::chain_from_top(cwd)
            .into_iter()
            .map(|dir| dir.join(".agents").join("skills")),
    );
    let project = crate::worktree::project_of(cwd).unwrap_or_else(|| cwd.to_path_buf());
    dirs.push(project.join(".luxe").join("skills"));
    dirs
}

/// Skill directories holding a `SKILL.md` of their own, where a skill folder should be.
///
/// The shape an ordinary `cp -r some-skill ~/.config/luxe/skills/` leaves behind when `skills/`
/// did not exist yet: the copy *becomes* `skills/`, so `SKILL.md` lands one level too high and
/// nothing is indexed. Discovery cannot guess what was meant, but it can say what it found.
pub fn misplaced(cwd: &Path) -> Vec<PathBuf> {
    skill_dirs(cwd)
        .into_iter()
        .map(|dir| dir.join("SKILL.md"))
        .filter(|path| path.is_file())
        .collect()
}

/// Scan one directory for `<subdir>/SKILL.md` files.
fn discover_in(dir: &Path, skills: &mut Vec<SkillDoc>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let skill_md = entry.path().join("SKILL.md");
        if skill_md.is_file() {
            if let Some(doc) = parse(&skill_md) {
                skills.push(doc);
            }
        }
    }
}

/// Parse a `SKILL.md`'s frontmatter into a [`SkillDoc`].
///
/// The description is indexed into the system prompt of every session that discovers the skill, so
/// it goes through the same filter as an instruction file: a skill is a file in a repository, and
/// cloning a repository is not consent to run what is hidden in it.
fn parse(path: &Path) -> Option<SkillDoc> {
    let content = std::fs::read_to_string(path).ok()?;
    let content = crate::instructions::sanitize(&content).text;
    let (mut name, description) = parse_frontmatter(&content);
    if name.is_empty() {
        name = path
            .parent()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "skill".to_string());
    }
    Some(SkillDoc { name, description, path: path.to_path_buf() })
}

/// Pull `name:` and `description:` out of leading `---`-delimited frontmatter.
fn parse_frontmatter(content: &str) -> (String, String) {
    let Some(front) = frontmatter(content) else { return (String::new(), String::new()) };
    let mut name = String::new();
    let mut description = String::new();
    let lines: Vec<&str> = front.lines().collect();
    for (index, line) in lines.iter().enumerate() {
        if let Some(value) = line.strip_prefix("name:") {
            name = scalar(value.trim(), &lines[index + 1..]);
        } else if let Some(value) = line.strip_prefix("description:") {
            description = scalar(value.trim(), &lines[index + 1..]);
        }
    }
    (name, description)
}

/// A frontmatter value, following a YAML block scalar onto the lines beneath it.
///
/// `description: >` and its relatives are how anyone with more than a few words to say writes this
/// — GitHub's own `gh-stack` skill does — and reading only the first line yielded the description
/// `>`. That is not a cosmetic loss: the description is the *whole* of what a session knows about a
/// skill until something loads its body, so a mangled one is a skill the model can never find a
/// reason to open.
///
/// Folded (`>`) joins its lines with spaces, literal (`|`) keeps the newlines; the chomping
/// indicators (`-`, `+`) change trailing whitespace, which nothing here cares about. Anything else
/// is a plain scalar and is already complete.
fn scalar(first: &str, rest: &[&str]) -> String {
    let folded = match first {
        ">" | ">-" | ">+" => true,
        "|" | "|-" | "|+" => false,
        plain => return unquote(plain),
    };
    // The block runs until a line that is not indented — the next key, or the end.
    let block: Vec<&str> = rest
        .iter()
        .take_while(|line| line.trim().is_empty() || line.starts_with([' ', '\t']))
        .map(|line| line.trim())
        .collect();
    let joined = match folded {
        true => block.join(" "),
        false => block.join("\n"),
    };
    joined.trim().to_string()
}

/// Drop the matching quotes YAML allows around a plain scalar.
fn unquote(value: &str) -> String {
    for quote in ['"', '\''] {
        if let Some(inner) = value.strip_prefix(quote).and_then(|v| v.strip_suffix(quote)) {
            return inner.to_string();
        }
    }
    value.to_string()
}

/// The frontmatter block (between the leading `---` fences), if present.
fn frontmatter(content: &str) -> Option<&str> {
    let rest = content.strip_prefix("---\n").or_else(|| content.strip_prefix("---\r\n"))?;
    let end = rest.find("\n---")?;
    Some(&rest[..end])
}

/// The body with any leading frontmatter block removed.
fn strip_frontmatter(content: &str) -> &str {
    match frontmatter(content) {
        Some(front) => {
            // Skip past the closing fence line after the frontmatter.
            let after_front = &content[content.find(front).unwrap_or(0) + front.len()..];
            after_front
                .find("---")
                .and_then(|i| {
                    after_front[i + 3..].strip_prefix('\n').or(Some(&after_front[i + 3..]))
                })
                .unwrap_or(content)
        }
        None => content,
    }
}

#[cfg(test)]
mod tests;
