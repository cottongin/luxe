use super::*;

fn write_skill(dir: &Path, slug: &str, front: &str, body: &str) {
    let sd = dir.join(".luxe").join("skills").join(slug);
    std::fs::create_dir_all(&sd).unwrap();
    std::fs::write(sd.join("SKILL.md"), format!("---\n{front}\n---\n\n{body}")).unwrap();
}

#[test]
fn discovers_and_loads_a_project_skill() {
    let tmp = tempfile::tempdir().unwrap();
    // so a real global skills dir doesn't leak in
    let _home = isolate_home(tmp.path());
    write_skill(
        tmp.path(),
        "tidy",
        "name: Tidy\ndescription: Keep diffs small",
        "# Tidy\n\nDo the minimum that works.",
    );

    let found = discover(tmp.path());
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].name, "Tidy");
    assert_eq!(found[0].description, "Keep diffs small");

    let body = load(tmp.path(), "tidy").expect("load by name (case-insensitive)");
    assert!(body.starts_with("# Tidy"), "frontmatter stripped: {body:?}");
    assert!(body.contains("minimum that works"));
    assert!(!body.contains("description:"));
}

/// In a session's worktree the project's skills are found at the project, since a checkout
/// never carries `.luxe/`.
#[test]
fn a_worktree_finds_its_projects_skills() {
    let tmp = tempfile::tempdir().unwrap();
    let _home = isolate_home(&tmp.path().join("home"));
    let project = tmp.path().join("app");
    write_skill(&project, "tidy", "name: tidy\ndescription: Keep diffs small", "Small.");
    let worktree = project.join(".luxe").join("worktrees").join("topic");
    std::fs::create_dir_all(&worktree).unwrap();
    let found = discover(&worktree);
    assert_eq!(found.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["tidy"]);
    assert!(load(&worktree, "tidy").is_some());
}

/// A folded description is read whole, not as the `>` that introduces it.
///
/// Pinned against the real shape rather than an invented one: this is `github/gh-stack`'s own
/// `SKILL.md`, the first widely-published skill Luxe was pointed at, and it exposed the bug
/// immediately. The description is all a session knows about a skill until something opens it,
/// so `>` meant a skill the model had no reason to load.
#[test]
fn a_folded_description_survives_the_frontmatter() {
    let tmp = tempfile::tempdir().unwrap();
    let _home = isolate_home(tmp.path());
    write_skill(
        tmp.path(),
        "gh-stack",
        "name: gh-stack\ndescription: >\n  Manages stacked PRs and splits multi-part work into \
             reviewable branches.\n  Use for stack creation, viewing, edits, push, submit.\n\
             metadata:\n  author: github",
        "# gh-stack\n\nA stack is an ordered chain of branches.",
    );

    let found = discover(tmp.path());
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].name, "gh-stack");
    assert_eq!(
        found[0].description,
        "Manages stacked PRs and splits multi-part work into reviewable branches. Use for \
             stack creation, viewing, edits, push, submit.",
        "folded lines join with spaces, and the block stops at the next key"
    );
}

/// A literal block keeps its line breaks; the chomping indicators are accepted and ignored.
#[test]
fn a_literal_description_keeps_its_lines() {
    assert_eq!(scalar("|", &["  one", "  two", "next: key"]), "one\ntwo");
    assert_eq!(scalar(">-", &["  one", "  two"]), "one two");
    assert_eq!(scalar("|+", &["  one"]), "one");
}

/// A plain scalar is already whole, quotes and all.
#[test]
fn a_plain_description_is_taken_as_written() {
    assert_eq!(scalar("Keep diffs small", &[]), "Keep diffs small");
    assert_eq!(scalar("\"Keep diffs small\"", &[]), "Keep diffs small");
    assert_eq!(scalar("'Keep diffs small'", &[]), "Keep diffs small");
    // A lone apostrophe is not a quote pair, and must not lose a character.
    assert_eq!(scalar("don't", &[]), "don't");
}

/// A skill copied onto a `skills/` that did not exist yet becomes that directory, and a
/// `SKILL.md` directly inside it is named rather than silently skipped.
#[test]
fn a_skill_one_level_too_high_is_reported() {
    let tmp = tempfile::tempdir().unwrap();
    let skills = tmp.path().join(".luxe").join("skills");
    std::fs::create_dir_all(skills.join("references")).unwrap();
    std::fs::write(skills.join("SKILL.md"), "---\nname: gh-stack\n---\nbody").unwrap();
    assert!(discover(tmp.path()).iter().all(|skill| skill.name != "gh-stack"));
    assert!(misplaced(tmp.path()).contains(&skills.join("SKILL.md")));

    write_skill(tmp.path(), "tidy", "name: tidy", "body");
    std::fs::remove_file(skills.join("SKILL.md")).unwrap();
    assert!(!misplaced(tmp.path()).contains(&skills.join("SKILL.md")), "a proper one is fine");
}

/// Points every home lookup at `home`, on every platform: Windows reads `USERPROFILE` for home
/// and `APPDATA` for config, and neither cares what `HOME` says.
///
/// Returns the lock the config and keymap tests share for the same variables; hold it for the
/// test, since the environment is the process's and not the test's.
fn isolate_home(home: &Path) -> std::sync::MutexGuard<'static, ()> {
    let lock =
        crate::config::XDG_CONFIG_HOME_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    std::env::set_var("HOME", home);
    std::env::set_var("USERPROFILE", home);
    std::env::set_var("XDG_CONFIG_HOME", home.join(".config"));
    std::env::remove_var("LUXE_CONFIG_DIR");
    lock
}

fn write_skill_in(skills: &Path, slug: &str, description: &str) -> PathBuf {
    let dir = skills.join(slug);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("SKILL.md");
    std::fs::write(&path, format!("---\nname: {slug}\ndescription: {description}\n---\nbody"))
        .unwrap();
    path
}

/// A skill installed for every agent, by `npx skills` or by hand, is installed for Luxe too —
/// at home and in the repository, which commits `.agents/skills` like any other file.
#[test]
fn the_shared_agents_folders_are_read() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let _home = isolate_home(&home);
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    let package = repo.join("packages").join("app");
    std::fs::create_dir_all(&package).unwrap();
    write_skill_in(&home.join(".agents").join("skills"), "personal", "Mine everywhere");
    write_skill_in(&home.join(".config").join("luxe").join("skills"), "luxe-only", "Mine here");
    write_skill_in(&repo.join(".agents").join("skills"), "team", "The repository's");

    let names: Vec<_> = discover(&package).into_iter().map(|skill| skill.name).collect();
    assert_eq!(
        names,
        ["personal", "luxe-only", "team"],
        "broadest first, and found from below the top"
    );
}

/// The reverse of what Luxe used to do: `load` took the first match, which was the global one.
#[test]
fn a_project_skill_beats_a_personal_one_of_the_same_name() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let _home = isolate_home(&home);
    let project = tmp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    write_skill_in(&home.join(".agents").join("skills"), "review", "Personal");
    write_skill_in(&home.join(".config").join("luxe").join("skills"), "review", "Luxe-wide");
    write_skill_in(&project.join(".agents").join("skills"), "review", "Shared in the repo");
    let winner = write_skill_in(&project.join(".luxe").join("skills"), "review", "This project");

    let found = discover(&project);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].path, winner);
    assert_eq!(load_with_path(&project, "review").map(|(_, path)| path), Some(winner));
}

/// A symlinked install is one skill, not two that happen to agree.
#[cfg(unix)]
#[test]
fn a_symlinked_skill_is_one_skill() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let _home = isolate_home(&home);
    let shared = home.join(".agents").join("skills");
    write_skill_in(&shared, "tidy", "Keep diffs small");
    let luxe = home.join(".config").join("luxe").join("skills");
    std::fs::create_dir_all(&luxe).unwrap();
    std::os::unix::fs::symlink(shared.join("tidy"), luxe.join("tidy")).unwrap();
    let project = tmp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();

    assert_eq!(discover(&project).len(), 1);
}

/// A skill folder above the checkout belongs to whoever owns that directory.
#[test]
fn nothing_above_the_checkout_is_read() {
    let tmp = tempfile::tempdir().unwrap();
    let _home = isolate_home(&tmp.path().join("home"));
    write_skill_in(&tmp.path().join(".agents").join("skills"), "stranger", "Not yours");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(repo.join(".git")).unwrap();

    assert!(discover(&repo).is_empty());
}

#[test]
fn missing_dirs_are_not_an_error() {
    let tmp = tempfile::tempdir().unwrap();
    let _home = isolate_home(tmp.path());
    assert!(discover(tmp.path()).is_empty());
    assert!(load(tmp.path(), "nope").is_none());
}
