//! Discovery paths — where the specifications put each metadata document, and how to read
//! what a server's challenge points at.

use serde_json::Value;

use super::endpoints::{endpoints_from, is_loopback, require_secure, Endpoints};
use super::{OAuthClient, OAuthError, Resolved};

impl OAuthClient {
    /// Endpoints and resource: configured ones win, then discovery.
    pub(super) async fn resolve(&self) -> Result<Resolved, OAuthError> {
        let configured = Endpoints {
            authorization: self.config.authorization_endpoint.clone(),
            token: self.config.token_endpoint.clone(),
            registration: self.config.registration_endpoint.clone(),
        }
        .checked()?;
        if configured.authorization.is_some() && configured.token.is_some() {
            return Ok(Resolved { endpoints: configured, resource: self.server_url.clone() });
        }
        let (discovered, resource) = match self.discover().await? {
            Some((endpoints, resource)) => (endpoints, resource),
            None => (Endpoints::default(), None),
        };
        Ok(Resolved {
            endpoints: Endpoints {
                authorization: configured.authorization.or(discovered.authorization),
                token: configured.token.or(discovered.token),
                registration: configured.registration.or(discovered.registration),
            },
            resource: resource.unwrap_or_else(|| self.server_url.clone()),
        })
    }

    /// Discovery in the specification's order; see the module docs for why the order matters.
    ///
    /// `Err` when metadata was found and failed its checks, or pointed at an authorization server
    /// that would not describe itself: falling back from there would be exactly how the wrong server
    /// gets used.
    async fn discover(&self) -> Result<Option<(Endpoints, Option<String>)>, OAuthError> {
        require_secure(&self.server_url, "the MCP server")?;
        let server = url::Url::parse(&self.server_url)
            .map_err(|e| OAuthError::Discovery(format!("bad server url: {e}")))?;
        if let Some((resource, issuer)) = self.protected_resource(&server).await? {
            let endpoints = self.authorization_server(&issuer).await?;
            return Ok(Some((endpoints, Some(resource))));
        }
        Ok(self.origin_metadata(&server).await?.map(|endpoints| (endpoints, None)))
    }

    /// The protected-resource document (RFC 9728): `(resource, first authorization server)`.
    async fn protected_resource(
        &self,
        server: &url::Url,
    ) -> Result<Option<(String, String)>, OAuthError> {
        // The server's own pointer is an instruction, not a hint: when it names a document, that
        // document or nothing
        if let Some(pointed) = self.resource_metadata_pointer().await {
            return match self.fetch_protected_resource(&pointed, server).await? {
                Some(found) => Ok(Some(found)),
                None => Err(OAuthError::Discovery(format!(
                    "{pointed} did not return protected-resource metadata"
                ))),
            };
        }
        for candidate in protected_resource_candidates(server) {
            if let Some(found) = self.fetch_protected_resource(&candidate, server).await? {
                return Ok(Some(found));
            }
        }
        Ok(None)
    }

    /// `resource_metadata` from the `WWW-Authenticate` header an unauthenticated request draws.
    async fn resource_metadata_pointer(&self) -> Option<String> {
        let initialize = serde_json::json!({
            "jsonrpc": "2.0", "id": 0, "method": "initialize",
            "params": {
                "protocolVersion": luxe_core::mcp_client::MCP_PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": { "name": "luxe", "version": env!("CARGO_PKG_VERSION") }
            }
        });
        let response = self
            .http
            .post(&self.server_url)
            .header(reqwest::header::ACCEPT, "application/json, text/event-stream")
            .json(&initialize)
            .send()
            .await
            .ok()?;
        if response.status() != reqwest::StatusCode::UNAUTHORIZED {
            return None;
        }
        let header = response.headers().get(reqwest::header::WWW_AUTHENTICATE)?.to_str().ok()?;
        auth_param(header, "resource_metadata")
    }

    /// One protected-resource document, checked. `Ok(None)` when there is none at `url`.
    async fn fetch_protected_resource(
        &self,
        url: &str,
        server: &url::Url,
    ) -> Result<Option<(String, String)>, OAuthError> {
        let Some(doc) = self.fetch_json(url).await else {
            return Ok(None);
        };
        let Some(issuer) = doc
            .get("authorization_servers")
            .and_then(Value::as_array)
            .and_then(|servers| servers.first())
            .and_then(Value::as_str)
        else {
            return Ok(None);
        };
        let resource = doc.get("resource").and_then(Value::as_str).unwrap_or_default();
        if !names_the_server(resource, server) {
            return Err(OAuthError::Discovery(format!(
                "{url} describes {resource:?}, not {server} — refusing to send a token for it"
            )));
        }
        Ok(Some((resource.to_string(), issuer.to_string())))
    }

    /// Authorization-server metadata for `issuer` (RFC 8414, then OpenID discovery), checked.
    async fn authorization_server(&self, issuer: &str) -> Result<Endpoints, OAuthError> {
        let url = url::Url::parse(issuer)
            .map_err(|e| OAuthError::Discovery(format!("bad authorization server: {e}")))?;
        if url.scheme() != "https" && !is_loopback(&url) {
            return Err(OAuthError::Insecure(format!(
                "the authorization server {issuer} is not https"
            )));
        }
        for candidate in authorization_server_candidates(&url) {
            let Some(doc) = self.fetch_json(&candidate).await else {
                continue;
            };
            let endpoints = endpoints_from(&doc, &candidate, issuer)?;
            if endpoints.authorization.is_some() || endpoints.token.is_some() {
                return Ok(endpoints);
            }
        }
        Err(OAuthError::Discovery(format!("{issuer} publishes no authorization server metadata")))
    }

    /// The origin lookup, for a server that publishes no protected-resource metadata.
    async fn origin_metadata(&self, server: &url::Url) -> Result<Option<Endpoints>, OAuthError> {
        let Some(origin) = origin_of(server) else { return Ok(None) };
        for suffix in
            ["/.well-known/oauth-authorization-server", "/.well-known/openid-configuration"]
        {
            let candidate = format!("{origin}{suffix}");
            let Some(doc) = self.fetch_json(&candidate).await else {
                continue;
            };
            let endpoints = endpoints_from(&doc, &candidate, &origin)?;
            if endpoints.authorization.is_some() || endpoints.token.is_some() {
                return Ok(Some(endpoints));
            }
        }
        Ok(None)
    }

    /// GET `url` as JSON, or `None` for anything other than a successful JSON answer.
    async fn fetch_json(&self, url: &str) -> Option<Value> {
        let response = self.http.get(url).send().await.ok()?;
        if !response.status().is_success() {
            return None;
        }
        response.json::<Value>().await.ok()
    }
}

/// Where RFC 9728 puts the protected-resource document for `server`: the path form, then the root.
pub(super) fn protected_resource_candidates(server: &url::Url) -> Vec<String> {
    let Some(origin) = origin_of(server) else {
        return Vec::new();
    };
    let path = server.path().trim_end_matches('/');
    let mut candidates = Vec::new();
    if !path.is_empty() {
        candidates.push(format!("{origin}/.well-known/oauth-protected-resource{path}"));
    }
    candidates.push(format!("{origin}/.well-known/oauth-protected-resource"));
    candidates
}

/// Where RFC 8414 and OpenID discovery put an issuer's metadata, in that order.
pub(super) fn authorization_server_candidates(issuer: &url::Url) -> Vec<String> {
    let Some(origin) = origin_of(issuer) else {
        return Vec::new();
    };
    let path = issuer.path().trim_end_matches('/');
    let mut candidates = vec![format!("{origin}/.well-known/oauth-authorization-server{path}")];
    candidates.push(format!("{origin}{path}/.well-known/openid-configuration"));
    if !path.is_empty() {
        candidates.push(format!("{origin}/.well-known/openid-configuration{path}"));
    }
    candidates.dedup();
    candidates
}

/// Whether a protected-resource document's `resource` names the server it was fetched for: the
/// same origin, and the server's path at or under the resource's, a whole segment at a time.
pub(super) fn names_the_server(resource: &str, server: &url::Url) -> bool {
    let Ok(resource) = url::Url::parse(resource) else {
        return false;
    };
    if origin_of(&resource) != origin_of(server) {
        return false;
    }
    let owned = resource.path().trim_end_matches('/');
    let path = server.path().trim_end_matches('/');
    owned.is_empty() || path == owned || path.starts_with(&format!("{owned}/"))
}

/// `scheme://host[:port]`, the part of a URL two documents must share to describe one server.
pub(super) fn origin_of(url: &url::Url) -> Option<String> {
    // Already bracketed for an IPv6 literal
    let host = url.host_str()?;
    Some(format!(
        "{}://{host}{}",
        url.scheme(),
        url.port().map(|port| format!(":{port}")).unwrap_or_default()
    ))
}

/// One parameter of a `WWW-Authenticate` challenge: `resource_metadata="…"`, quoted or bare.
pub(super) fn auth_param(header: &str, name: &str) -> Option<String> {
    let at = header.find(&format!("{name}="))? + name.len() + 1;
    let rest = &header[at..];
    let value = match rest.strip_prefix('"') {
        Some(quoted) => quoted.split('"').next()?,
        None => rest.split([',', ' ']).next()?,
    };
    (!value.is_empty()).then(|| value.to_string())
}
