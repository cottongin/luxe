//! Remote URLs — which local remote points at a repository on a code host.
//!
//! `gh` answers "which repository is this" with a URL; a fetch needs a *remote*. Git spells the
//! same place half a dozen ways (https, scp-style, `ssh://`, with or without `.git`, any case), so
//! both sides are reduced to `(host, owner, name)` before they are compared.

/// Where a remote URL points, reduced to the parts that identify a repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RemoteUrl {
    /// Lowercased host, with GitHub's SSH-over-443 alias folded into `github.com`
    pub host: String,
    /// Owner, as written
    pub owner: String,
    /// Repository name, without `.git`
    pub name: String,
}

impl RemoteUrl {
    /// Whether this names the same repository as `(owner, name)`, ignoring case as GitHub does.
    fn same_repository(&self, owner: &str, name: &str) -> bool {
        self.owner.eq_ignore_ascii_case(owner) && self.name.eq_ignore_ascii_case(name)
    }
}

/// Reduce a remote URL to host, owner and name, or `None` for something that is not a hosted
/// repository (a local path, `file://`, a URL with no owner).
pub(crate) fn parse_remote_url(url: &str) -> Option<RemoteUrl> {
    let url = url.trim();
    let (host, path) = if let Some((scheme, rest)) = url.split_once("://") {
        if scheme.eq_ignore_ascii_case("file") {
            return None;
        }
        let (authority, path) = rest.split_once('/')?;
        let host = authority.rsplit('@').next()?;
        // `ssh://git@host:22/owner/repo` carries a port; the host is what matters
        let host = host.split(':').next()?;
        (host, path)
    } else {
        // scp-style: `[user@]host:owner/repo`. A path with a slash before any colon is local.
        let (authority, path) = url.split_once(':')?;
        if authority.contains('/') || authority.contains('\\') || authority.len() <= 1 {
            return None;
        }
        (authority.rsplit('@').next()?, path)
    };

    let segments: Vec<&str> = path.split('/').filter(|segment| !segment.is_empty()).collect();
    let [.., owner, name] = segments.as_slice() else { return None };
    let name = name.strip_suffix(".git").unwrap_or(name);
    if host.is_empty() || owner.is_empty() || name.is_empty() {
        return None;
    }
    Some(RemoteUrl {
        host: canonical_host(host),
        owner: (*owner).to_string(),
        name: name.to_string(),
    })
}

/// Lowercase, with the aliases GitHub itself serves folded into the name it answers to.
fn canonical_host(host: &str) -> String {
    let host = host.to_ascii_lowercase();
    match host.as_str() {
        "ssh.github.com" | "www.github.com" => "github.com".to_string(),
        _ => host,
    }
}

/// `(remote, fetch URL)` pairs from `git remote -v`, in the order git printed them.
pub(crate) fn parse_remotes(git_remote_v: &str) -> Vec<(String, String)> {
    git_remote_v
        .lines()
        .filter_map(|line| {
            let (name, rest) = line.split_once(char::is_whitespace)?;
            let url = rest.trim().strip_suffix("(fetch)")?.trim();
            (!name.is_empty() && !url.is_empty()).then(|| (name.to_string(), url.to_string()))
        })
        .collect()
}

/// The remote that points at `host/owner/name`.
///
/// An exact host match wins. Failing that, a remote with the right owner and name on a host
/// nobody recognizes is taken when it is the only one, because that is an SSH host alias
/// (`github-work:owner/repo`) — git resolves it through `~/.ssh/config`, which this does not read.
pub(crate) fn matching_remote(
    remotes: &[(String, String)],
    host: &str,
    owner: &str,
    name: &str,
) -> Option<String> {
    let host = canonical_host(host);
    let parsed: Vec<(&str, RemoteUrl)> = remotes
        .iter()
        .filter_map(|(remote, url)| parse_remote_url(url).map(|parsed| (remote.as_str(), parsed)))
        .filter(|(_, parsed)| parsed.same_repository(owner, name))
        .collect();
    if let Some((remote, _)) = parsed.iter().find(|(_, parsed)| parsed.host == host) {
        return Some((*remote).to_string());
    }
    match parsed.as_slice() {
        [(remote, _)] => Some((*remote).to_string()),
        _ => None,
    }
}

/// This checkout's remotes, as `(name, fetch URL)`, without the push routes a renamed branch was
/// given ([`crate::worktree`]): one shares its fork remote's URL and is not another way to it, and
/// two matches turn an SSH host alias's one-match fallback in [`matching_remote`] into none.
pub(crate) async fn ways_to_forks(cwd: &std::path::Path) -> Vec<(String, String)> {
    without_push_routes(cwd, super::gh::local_remotes(cwd).await).await
}

async fn without_push_routes(
    cwd: &std::path::Path,
    mut remotes: Vec<(String, String)>,
) -> Vec<(String, String)> {
    // git matches and prints the variable name lowercased, not the remote's
    let mark = crate::worktree::PUSH_ROUTE_MARK.to_ascii_lowercase();
    let pattern = format!(r"^remote\..*\.{mark}$");
    let program = crate::program::resolve("git").unwrap_or_else(|| "git".into());
    let command =
        crate::program::captured_command(program, &["config", "--get-regexp", &pattern], cwd);
    let deadline = std::time::Duration::from_secs(10);
    let Ok(output) = crate::program::run_bounded(command, deadline).await else { return remotes };
    let listed = String::from_utf8_lossy(&output.stdout);
    let suffix = format!(".{mark}");
    let routes: Vec<&str> = listed
        .lines()
        .filter_map(|line| {
            line.split_whitespace().next()?.strip_prefix("remote.")?.strip_suffix(&suffix)
        })
        .collect();
    remotes.retain(|(name, _)| !routes.contains(&name.as_str()));
    remotes
}

/// The same URL, spelled the same way, pointing at `owner/name` instead: how to reach a fork
/// when all that is known is how this checkout reaches its own repository.
///
/// `None` for a URL that is not a hosted repository.
pub(crate) fn sibling_url(url: &str, owner: &str, name: &str) -> Option<String> {
    let url = url.trim();
    parse_remote_url(url)?;
    let suffix = if url.ends_with(".git") { ".git" } else { "" };
    let trimmed = url.strip_suffix(".git").unwrap_or(url).trim_end_matches('/');
    // Everything before the last two path segments stays as written: scheme, user, host, port
    let cut = trimmed.rfind('/')?;
    let before_name = &trimmed[..cut];
    let owner_start = before_name.rfind(['/', ':'])? + 1;
    Some(format!("{}{owner}/{name}{suffix}", &trimmed[..owner_start]))
}

#[cfg(test)]
mod tests;
