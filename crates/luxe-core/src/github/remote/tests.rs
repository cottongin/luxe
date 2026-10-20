use super::*;

/// A renamed branch's push route is not one of the ways to its fork, so an SSH alias's
/// single match still finds the fork remote.
#[tokio::test]
async fn a_push_route_is_not_a_way_to_the_fork() {
    let dir = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| {
        let ok = std::process::Command::new("git").args(args).current_dir(dir.path()).status();
        assert!(ok.unwrap().success(), "{args:?}");
    };
    git(&["init", "-q"]);
    git(&["remote", "add", "mara", "git@github-work:mara/app-fork.git"]);
    git(&["config", "remote.mara-push-main.url", "git@github-work:mara/app-fork.git"]);
    git(&["config", "remote.mara-push-main.luxePushRoute", "true"]);
    let both = vec![
        ("mara".to_string(), "git@github-work:mara/app-fork.git".to_string()),
        ("mara-push-main".to_string(), "git@github-work:mara/app-fork.git".to_string()),
    ];
    assert_eq!(matching_remote(&both, "github.com", "mara", "app-fork"), None, "the bug");
    let kept = without_push_routes(dir.path(), both).await;
    assert_eq!(matching_remote(&kept, "github.com", "mara", "app-fork").as_deref(), Some("mara"));
}

#[test]
fn a_fork_is_reached_the_way_this_checkout_reaches_its_own() {
    for (url, expected) in [
        ("https://github.com/cli/cli.git", "https://github.com/jarrensj/gh-cli.git"),
        ("https://github.com/cli/cli", "https://github.com/jarrensj/gh-cli"),
        ("git@github.com:cli/cli.git", "git@github.com:jarrensj/gh-cli.git"),
        (
            "ssh://git@ssh.github.com:443/cli/cli.git",
            "ssh://git@ssh.github.com:443/jarrensj/gh-cli.git",
        ),
        ("github-work:cli/cli", "github-work:jarrensj/gh-cli"),
        (
            "https://ghe.example.com/org/team/cli.git",
            "https://ghe.example.com/org/jarrensj/gh-cli.git",
        ),
    ] {
        assert_eq!(sibling_url(url, "jarrensj", "gh-cli").as_deref(), Some(expected), "{url}");
    }
    assert_eq!(sibling_url("/srv/git/cli.git", "jarrensj", "gh-cli"), None, "a local path");
}

fn parsed(host: &str, owner: &str, name: &str) -> Option<RemoteUrl> {
    Some(RemoteUrl { host: host.into(), owner: owner.into(), name: name.into() })
}

/// Every spelling git accepts for the same repository reduces to the same three parts.
#[test]
fn every_spelling_of_a_remote_reduces_to_host_owner_name() {
    for url in [
        "https://github.com/nora-quinn/luxe-dev.git",
        "https://github.com/nora-quinn/luxe-dev",
        "https://github.com/nora-quinn/luxe-dev/",
        "https://token@github.com/nora-quinn/luxe-dev.git",
        "http://GitHub.com/nora-quinn/luxe-dev.git",
        "git@github.com:nora-quinn/luxe-dev.git",
        "github.com:nora-quinn/luxe-dev",
        "ssh://git@github.com/nora-quinn/luxe-dev.git",
        "ssh://git@ssh.github.com:443/nora-quinn/luxe-dev.git",
        "git://github.com/nora-quinn/luxe-dev.git",
    ] {
        assert_eq!(parse_remote_url(url), parsed("github.com", "nora-quinn", "luxe-dev"), "{url}");
    }
    assert_eq!(
        parse_remote_url("git@ghe.example.com:Platform/Tools.git"),
        parsed("ghe.example.com", "Platform", "Tools"),
        "an enterprise host keeps its name and the owner keeps its case"
    );
}

/// A local path is not a hosted repository, however much it looks like one.
#[test]
fn a_local_path_is_not_a_hosted_repository() {
    for url in [
        "/srv/git/luxe.git",
        "../luxe.git",
        "file:///srv/git/luxe.git",
        "C:\\src\\luxe.git",
        "C:/src/luxe.git",
        "https://github.com/",
        "https://github.com/only-owner",
        "https://github.com/owner/.git",
        "https:///owner/repo",
        "../relative/path:owner/repo",
        "sub\\dir:owner/repo",
        "",
    ] {
        assert_eq!(parse_remote_url(url), None, "{url:?}");
    }
}

/// Only fetch URLs count, and every remote is kept in git's order. Copied from a real
/// `git remote -v`.
#[test]
fn remotes_are_read_from_git_remote_v() {
    let output = "origin\thttps://github.com/nora-quinn/luxe-dev.git (fetch)\n\
                  origin\thttps://github.com/nora-quinn/luxe-dev.git (push)\n\
                  upstream\tgit@github.com:someone/luxe.git (fetch)\n\
                  upstream\tgit@github.com:someone/luxe.git (push)\n";
    assert_eq!(
        parse_remotes(output),
        vec![
            ("origin".to_string(), "https://github.com/nora-quinn/luxe-dev.git".to_string()),
            ("upstream".to_string(), "git@github.com:someone/luxe.git".to_string()),
        ]
    );
    assert!(parse_remotes("").is_empty());
    assert!(parse_remotes("\thttps://github.com/a/b.git (fetch)").is_empty(), "no name");
    assert!(parse_remotes("origin\t (fetch)").is_empty(), "no URL");
}

/// The remote that points at the repository wins, whatever it is called.
#[test]
fn the_matching_remote_is_found_by_where_it_points() {
    let remotes = vec![
        ("origin".to_string(), "git@github.com:me/luxe.git".to_string()),
        ("upstream".to_string(), "https://github.com/Someone/luxe".to_string()),
    ];
    assert_eq!(
        matching_remote(&remotes, "github.com", "someone", "luxe"),
        Some("upstream".to_string()),
        "case does not matter on GitHub"
    );
    assert_eq!(matching_remote(&remotes, "github.com", "me", "luxe"), Some("origin".into()));
    assert_eq!(matching_remote(&remotes, "github.com", "nobody", "luxe"), None);
}

/// An SSH alias is taken when it is the only candidate, and refused when it is ambiguous.
#[test]
fn an_ssh_host_alias_is_matched_only_when_it_is_unambiguous() {
    let one = vec![("origin".to_string(), "github-work:acme/api.git".to_string())];
    assert_eq!(matching_remote(&one, "github.com", "acme", "api"), Some("origin".into()));

    let two = vec![
        ("a".to_string(), "github-work:acme/api.git".to_string()),
        ("b".to_string(), "github-home:acme/api.git".to_string()),
    ];
    assert_eq!(matching_remote(&two, "github.com", "acme", "api"), None);

    let exact_and_alias = vec![
        ("alias".to_string(), "github-work:acme/api.git".to_string()),
        ("real".to_string(), "https://github.com/acme/api.git".to_string()),
    ];
    assert_eq!(
        matching_remote(&exact_and_alias, "github.com", "acme", "api"),
        Some("real".into()),
        "an exact host beats an alias"
    );
}
