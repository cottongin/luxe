//! Background refreshers — re-asking local servers and Azure resources what they serve.
//!
//! Both run while a picker is on screen and leave the recorded list standing when the server does
//! not answer in time.

use super::models::verify_credential;
use super::{ProviderAuth, ProviderDescriptor};
use crate::auth::catalog::provider_descriptors;
use crate::auth::deployments;
use crate::auth::source::{AuthSource, KeyStyle, RequestAuth, StoreAuth};
use crate::config::ApiKey;

/// How long a local server gets to answer before its recorded list stands in.
///
/// Short on purpose: this is a loopback request made while a picker is already on screen, so the
/// cost of waiting is a list that updates late, and the cost of not waiting is nothing.
const LOCAL_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// How long an Azure resource gets to list its deployments before the recorded list stands in.
///
/// Longer than a local server gets, because this one crosses the internet, and still short: it
/// runs while a picker is on screen, and the cost of giving up is a list that updates next time.
const REMOTE_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

impl ProviderAuth {
    /// Re-ask every connected local server what it is serving, answering whether anything moved.
    ///
    /// A local list is not a fact about the world, it is a fact about this machine ten seconds
    /// ago: loading a model in LM Studio changes it, and a picker built from a snapshot taken at
    /// connect time would keep offering yesterday's. Only keyless providers with a block are
    /// asked — they are the ones on this machine, so the call is a loopback round trip that costs
    /// nothing and fails instantly when the server is off, and the recorded list stands in when it
    /// does.
    pub async fn refresh_local_models(&self) -> bool {
        let Ok(config) = crate::config::load_models_config(self.config.path()) else {
            return false;
        };
        let mut changed = false;
        for descriptor in provider_descriptors().iter().filter(|entry| entry.keyless) {
            let Some(block) = config.providers.get(&*descriptor.id) else { continue };
            let Some(base_url) =
                block.base_url.clone().or_else(|| descriptor.base_url.map(str::to_string))
            else {
                continue;
            };
            let anonymous = RequestAuth::none();
            let probe = verify_credential(descriptor, Some(&base_url), &anonymous);
            let Ok(Ok(models)) = tokio::time::timeout(LOCAL_PROBE_TIMEOUT, probe).await else {
                tracing::debug!(provider = %descriptor.id, %base_url, "local server did not answer");
                continue;
            };
            if models.is_empty() || block.served_models.as_deref() == Some(models.as_slice()) {
                continue;
            }
            match self.config.set_served_models(&descriptor.id, &models) {
                Ok(()) => changed = true,
                Err(error) => {
                    tracing::warn!(%error, provider = %descriptor.id, "could not record what the local server serves")
                }
            }
        }
        changed
    }

    /// Ask every Azure resource luxe is set up for what it has deployed, answering whether any
    /// recorded list moved.
    ///
    /// Deployments change in a portal luxe never sees, so the list is re-read rather than trusted
    /// from login. One request per resource, not per provider: the three Foundry providers share
    /// one. A resource that does not answer in time, refuses the key, or no longer has the route
    /// leaves its recorded list standing, and `"discover_deployments": false` on any of its
    /// providers stops the request entirely.
    pub async fn refresh_deployments(&self) -> bool {
        let Ok(config) = crate::config::load_models_config(self.config.path()) else {
            return false;
        };
        let mut asked: Vec<&str> = Vec::new();
        let mut changed = false;
        for descriptor in provider_descriptors().iter().filter(|entry| entry.lists_deployments) {
            if asked.contains(&&*descriptor.id) {
                continue;
            }
            let group = deployments::group(descriptor);
            asked.extend(group.iter().map(|member| &*member.id));
            let reachable =
                |id: &str| config.providers.get(id).filter(|block| block.base_url.is_some());
            if group.iter().any(|member| {
                config.providers.get(&*member.id).and_then(|block| block.discover_deployments)
                    == Some(false)
            }) {
                continue;
            }
            let Some((lead, block)) =
                group.iter().find_map(|member| reachable(&member.id).map(|block| (member, block)))
            else {
                continue;
            };
            let Some(url) = block.base_url.as_deref().and_then(deployments::listing_url) else {
                continue;
            };
            let source = StoreAuth::new(
                self.store.clone(),
                lead.id.to_string(),
                crate::config::resolve_api(&lead.id, Some(block)),
            )
            .with_key_style(Some(crate::config::resolve_key_style(&lead.id, Some(block))))
            .with_fallback(block.api_key.clone());
            let Ok(auth) = source.request_auth().await else { continue };
            let found =
                match tokio::time::timeout(REMOTE_PROBE_TIMEOUT, deployments::list(&url, &auth))
                    .await
                {
                    Ok(Ok(found)) if !found.is_empty() => found,
                    Ok(Ok(_)) => continue,
                    Ok(Err(error)) => {
                        tracing::debug!(provider = %lead.id, %error, "deployments not listed");
                        continue;
                    }
                    Err(_) => {
                        tracing::debug!(provider = %lead.id, "deployment listing timed out");
                        continue;
                    }
                };
            for (id, served) in deployments::partition(&group, &found) {
                let Some(block) = reachable(id) else { continue };
                let recorded = deployments::Served {
                    models: block.served_models.clone().unwrap_or_default(),
                    deployments: block.deployments.clone(),
                };
                if block.served_models.is_some() && recorded == served {
                    continue;
                }
                match self.config.set_deployments(id, &served) {
                    Ok(()) => {
                        tracing::info!(
                            provider = id,
                            count = served.models.len(),
                            "recorded deployments"
                        );
                        changed = true;
                    }
                    Err(error) => {
                        tracing::warn!(%error, provider = id, "could not record deployments")
                    }
                }
            }
        }
        changed
    }
}

/// What a resource has deployed, split between the providers that reach it, using a key that has
/// just been typed.
///
/// Sent as `api-key`, which every Azure data plane accepts for a key. `None` on any failure,
/// including a refusal: a key that cannot list deployments may still run models (a role scoped to
/// inference), so the ordinary verification gets the final say on whether the key is good.
pub(super) async fn discover_with_key<'a>(
    descriptor: &'a ProviderDescriptor,
    resource_root: &str,
    key: &ApiKey,
) -> Option<Vec<(&'a str, deployments::Served)>> {
    let url = deployments::listing_url(&descriptor.endpoint_for(resource_root))?;
    let auth = KeyStyle::AzureKey.headers(key);
    match tokio::time::timeout(REMOTE_PROBE_TIMEOUT, deployments::list(&url, &auth)).await {
        Ok(Ok(found)) if !found.is_empty() => {
            Some(deployments::partition(&deployments::group(descriptor), &found))
        }
        Ok(Ok(_)) => None,
        Ok(Err(error)) => {
            tracing::debug!(provider = %descriptor.id, %error, "deployments not listed at login");
            None
        }
        Err(_) => None,
    }
}
