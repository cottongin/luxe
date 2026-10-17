//! Discovery — the signed-in account, the repository a checkout belongs to, and search.

use super::*;

#[async_trait]
impl Discovery for GhCli {
    async fn account(&self, host: Option<&str>) -> Result<Account, GitHubError> {
        let mut args = vec!["auth", "status", "--json", "hosts"];
        if let Some(host) = host {
            args.extend(["--hostname", host]);
        }
        // Signed out still exits zero with `--json`, so the answer is read from the body
        let cwd = std::env::temp_dir();
        let body = self.gh(&cwd, &args).await?;
        parse_account(&body, host)
    }

    async fn repository(&self, cwd: &Path) -> Result<Repository, GitHubError> {
        let body =
            self.gh(cwd, &["repo", "view", "--json", "nameWithOwner,url,defaultBranchRef"]).await?;
        let mut repository = parse_repository(&body)?;
        repository.remote = matching_remote(
            &Self::remotes(cwd).await,
            &repository.host,
            &repository.owner,
            &repository.name,
        );
        Ok(repository)
    }

    async fn search(
        &self,
        host: &str,
        query: &str,
        after: Option<&str>,
    ) -> Result<SearchPage, GitHubError> {
        let graphql = format!("query={}", search::SEARCH_QUERY);
        let search = format!("q={query}");
        let after = after.map(|cursor| format!("after={cursor}"));
        let mut args = vec!["api", "graphql"];
        if host != "github.com" && !host.is_empty() {
            args.extend(["--hostname", host]);
        }
        args.extend(["-f", graphql.as_str(), "-f", search.as_str()]);
        // Left out, `$after` is null, which is the first page
        if let Some(after) = after.as_deref() {
            args.extend(["-f", after]);
        }
        let body = self.gh(&std::env::temp_dir(), &args).await?;
        parse_search(&body)
    }
}

/// The body of `gh auth status --json hosts`.
#[derive(Deserialize)]
pub(super) struct AuthStatus {
    hosts: std::collections::BTreeMap<String, Vec<AuthEntry>>,
}

#[derive(Deserialize)]
pub(super) struct AuthEntry {
    state: String,
    #[serde(default)]
    active: bool,
    host: String,
    #[serde(default)]
    login: String,
}

/// The active, working account on `host`, or on any host when `host` is `None`.
pub(super) fn parse_account(body: &str, host: Option<&str>) -> Result<Account, GitHubError> {
    let status: AuthStatus = serde_json::from_str(body).map_err(GitHubError::Unreadable)?;
    status
        .hosts
        .values()
        .flatten()
        .filter(|entry| host.is_none_or(|host| entry.host.eq_ignore_ascii_case(host)))
        .find(|entry| entry.active && entry.state == "success" && !entry.login.is_empty())
        .map(|entry| Account { host: entry.host.clone(), login: entry.login.clone() })
        .ok_or_else(|| GitHubError::NotAuthenticated { host: host.map(str::to_string) })
}

/// The body of `gh repo view --json nameWithOwner,url,defaultBranchRef`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RepoView {
    name_with_owner: String,
    url: String,
    default_branch_ref: Option<BranchRef>,
}

#[derive(Deserialize)]
pub(super) struct BranchRef {
    name: String,
}

/// The repository `gh repo view` described, with no remote chosen yet.
pub(super) fn parse_repository(body: &str) -> Result<Repository, GitHubError> {
    let view: RepoView = serde_json::from_str(body).map_err(GitHubError::Unreadable)?;
    let (owner, name) =
        view.name_with_owner.split_once('/').ok_or_else(|| GitHubError::Failed {
            message: format!(
                "gh named the repository `{}`, which is not owner/name",
                view.name_with_owner
            ),
        })?;
    let host =
        parse_remote_url(&view.url).map(|url| url.host).ok_or_else(|| GitHubError::Failed {
            message: format!("gh gave the repository URL `{}`, which names no host", view.url),
        })?;
    Ok(Repository {
        host,
        owner: owner.to_string(),
        name: name.to_string(),
        // An empty repository has no default branch yet; `main` is what GitHub will create
        default_branch: view
            .default_branch_ref
            .map(|branch| branch.name)
            .unwrap_or_else(|| "main".into()),
        remote: None,
    })
}
