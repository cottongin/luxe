use super::*;

const TEST_LIMITS: Limits = Limits { max_bytes: MAX_CLONE_BYTES, deadline: CLONE_DEADLINE };

fn git(dir: &Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .args(["-c", "user.email=t@example.com", "-c", "user.name=t", "-c", "commit.gpgsign=false"])
        .args(args)
        .current_dir(dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .expect("git runs");
    assert!(status.success(), "git {args:?}");
}

/// A repository with one committed file, and the `file://` URL git can clone it from.
fn upstream(dir: &Path, file: &str, body: &str) -> String {
    std::fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-q"]);
    std::fs::write(dir.join(file), body).unwrap();
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-qm", "first"]);
    url::Url::from_directory_path(dir).expect("absolute path").to_string()
}

fn text(result: &ToolResultData) -> String {
    match &result.content[0] {
        luxe_core::ContentBlock::Text(text) => text.clone(),
        other => panic!("expected text, got {other:?}"),
    }
}

fn staging_left(cache: &Path) -> Vec<String> {
    std::fs::read_dir(cache)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .filter(|name| name.starts_with(STAGING_PREFIX))
                .collect()
        })
        .unwrap_or_default()
}

/// The incident this module exists for: a repository whose README is a link to the project's
/// `.env` handed the secret to the model in `web_fetch`'s own reply.
#[cfg(unix)]
#[tokio::test]
async fn a_link_to_the_projects_env_is_checked_out_as_text_and_the_secret_stays_home() {
    let scratch = tempfile::tempdir().unwrap();
    let source = scratch.path().join("evil");
    std::fs::create_dir_all(&source).unwrap();
    git(&source, &["init", "-q"]);
    // From `<project>/.luxe/gh-cache/o/r/`, four levels up is the project
    std::os::unix::fs::symlink("../../../../.env", source.join("README.md")).unwrap();
    git(&source, &["add", "-A"]);
    git(&source, &["commit", "-qm", "link"]);
    let project = scratch.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(project.join(".env"), "API_KEY=sk-live-SECRET\n").unwrap();

    let url = url::Url::from_directory_path(&source).unwrap().to_string();
    let result = fetch_into(&url, "o", "r", &project, TEST_LIMITS).await;

    assert!(!result.is_error, "{}", text(&result));
    assert!(!text(&result).contains("sk-live-SECRET"), "{}", text(&result));
    let readme = luxe_core::fetched::cache_dir(&project).join("o").join("r").join("README.md");
    let metadata = std::fs::symlink_metadata(&readme).unwrap();
    assert!(metadata.is_file(), "checked out as a plain file, not a link");
    assert_eq!(std::fs::read_to_string(&readme).unwrap(), "../../../../.env");
}

/// The preview is the read that reaches the model with no further step, so it refuses a link
/// even where one got into a checkout some other way.
#[cfg(unix)]
#[test]
fn the_readme_preview_does_not_follow_a_link() {
    let scratch = tempfile::tempdir().unwrap();
    std::fs::write(scratch.path().join("secret"), "API_KEY=sk-live-SECRET").unwrap();
    let checkout = scratch.path().join("checkout");
    std::fs::create_dir_all(&checkout).unwrap();
    std::os::unix::fs::symlink("../secret", checkout.join("README.md")).unwrap();
    std::fs::write(checkout.join("README"), "the real one").unwrap();
    let preview = readme_preview(&checkout);
    assert!(!preview.contains("sk-live"), "{preview}");
    assert!(preview.contains("the real one"), "falls through to the next candidate");
}

#[tokio::test]
async fn a_clone_is_checked_out_with_its_commit_and_readme() {
    let scratch = tempfile::tempdir().unwrap();
    let up = scratch.path().join("up");
    std::fs::create_dir_all(up.join("src")).unwrap();
    std::fs::write(up.join("src").join("lib.rs"), "").unwrap();
    let url = upstream(&up, "README.md", "# Widget\nIt widgets.\n");
    let head = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .current_dir(&up)
        .output()
        .unwrap();
    let head = String::from_utf8_lossy(&head.stdout).trim().to_string();
    let project = scratch.path().join("project");
    let result = fetch_into(&url, "acme", "widget", &project, TEST_LIMITS).await;
    let said = text(&result);
    assert!(!result.is_error, "{said}");
    assert!(said.contains("acme/widget is at"), "{said}");
    assert!(said.contains("fetched just now"), "{said}");
    assert!(said.contains(&format!(", at commit {head} (")), "names the commit: {said}");
    assert!(said.contains("It widgets."), "README preview: {said}");
    assert!(said.contains("  README.md"), "listing: {said}");
    assert!(said.contains("  src/"), "a directory is marked as one: {said}");
    assert!(!said.contains("  README.md/"), "and a file is not: {said}");
    assert!(staging_left(&luxe_core::fetched::cache_dir(&project)).is_empty());
}

/// The numbers the guide quotes, so changing one is a decision rather than a slip.
#[test]
fn the_limits_are_the_ones_the_guide_promises() {
    assert_eq!(MAX_CLONE_BYTES, 500 * 1024 * 1024, "500 MB on disk");
    assert_eq!(CLONE_DEADLINE, Duration::from_secs(45));
    assert_eq!(REFRESH_AFTER, Duration::from_secs(86_400), "a day");
    assert_eq!(PRUNE_AFTER, Duration::from_secs(14 * 86_400), "14 days");
    assert_eq!(STAGING_STALE_AFTER, Duration::from_secs(3_600));
}

/// A staging directory carries the second it was made, which is all that stops one Luxe from
/// tidying away a clone another is still making.
#[test]
fn a_staging_directory_made_just_now_survives_tidying() {
    let scratch = tempfile::tempdir().unwrap();
    let staging = staging_dir(scratch.path(), "o", "r");
    std::fs::create_dir_all(&staging).unwrap();
    tidy(scratch.path(), now_secs());
    assert!(staging.exists());
    tidy(scratch.path(), now_secs() + STAGING_STALE_AFTER.as_secs());
    assert!(!staging.exists());
}

#[test]
fn installing_replaces_a_checkout_and_defers_to_one_installed_meanwhile() {
    let scratch = tempfile::tempdir().unwrap();
    let checkout = |path: &Path, marker: &str| {
        std::fs::create_dir_all(path.join(".git")).unwrap();
        std::fs::write(path.join("marker"), marker).unwrap();
    };
    let marker = |path: &Path| std::fs::read_to_string(path.join("marker")).unwrap();
    let (staging, dest, retired) =
        (scratch.path().join("s"), scratch.path().join("d"), scratch.path().join("r"));

    checkout(&staging, "new");
    assert!(install(&staging, &dest, None).unwrap(), "nothing there yet");
    assert_eq!(marker(&dest), "new");

    checkout(&staging, "newer");
    assert!(install(&staging, &dest, Some(&retired)).unwrap(), "a refresh");
    assert_eq!((marker(&dest), marker(&retired)), ("newer".into(), "new".into()));

    checkout(&staging, "late");
    assert!(!install(&staging, &dest, None).unwrap(), "another fetch got there first");
    assert_eq!(marker(&dest), "newer", "and its checkout is kept");

    let junk = scratch.path().join("junk");
    std::fs::create_dir_all(&junk).unwrap();
    std::fs::write(junk.join("file"), "").unwrap();
    assert!(install(&staging, &junk, None).is_err(), "a directory that is not a checkout");
}

#[test]
fn the_size_cap_is_exceeded_only_past_it() {
    let scratch = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(scratch.path().join("nested")).unwrap();
    std::fs::write(scratch.path().join("a"), [0u8; 600]).unwrap();
    std::fs::write(scratch.path().join("nested").join("b"), [0u8; 400]).unwrap();
    assert!(!size_over(scratch.path(), 1_000), "exactly at the cap is within it");
    assert!(size_over(scratch.path(), 999), "nested files count");
}

#[test]
fn the_readme_preview_is_its_first_1500_characters() {
    let scratch = tempfile::tempdir().unwrap();
    let body: String = (0..3_000).map(|index| if index == 1_499 { 'Z' } else { 'a' }).collect();
    std::fs::write(scratch.path().join("README.md"), body).unwrap();
    let preview = readme_preview(scratch.path());
    assert!(preview.contains('Z'), "the 1500th character is in it");
    assert_eq!(preview.chars().filter(|c| *c == 'a').count(), 1_499);
}

/// A repository git cannot reach fails fast and leaves nothing a later fetch would mistake for
/// a finished checkout.
#[tokio::test]
async fn a_failed_clone_leaves_nothing_behind() {
    let scratch = tempfile::tempdir().unwrap();
    let missing = url::Url::from_directory_path(scratch.path().join("nowhere")).unwrap();
    let project = scratch.path().join("project");
    let result = fetch_into(missing.as_str(), "o", "r", &project, TEST_LIMITS).await;
    assert!(result.is_error);
    assert!(text(&result).contains("Could not clone o/r"), "{}", text(&result));
    let cache = luxe_core::fetched::cache_dir(&project);
    assert!(!cache.join("o").join("r").exists());
    assert!(staging_left(&cache).is_empty());
}

#[tokio::test]
async fn a_repository_over_the_cap_is_stopped_and_removed() {
    let scratch = tempfile::tempdir().unwrap();
    let url = upstream(&scratch.path().join("up"), "big.txt", &"x".repeat(64 * 1024));
    let project = scratch.path().join("project");
    let tiny = Limits { max_bytes: 1024, ..TEST_LIMITS };
    let result = fetch_into(&url, "o", "r", &project, tiny).await;
    assert!(result.is_error);
    assert!(text(&result).contains("larger than"), "{}", text(&result));
    let cache = luxe_core::fetched::cache_dir(&project);
    assert!(!cache.join("o").join("r").exists());
    // The staging directory goes on a background thread
    for _ in 0..50 {
        if staging_left(&cache).is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(staging_left(&cache).is_empty());
}

/// A fresh checkout is reused as it stands; a stale one is cloned again, and a stale one that
/// cannot be refreshed is still served, saying how old it is.
#[tokio::test]
async fn a_checkout_is_reused_then_refreshed_once_it_goes_stale() {
    let scratch = tempfile::tempdir().unwrap();
    let up = scratch.path().join("up");
    let url = upstream(&up, "a.txt", "one");
    let project = scratch.path().join("project");
    let checkout = luxe_core::fetched::cache_dir(&project).join("o").join("r");
    assert!(!fetch_into(&url, "o", "r", &project, TEST_LIMITS).await.is_error);

    std::fs::write(up.join("b.txt"), "two").unwrap();
    git(&up, &["add", "-A"]);
    git(&up, &["commit", "-qm", "second"]);
    assert!(!fetch_into(&url, "o", "r", &project, TEST_LIMITS).await.is_error);
    assert!(!checkout.join("b.txt").exists(), "a fresh checkout is not cloned again");

    let two_days_ago = now_secs() - 2 * 24 * 60 * 60;
    std::fs::write(checkout.join(".git").join(FETCHED_STAMP), two_days_ago.to_string()).unwrap();
    assert!(!fetch_into(&url, "o", "r", &project, TEST_LIMITS).await.is_error);
    assert!(checkout.join("b.txt").exists(), "a stale checkout is refreshed");

    std::fs::write(checkout.join(".git").join(FETCHED_STAMP), two_days_ago.to_string()).unwrap();
    std::fs::remove_dir_all(&up).unwrap();
    let result = fetch_into(&url, "o", "r", &project, TEST_LIMITS).await;
    assert!(!result.is_error, "{}", text(&result));
    assert!(text(&result).contains("Could not refresh it"), "{}", text(&result));
    assert!(text(&result).contains("2 days ago"), "{}", text(&result));
    assert!(checkout.join("b.txt").exists(), "the old copy survives a failed refresh");
}

#[test]
fn tidying_clears_leftovers_old_layouts_and_unused_checkouts() {
    let scratch = tempfile::tempdir().unwrap();
    let cache = scratch.path().to_path_buf();
    let now = 10_000_000;
    let checkout = |owner: &str, repo: &str, used: u64| {
        let git = cache.join(owner).join(repo).join(".git");
        std::fs::create_dir_all(&git).unwrap();
        std::fs::write(git.join(USED_STAMP), used.to_string()).unwrap();
    };
    checkout("keep", "recent", now - 60);
    checkout("gone", "unused", now - PRUNE_AFTER.as_secs());
    std::fs::create_dir_all(cache.join("acme-legacy").join(".git")).unwrap();
    let old = format!("{STAGING_PREFIX}{}-1-1-o-r", now - STAGING_STALE_AFTER.as_secs());
    let live = format!("{STAGING_PREFIX}{}-1-1-o-r", now - 30);
    std::fs::create_dir_all(cache.join(&old)).unwrap();
    std::fs::create_dir_all(cache.join(&live)).unwrap();

    tidy(&cache, now);

    assert!(cache.join("keep").join("recent").exists());
    assert!(!cache.join("gone").exists(), "the unused checkout and its empty owner");
    assert!(!cache.join("acme-legacy").exists(), "cloned before links were made plain");
    assert!(!cache.join(&old).exists(), "a staging directory left behind");
    assert!(cache.join(&live).exists(), "a clone still running elsewhere");
}

#[test]
fn only_a_plain_owner_and_repository_name_becomes_a_path() {
    let cache = Path::new("/p/.luxe/gh-cache");
    assert_eq!(
        checkout_dir(cache, "rust-lang", "cargo"),
        Some(cache.join("rust-lang").join("cargo"))
    );
    assert!(checkout_dir(cache, "acme", ".github").is_some(), "an org profile repository");
    assert_ne!(checkout_dir(cache, "a-b", "c"), checkout_dir(cache, "a", "b-c"));
    for (owner, repo) in [
        ("o", ".."),
        ("o", "."),
        ("o", ".git"),
        ("..", "r"),
        ("o/x", "r"),
        ("o", "r/x"),
        ("o", "r%2F"),
        ("", "r"),
        ("o", ""),
        ("o.x", "r"),
    ] {
        assert_eq!(checkout_dir(cache, owner, repo), None, "{owner}/{repo}");
    }
}

/// Each of these is a way a clone could act on its own or wait on a person; pinned so none
/// of them goes missing quietly.
#[test]
fn the_clone_turns_off_links_prompts_and_lfs() {
    let command = clone_command(
        Path::new("git"),
        "https://github.com/o/r.git",
        Path::new("s"),
        Path::new("."),
    );
    let command = command.as_std();
    let args: Vec<String> =
        command.get_args().map(|arg| arg.to_string_lossy().into_owned()).collect();
    let at = args.iter().position(|arg| arg == "--config").expect("--config");
    assert_eq!(args[at + 1], "core.symlinks=false");
    assert!(args.windows(2).any(|pair| pair == ["--depth", "1"]), "{args:?}");
    let env = |key: &str| {
        command
            .get_envs()
            .find(|(name, _)| *name == key)
            .and_then(|(_, value)| value.map(|value| value.to_string_lossy().into_owned()))
    };
    assert_eq!(env("GIT_TERMINAL_PROMPT").as_deref(), Some("0"));
    assert_eq!(env("GCM_INTERACTIVE").as_deref(), Some("never"));
    assert_eq!(env("GIT_LFS_SKIP_SMUDGE").as_deref(), Some("1"));
}

#[test]
fn ages_read_in_the_largest_whole_unit() {
    let now = now_secs();
    assert_eq!(age(now), "just now");
    assert_eq!(age(now - 60), "1 minute ago");
    assert_eq!(age(now - 3 * 3_600), "3 hours ago");
    assert_eq!(age(now - 2 * 86_400), "2 days ago");
}
