//! Deployments — what an Azure resource serves, asked of the resource.
//!
//! The models.dev `azure` catalog lists every model Azure *offers*. A resource serves only what
//! somebody deployed on it, under whatever name they picked, so a picker built from the catalog
//! offers models the resource has never heard of: `claude-opus-5-5` beside a resource that has
//! `claude-opus-5`. The resource is the only authority, the same way a local server is for what it
//! has loaded, and its answer is recorded as the provider's `served_models`.
//!
//! The answer comes from the data-plane `GET /openai/deployments`. Newer API versions dropped that
//! route and only the oldest still answers, on a Foundry resource too, where it lists the Claude and
//! partner deployments beside OpenAI's. It may go away. When it does, the list recorded last
//! stands, and `served_models` is still the way to write one by hand.

use std::collections::BTreeMap;

use super::catalog::ProviderDescriptor;
use super::source::RequestAuth;

/// The one API version the listing still answers at.
///
/// `2023-03-15-preview` answers too; everything after `2023-05-15` returns 404.
pub const API_VERSION: &str = "2022-12-01";

/// One deployment on a resource.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Deployment {
    /// What a request names: the deployment, which is not always the model
    pub name: String,
    /// The model it runs, as Azure spells it (`DeepSeek-V4-Flash`, `claude-opus-5`)
    pub model: String,
}

/// Why a resource did not say what it serves.
#[derive(Debug, thiserror::Error)]
pub enum DeploymentsError {
    /// Nothing answered.
    #[error("could not reach {url}: {message}")]
    Network {
        /// The listing URL
        url: String,
        /// What the transport said
        message: String,
    },
    /// The resource answered, and refused the credential.
    #[error("the resource refused the credential ({0})")]
    Rejected(u16),
    /// The resource answered with anything else: the route is gone, or this kind of resource
    /// never had it.
    #[error("the resource has no deployment listing ({0})")]
    Unsupported(u16),
}

/// What one provider on a resource serves, out of everything deployed there.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Served {
    /// Deployment names, in the order the picker should offer them
    pub models: Vec<String>,
    /// Deployment name → model, for the deployments named something else
    pub deployments: BTreeMap<String, String>,
}

/// The listing URL for a resource, from any base URL configured under it.
///
/// Cut at the first `/openai` or `/anthropic` path segment, because that is where a resource's
/// APIs hang, whichever of them the base URL happens to be. `None` for something that is not a URL.
pub fn listing_url(base_url: &str) -> Option<String> {
    let mut url = reqwest::Url::parse(base_url.trim()).ok()?;
    let segments: Vec<String> =
        url.path_segments().map(|parts| parts.map(str::to_string).collect()).unwrap_or_default();
    let root = segments
        .iter()
        .position(|segment| matches!(segment.as_str(), "openai" | "anthropic"))
        .unwrap_or(segments.len());
    let prefix: Vec<&str> =
        segments[..root].iter().map(String::as_str).filter(|part| !part.is_empty()).collect();
    let path = match prefix.is_empty() {
        true => "/openai/deployments".to_string(),
        false => format!("/{}/openai/deployments", prefix.join("/")),
    };
    url.set_path(&path);
    url.set_query(Some(&format!("api-version={API_VERSION}")));
    url.set_fragment(None);
    Some(url.to_string())
}

/// Ask a resource what it has deployed.
///
/// Times out with the shared client's own limits; a caller probing in the background races it
/// against something shorter.
pub async fn list(url: &str, auth: &RequestAuth) -> Result<Vec<Deployment>, DeploymentsError> {
    let request = crate::http::shared_client().get(url).header("Accept", "application/json");
    let response = auth.apply(request).send().await.map_err(|error| DeploymentsError::Network {
        url: url.to_string(),
        message: error.to_string(),
    })?;
    let status = response.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err(DeploymentsError::Rejected(status.as_u16()));
    }
    if !status.is_success() {
        return Err(DeploymentsError::Unsupported(status.as_u16()));
    }
    let body = response.text().await.map_err(|error| DeploymentsError::Network {
        url: url.to_string(),
        message: error.to_string(),
    })?;
    Ok(parse(&body))
}

/// The usable deployments in a listing, sorted by name.
///
/// A deployment still creating, failed or being deleted cannot answer a request, so only
/// `succeeded` ones count. One with no status is kept: the field is the listing's, not the
/// deployment's, and its absence says nothing.
pub fn parse(body: &str) -> Vec<Deployment> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(body) else { return Vec::new() };
    let rows = value.get("data").and_then(serde_json::Value::as_array);
    let mut found: Vec<Deployment> = rows
        .into_iter()
        .flatten()
        .filter(|row| {
            row.get("status")
                .and_then(serde_json::Value::as_str)
                .is_none_or(|status| status.eq_ignore_ascii_case("succeeded"))
        })
        .filter_map(|row| {
            let name = row.get("id")?.as_str()?.trim();
            let model = row.get("model").and_then(serde_json::Value::as_str).unwrap_or(name).trim();
            (!name.is_empty()).then(|| Deployment { name: name.into(), model: model.into() })
        })
        .collect();
    found.sort_by_key(|deployment| deployment.name.to_ascii_lowercase());
    found
}

/// Model-name fragments of deployments that cannot hold a conversation.
///
/// A resource carries speech, image and embedding deployments beside its chat ones, and the
/// catalog does not know all of them (`gpt-realtime-2` is not in it at all), so the name is the
/// evidence that is always there.
const NOT_CHAT: &[&str] =
    &["realtime", "transcribe", "tts", "whisper", "embedding", "dall-e", "sora", "-image", "audio"];

/// Whether a deployment of `model` can hold a conversation.
///
/// Judged by name first, then by the catalog where it knows the model and says it writes no text.
/// A model the catalog marks as unable to call tools is kept: somebody deployed it, and the
/// endpoint is a better judge of what it serves than a catalog flag.
pub fn is_chat(model: &str, models_dev: Option<&str>) -> bool {
    let lower = model.to_ascii_lowercase();
    if lower.starts_with("gpt-image") || NOT_CHAT.iter().any(|fragment| lower.contains(fragment)) {
        return false;
    }
    let Some(known) = models_dev.and_then(|id| crate::catalog::snapshot().model(id, &lower)) else {
        return true;
    };
    known.modalities.as_ref().is_none_or(|modalities| {
        modalities.output.is_empty() || modalities.output.iter().any(|kind| kind == "text")
    })
}

/// The providers one resource is reached through: `descriptor` and the siblings its login
/// configures, the ones that can list deployments.
pub fn group(descriptor: &ProviderDescriptor) -> Vec<&ProviderDescriptor> {
    std::iter::once(descriptor)
        .chain(descriptor.paired_with.iter().filter_map(|id| super::provider_descriptor(id)))
        .filter(|member| member.lists_deployments)
        .collect()
}

/// "22 deployments: 12 OpenAI, 7 Claude, 3 partner" — the line that says discovery worked.
pub fn describe(split: &[(&str, Served)]) -> Option<String> {
    let total: usize = split.iter().map(|(_, served)| served.models.len()).sum();
    if total == 0 {
        return None;
    }
    let parts: Vec<String> = split
        .iter()
        .filter(|(_, served)| !served.models.is_empty())
        .map(|(id, served)| format!("{} on {id}", served.models.len()))
        .collect();
    Some(format!(
        "{total} chat deployment{} on this resource ({})",
        if total == 1 { "" } else { "s" },
        parts.join(", ")
    ))
}

/// Split a resource's deployments between the providers it is reached through.
///
/// Each deployment goes to the first member that serves its **model** — never its name, which
/// can be anything. Every member gets an entry, an empty one included: a resource with no Claude
/// deployment should offer none, not fall back to the catalog's idea of Claude.
pub fn partition<'a>(
    group: &[&'a ProviderDescriptor],
    found: &[Deployment],
) -> Vec<(&'a str, Served)> {
    let mut served: Vec<(&'a str, Served)> =
        group.iter().map(|member| (&*member.id, Served::default())).collect();
    for deployment in found {
        let models_dev = group.first().and_then(|member| member.models_dev);
        if !is_chat(&deployment.model, models_dev) {
            continue;
        }
        let Some(at) =
            group.iter().position(|member| member.served_models.serves(&deployment.model))
        else {
            continue;
        };
        let entry = &mut served[at].1;
        entry.models.push(deployment.name.clone());
        if !deployment.name.eq_ignore_ascii_case(&deployment.model) {
            entry.deployments.insert(deployment.name.clone(), deployment.model.clone());
        }
    }
    served
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::auth::provider_descriptor;

    /// Copied from a real Foundry resource's answer, trimmed to the rows that each make a point.
    const LISTING: &str = r#"{
      "data": [
        {"scale_settings": {"scale_type": "standard"}, "model": "gpt-4.1", "owner": "organization-owner",
         "id": "gpt-4.1", "status": "succeeded", "created_at": 1764578474, "updated_at": 1771687684,
         "object": "deployment"},
        {"model": "gpt-chat-latest", "id": "gpt-5.1-chat", "status": "succeeded", "object": "deployment"},
        {"model": "gpt-realtime", "id": "gpt-realtime", "status": "succeeded", "object": "deployment"},
        {"model": "gpt-4o-mini-transcribe", "id": "gpt-4o-mini-transcribe", "status": "succeeded", "object": "deployment"},
        {"model": "gpt-4o-mini-tts", "id": "gpt-4o-mini-tts", "status": "succeeded", "object": "deployment"},
        {"model": "gpt-image-2", "id": "gpt-image-2", "status": "succeeded", "object": "deployment"},
        {"model": "Kimi-K2.5", "id": "Kimi-K2-Thinking", "status": "succeeded", "object": "deployment"},
        {"model": "claude-opus-5", "id": "claude-opus-5", "status": "succeeded", "object": "deployment"},
        {"model": "claude-sonnet-5", "id": "claude-sonnet-5", "status": "creating", "object": "deployment"},
        {"model": "gpt-5.6-sol", "id": "gpt-5.6-sol", "status": "succeeded", "object": "deployment"},
        {"model": "DeepSeek-V4-Flash", "id": "DeepSeek-V4-Flash", "status": "succeeded", "object": "deployment"}
      ],
      "object": "list"
    }"#;

    fn foundry() -> Vec<&'static ProviderDescriptor> {
        ["foundry-openai", "foundry-anthropic", "foundry-completions"]
            .iter()
            .map(|id| provider_descriptor(id).unwrap())
            .collect()
    }

    fn served<'a>(split: &'a [(&str, Served)], id: &str) -> &'a Served {
        &split.iter().find(|(provider, _)| *provider == id).unwrap().1
    }

    #[test]
    fn the_listing_hangs_off_the_resource_root_whatever_the_base_url() {
        let expected = format!(
            "https://acme.services.ai.azure.com/openai/deployments?api-version={API_VERSION}"
        );
        for base in [
            "https://acme.services.ai.azure.com/openai/v1",
            "https://acme.services.ai.azure.com/anthropic/v1",
            "https://acme.services.ai.azure.com/openai/v1/",
            "https://acme.services.ai.azure.com",
        ] {
            assert_eq!(listing_url(base).as_deref(), Some(expected.as_str()), "{base}");
        }
        // A resource named after an API keeps its name: the cut is a path segment, not a substring
        assert_eq!(
            listing_url("https://openai-eastus.openai.azure.com/openai/v1").unwrap(),
            format!("https://openai-eastus.openai.azure.com/openai/deployments?api-version={API_VERSION}")
        );
        // A gateway in front of the resource keeps its own path prefix
        assert_eq!(
            listing_url("https://gw.example/azure/openai/v1").unwrap(),
            format!("https://gw.example/azure/openai/deployments?api-version={API_VERSION}")
        );
        assert_eq!(listing_url("not a url"), None);
    }

    #[test]
    fn only_deployments_that_can_answer_are_listed() {
        let found = parse(LISTING);
        let names: Vec<&str> = found.iter().map(|d| d.name.as_str()).collect();
        assert!(!names.contains(&"claude-sonnet-5"), "still creating");
        assert!(names.contains(&"Kimi-K2-Thinking"));
        let kimi = found.iter().find(|d| d.name == "Kimi-K2-Thinking").unwrap();
        assert_eq!(kimi.model, "Kimi-K2.5", "the model, not the name");
        assert!(parse("<html>").is_empty());
        assert!(parse(r#"{"error": {"code": "404"}}"#).is_empty());
    }

    #[test]
    fn speech_image_and_embedding_deployments_are_not_offered() {
        for model in [
            "gpt-realtime",
            "gpt-realtime-2",
            "gpt-4o-mini-transcribe",
            "gpt-4o-mini-tts",
            "gpt-image-2",
            "text-embedding-3-large",
            "dall-e-3",
        ] {
            assert!(!is_chat(model, Some("azure")), "{model}");
        }
        for model in ["gpt-4.1", "claude-opus-5", "DeepSeek-V4-Flash", "FW-Kimi-K3", "my-model"] {
            assert!(is_chat(model, Some("azure")), "{model}");
        }
    }

    #[test]
    fn each_deployment_goes_to_the_surface_that_serves_its_model() {
        let split = partition(&foundry(), &parse(LISTING));
        assert_eq!(
            served(&split, "foundry-openai").models,
            ["gpt-4.1", "gpt-5.1-chat", "gpt-5.6-sol"]
        );
        assert_eq!(served(&split, "foundry-anthropic").models, ["claude-opus-5"]);
        assert_eq!(
            served(&split, "foundry-completions").models,
            ["DeepSeek-V4-Flash", "Kimi-K2-Thinking"],
            "the model decides, in any case: Kimi-K2-Thinking runs Kimi-K2.5"
        );
        assert_eq!(
            served(&split, "foundry-openai").deployments,
            BTreeMap::from([("gpt-5.1-chat".to_string(), "gpt-chat-latest".to_string())])
        );
        assert_eq!(
            served(&split, "foundry-completions").deployments,
            BTreeMap::from([("Kimi-K2-Thinking".to_string(), "Kimi-K2.5".to_string())]),
            "a deployment named for its model needs no entry"
        );
    }

    #[test]
    fn a_surface_with_nothing_deployed_offers_nothing() {
        let only_gpt = parse(r#"{"data": [{"id": "gpt-4.1", "model": "gpt-4.1"}]}"#);
        let split = partition(&foundry(), &only_gpt);
        assert_eq!(split.len(), 3, "every member is answered for");
        assert!(served(&split, "foundry-anthropic").models.is_empty());
    }
}
