//! Process — `gh` run as a real program: found, failing, fed a private file.

use super::*;

#[cfg(unix)]
pub(super) fn git(dir: &Path, args: &[&str]) {
    let status = std::process::Command::new("git").args(args).current_dir(dir).status().unwrap();
    assert!(status.success(), "git {args:?}");
}

/// The whole path through a real subprocess: prompts are off, stdout is the answer, and the
/// local remote that points at the repository is found.
#[cfg(unix)]
#[tokio::test]
async fn a_repository_is_resolved_through_gh_and_the_local_remotes() {
    let bin = tempfile::tempdir().unwrap();
    let script = format!(
        "[ \"$GH_PROMPT_DISABLED\" = 1 ] || {{ echo 'prompts are on' >&2; exit 3; }}\n\
             case \"$1 $2\" in\n\
               'auth status') echo '{SIGNED_IN}' ;;\n\
               'repo view') echo '{REPO_VIEW}' ;;\n\
             esac"
    );
    let github = GhCli { program: Some(fake_gh(bin.path(), &script)), ..GhCli::default() };

    let checkout = tempfile::tempdir().unwrap();
    git(checkout.path(), &["init", "-q"]);
    git(checkout.path(), &["remote", "add", "fork", "git@github.com:me/luxe-dev.git"]);
    git(
        checkout.path(),
        &["remote", "add", "origin", "https://github.com/nora-quinn/luxe-dev.git"],
    );

    assert_eq!(github.account(None).await.unwrap().login, "Nora-Quinn");
    let repository = github.repository(checkout.path()).await.unwrap();
    assert_eq!(repository.slug(), "nora-quinn/luxe-dev");
    assert_eq!(repository.remote.as_deref(), Some("origin"), "the remote that points at it");
}

/// A failing `gh` is classified from what it printed, and a missing checkout has no remote.
#[cfg(unix)]
#[tokio::test]
async fn a_failing_gh_is_classified_from_what_it_printed() {
    let bin = tempfile::tempdir().unwrap();
    let github = GhCli {
        program: Some(fake_gh(bin.path(), "echo 'no git remotes found' >&2; exit 1")),
        ..GhCli::default()
    };
    let nowhere = tempfile::tempdir().unwrap();
    assert!(matches!(github.repository(nowhere.path()).await, Err(GitHubError::NotGitHub)));
    assert!(GhCli::remotes(nowhere.path()).await.is_empty(), "not a repository, no remotes");
}

/// `--repo` and, off github.com, `--hostname` reach `gh` the way it expects them.
#[cfg(unix)]
#[tokio::test]
async fn pull_request_reads_name_the_repository_and_host() {
    let bin = tempfile::tempdir().unwrap();
    let fixtures = bin.path().join("fixtures");
    std::fs::create_dir(&fixtures).unwrap();
    std::fs::write(fixtures.join("pr.json"), PR_MERGED).unwrap();
    std::fs::write(fixtures.join("threads.json"), THREADS).unwrap();
    std::fs::write(fixtures.join("issue.json"), ISSUE).unwrap();
    let script = format!(
        "case \"$*\" in\n\
               'pr view 14429 --repo ghe.example.com/cli/cli --json '*) cat '{dir}/pr.json' ;;\n\
               'issue view 14404 --repo ghe.example.com/cli/cli --json number,title,body,url,author,comments') cat '{dir}/issue.json' ;;\n\
               'api graphql --hostname ghe.example.com -f '*' -F owner=cli -F name=cli -F number=14475') cat '{dir}/threads.json' ;;\n\
               *) echo \"unexpected: $*\" >&2; exit 9 ;;\n\
             esac",
        dir = fixtures.display()
    );
    let github = GhCli { program: Some(fake_gh(bin.path(), &script)), ..GhCli::default() };
    let repository = Repository {
        host: "ghe.example.com".into(),
        owner: "cli".into(),
        name: "cli".into(),
        default_branch: "trunk".into(),
        remote: None,
    };
    assert_eq!(github.pull_request(&repository, 14429).await.unwrap().number, 14429);
    assert_eq!(github.issue(&repository, 14404).await.unwrap().number, 14404);
    assert_eq!(github.review_threads(&repository, 14475).await.unwrap().threads.len(), 9);
}

/// The review reaches `gh` as a file only its owner can read, which is gone afterwards; a
/// refusal comes back as GitHub's reasons rather than `HTTP 422`.
#[cfg(unix)]
#[tokio::test]
async fn a_review_is_posted_from_a_private_file_that_does_not_outlive_it() {
    let bin = tempfile::tempdir().unwrap();
    let seen = bin.path().join("seen");
    let answer = bin.path().join("answer.json");
    std::fs::write(&answer, REVIEW_POSTED).unwrap();
    let script = format!(
        "for last; do :; done\n\
             ls -l \"$last\" | cut -c1-10 > '{seen}.mode'\n\
             cat \"$last\" > '{seen}.body'\n\
             echo \"$last\" > '{seen}.path'\n\
             case \"$*\" in\n\
               'api --method POST repos/acme/app/pulls/7/reviews --input '*) cat '{answer}' ;;\n\
               *) echo \"unexpected: $*\" >&2; exit 9 ;;\n\
             esac",
        seen = seen.display(),
        answer = answer.display()
    );
    let github = GhCli { program: Some(fake_gh(bin.path(), &script)), ..GhCli::default() };
    let repository = Repository {
        host: "github.com".into(),
        owner: "acme".into(),
        name: "app".into(),
        default_branch: "main".into(),
        remote: None,
    };
    let review = SubmitReview {
        commit_id: "a".repeat(40),
        event: ReviewEvent::Comment,
        body: "Looks right to me.".into(),
        comments: Vec::new(),
    };

    let posted = github.submit_review(&repository, 7, &review).await.unwrap();

    assert_eq!(posted.id, 5307695620);
    let read =
        |suffix: &str| std::fs::read_to_string(format!("{}.{suffix}", seen.display())).unwrap();
    assert_eq!(read("mode").trim(), "-rw-------", "only the owner can read the reader's words");
    assert!(read("body").contains("Looks right to me."));
    assert!(!std::path::Path::new(read("path").trim()).exists(), "removed after the post");

    let refusing = GhCli {
        program: Some(fake_gh(
            bin.path(),
            &format!("cat '{}'; echo 'gh: Unprocessable Entity (HTTP 422)' >&2; exit 1", {
                let refused = bin.path().join("refused.json");
                std::fs::write(&refused, REFUSED_OWN_APPROVE).unwrap();
                refused.display().to_string()
            }),
        )),
        ..GhCli::default()
    };
    let error = refusing.submit_review(&repository, 7, &review).await.unwrap_err();
    assert!(
        matches!(&error, GitHubError::Rejected { reasons } if reasons[0].contains("own pull request")),
        "{error:?}"
    );
}

/// Resolves this very checkout through a real, signed-in `gh`, so the next host can repeat the
/// check the fixtures stand in for.
#[tokio::test]
#[ignore = "needs a signed-in gh and network access"]
async fn a_real_gh_resolves_this_repository() {
    let github = GhCli::new();
    let account = github.account(None).await.expect("gh is signed in");
    assert!(!account.login.is_empty());
    let repository =
        github.repository(Path::new(env!("CARGO_MANIFEST_DIR"))).await.expect("a GitHub checkout");
    assert_eq!(repository.host, "github.com");
    assert!(repository.remote.is_some(), "a local remote points at {}", repository.slug());
}

/// A real search, paged: the cursor `gh` is handed brings the rows after the first page's,
/// none of them repeated, under the same count.
#[tokio::test]
#[ignore = "needs a signed-in gh and network access"]
async fn a_real_search_pages_on_from_its_cursor() {
    let github = GhCli::new();
    let query = "repo:cli/cli is:pr is:merged sort:updated-desc";
    let first = github.search("github.com", query, None).await.expect("the first page");
    assert_eq!(first.items.len(), 30);
    assert!(first.total > 30, "cli/cli has merged more than a page");
    let after = first.next.as_deref().expect("a cursor to the second page");
    let second = github.search("github.com", query, Some(after)).await.expect("the second");
    assert_eq!(second.total, first.total);
    assert!(second.items.iter().all(|item| first.items.iter().all(|seen| seen.url != item.url)));
}
