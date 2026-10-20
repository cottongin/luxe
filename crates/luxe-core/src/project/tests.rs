use super::*;

/// A tree with `.git` at the top and a couple of levels under it.
fn repo() -> tempfile::TempDir {
    let tree = tempfile::tempdir().expect("temp tree");
    std::fs::create_dir_all(tree.path().join(".git")).expect("git marker");
    std::fs::create_dir_all(tree.path().join("crates").join("core").join("src"))
        .expect("nested dirs");
    tree
}

/// The whole point: standing in a subdirectory finds the same project as standing at the top.
#[test]
fn a_subdirectory_of_a_checkout_resolves_to_its_root() {
    let tree = repo();
    let deep = tree.path().join("crates").join("core").join("src");
    assert_eq!(discover_within(&deep, None), tree.path());
    assert_eq!(discover_within(tree.path(), None), tree.path(), "and the root finds itself");
}

/// A linked worktree and a submodule carry a `.git` *file*. Requiring a directory would walk
/// straight past them into whatever encloses the checkout.
#[test]
fn a_worktree_whose_git_is_a_file_is_still_a_root() {
    let tree = tempfile::tempdir().expect("temp tree");
    let linked = tree.path().join("feature");
    std::fs::create_dir_all(linked.join("src")).expect("nested dirs");
    std::fs::write(linked.join(".git"), "gitdir: /elsewhere/.git/worktrees/feature\n")
        .expect("git file");
    assert_eq!(discover_within(&linked.join("src"), None), linked);
}

/// Jujutsu and Mercurial checkouts have a top too, and it is the same question.
#[test]
fn the_other_vcs_markers_are_roots_as_well() {
    for marker in [".jj", ".hg"] {
        let tree = tempfile::tempdir().expect("temp tree");
        std::fs::create_dir_all(tree.path().join(marker)).expect("marker");
        let deep = tree.path().join("src");
        std::fs::create_dir_all(&deep).expect("nested dir");
        assert_eq!(discover_within(&deep, None), tree.path(), "{marker} marks a root");
    }
}

/// The property the whole design turns on. Opencode derives its project identity from Git and
/// loses a project's sessions every time that identity moves — `git init` in a directory with
/// history is one of its longest-running bug reports. Here the directory already answers, so a
/// repository appearing under it changes nothing.
#[test]
fn an_existing_project_outranks_a_repository_that_appears_around_it() {
    let tree = tempfile::tempdir().expect("temp tree");
    let package = tree.path().join("packages").join("app");
    std::fs::create_dir_all(package.join(PROJECT_DIR)).expect("luxe dir");
    std::fs::create_dir_all(package.join("src")).expect("nested dir");
    std::fs::create_dir_all(tree.path().join(".git")).expect("git marker");

    assert_eq!(
        discover_within(&package.join("src"), None),
        package,
        "the scoped project wins over the repository enclosing it"
    );
}

/// With no marker anywhere, the answer is where you stood — not a guess, and not `/`.
#[test]
fn a_directory_with_no_markers_is_its_own_root() {
    let tree = tempfile::tempdir().expect("temp tree");
    let loose = tree.path().join("notes");
    std::fs::create_dir_all(&loose).expect("nested dir");
    assert_eq!(discover_within(&loose, None), loose);
}

/// Dotfiles-in-a-repo at `~` is common enough that inheriting it would be a regular disaster:
/// a scratch directory under home would index the whole home directory.
#[test]
fn a_repository_at_home_is_not_inherited_by_what_sits_under_it() {
    let home = tempfile::tempdir().expect("temp home");
    std::fs::create_dir_all(home.path().join(".git")).expect("dotfiles repo");
    let scratch = home.path().join("Downloads").join("scratch");
    std::fs::create_dir_all(&scratch).expect("nested dirs");

    assert_eq!(discover_within(&scratch, Some(home.path())), scratch, "home is not inherited");
    assert_eq!(
        discover_within(home.path(), Some(home.path())),
        home.path(),
        "but opening home itself still names it"
    );
}

/// The guard stops the walk rather than skipping one directory, or a repository *above* home
/// would be adopted instead — the same failure one level further out.
#[test]
fn the_walk_does_not_continue_above_home() {
    let outer = tempfile::tempdir().expect("temp tree");
    std::fs::create_dir_all(outer.path().join(".git")).expect("outer repo");
    let home = outer.path().join("home").join("dev");
    let scratch = home.join("scratch");
    std::fs::create_dir_all(&scratch).expect("nested dirs");

    assert_eq!(discover_within(&scratch, Some(&home)), scratch);
}

/// A repository under home is an ordinary project; the guard is about home itself.
#[test]
fn a_repository_under_home_is_found_normally() {
    let home = tempfile::tempdir().expect("temp home");
    let project = home.path().join("code").join("thing");
    std::fs::create_dir_all(project.join(".git")).expect("git marker");
    std::fs::create_dir_all(project.join("src")).expect("nested dir");

    assert_eq!(discover_within(&project.join("src"), Some(home.path())), project);
}

/// A `.luxe` *file* is not a project directory, and treating it as one would adopt a root on
/// the strength of something nobody meant as a marker.
#[test]
fn only_a_project_directory_counts_as_one() {
    let tree = tempfile::tempdir().expect("temp tree");
    std::fs::create_dir_all(tree.path().join(".git")).expect("git marker");
    let inner = tree.path().join("inner");
    std::fs::create_dir_all(&inner).expect("nested dir");
    std::fs::write(inner.join(PROJECT_DIR), "not a directory").expect("decoy");

    assert_eq!(discover_within(&inner, None), tree.path());
}

// === a `.luxe/` claims nothing outside its checkout ===

/// The reported case: luxe launched once in `~/.config` left a `.luxe/` there, and every
/// config directory under it opened as `~/.config` from then on, sessions and all.
#[test]
fn a_luxe_directory_outside_any_checkout_claims_nothing_below_it() {
    let home = tempfile::tempdir().expect("temp home");
    let config = home.path().join(".config");
    std::fs::create_dir_all(config.join(PROJECT_DIR)).expect("luxe dir from an earlier run");
    let herdr = config.join("herdr");
    std::fs::create_dir_all(&herdr).expect("nested dir");

    assert_eq!(discover_within(&herdr, Some(home.path())), herdr);
    assert_eq!(discover_within(&config, Some(home.path())), config, "but it still names itself");
}

/// The `/tmp` case: a stray above a real checkout used to re-root the repository at the stray.
#[test]
fn a_luxe_directory_above_a_checkout_does_not_claim_it() {
    let tree = tempfile::tempdir().expect("temp tree");
    std::fs::create_dir_all(tree.path().join(PROJECT_DIR)).expect("stray luxe dir");
    let repository = tree.path().join("repo");
    std::fs::create_dir_all(repository.join(".git")).expect("git marker");
    let deep = repository.join("src");
    std::fs::create_dir_all(&deep).expect("nested dir");

    assert_eq!(discover_within(&deep, None), repository);
}

/// A non-VCS tree has no top anybody agrees on, so a subdirectory is not inherited by its
/// parent's project however recently luxe ran there.
#[test]
fn a_loose_subdirectory_is_its_own_project() {
    let tree = tempfile::tempdir().expect("temp tree");
    std::fs::create_dir_all(tree.path().join(PROJECT_DIR)).expect("luxe dir");
    let sub = tree.path().join("work");
    std::fs::create_dir_all(sub.join(PROJECT_DIR)).expect("its own luxe dir");

    assert_eq!(discover_within(&sub.join("notes"), None), sub.join("notes"));
    assert_eq!(discover_within(&sub, None), sub, "the nearest one is no special case either");
}

// === where the repository's own word stops ===

#[test]
fn the_checkout_top_is_the_nearest_working_copy() {
    let tree = repo();
    let deep = tree.path().join("crates").join("core").join("src");
    assert_eq!(checkout_top(&deep, None), Some(tree.path().to_path_buf()));

    let loose = tempfile::tempdir().expect("temp tree");
    assert_eq!(checkout_top(loose.path(), None), None);
}

/// A scoped package is its own project and still part of its repository, which is the whole
/// reason the two answers are separate functions.
#[test]
fn a_scoped_package_sits_below_its_checkout_top() {
    let tree = repo();
    let package = tree.path().join("crates").join("core");
    std::fs::create_dir_all(package.join(PROJECT_DIR)).expect("scoped package");

    assert_eq!(discover_within(&package.join("src"), None), package);
    assert_eq!(checkout_top(&package.join("src"), None), Some(tree.path().to_path_buf()));
}

/// The user's report: a `.luxe/` and no checkout, with somebody else's `AGENTS.md` one level
/// up. The chain is the directory alone, so nothing above it can reach the prompt.
#[test]
fn the_chain_outside_a_checkout_is_the_directory_alone() {
    let tree = tempfile::tempdir().expect("temp tree");
    std::fs::write(tree.path().join("AGENTS.md"), "not yours").expect("parent rules");
    let project = tree.path().join("test-dir-root");
    std::fs::create_dir_all(project.join(PROJECT_DIR)).expect("luxe dir");

    assert_eq!(chain_from_top_within(&project, None), vec![project]);
}

#[test]
fn the_chain_inside_a_checkout_runs_top_to_start() {
    let tree = repo();
    let deep = tree.path().join("crates").join("core");
    assert_eq!(
        chain_from_top_within(&deep, None),
        vec![tree.path().to_path_buf(), tree.path().join("crates"), deep.clone()]
    );
    assert_eq!(chain_from_top_within(tree.path(), None), vec![tree.path().to_path_buf()]);
}

/// The wrappers read the real home, which a temporary directory is never at or under on the
/// platforms CI runs — or, on Windows, is under with a checkout found long before reaching it.
#[test]
fn the_real_home_wrappers_answer_like_the_testable_ones() {
    let tree = repo();
    let deep = tree.path().join("crates").join("core");
    assert_eq!(discover(&deep), tree.path());
    assert_eq!(chain_from_top(&deep).first().map(PathBuf::as_path), Some(tree.path()));
}

/// Where the shell stood is what the dashboard and the context list walk from, as the prompt
/// does; with nothing recorded, the root answers for itself.
#[test]
fn the_instruction_dir_is_the_launch_directory_once_noted() {
    let tree = repo();
    let root = tree.path();
    let package = root.join("crates").join("core");
    assert_eq!(launched_from(), None);
    assert_eq!(instruction_dir(root), root);

    note_launched_from(&package);
    assert_eq!(launched_from(), Some(package.clone()));
    assert_eq!(instruction_dir(root), package);
}

#[test]
fn the_checkout_top_honors_the_home_guard() {
    let home = tempfile::tempdir().expect("temp home");
    std::fs::create_dir_all(home.path().join(".git")).expect("dotfiles repo");
    let scratch = home.path().join("scratch");
    std::fs::create_dir_all(&scratch).expect("nested dir");

    assert_eq!(checkout_top(&scratch, Some(home.path())), None);
    assert_eq!(checkout_top(home.path(), Some(home.path())), Some(home.path().to_path_buf()));
}
