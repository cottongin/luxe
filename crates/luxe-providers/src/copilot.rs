//! Copilot — one subscription, three wire APIs, chosen per model by what Copilot says it serves.
//!
//! Copilot fronts chat-completions models, Responses-only ones (the newer GPT, Grok, MAI) and
//! Claude on the same host, and refuses a model on the wrong door: "model … is not accessible via
//! the /chat/completions endpoint". Its own `/models` lists each model's `supported_endpoints`, so
//! the routing comes from there rather than from a table that rots or a guess at the id: each model
//! goes to the richest protocol it takes (Messages, then Responses, then chat completions), and one
//! luxe knows nothing about stays on chat completions. The same list says which `reasoning_effort`
//! values each model accepts, which Copilot enforces.
//!
//! Claude takes chat completions too, but that door costs it two things: Copilot hands its thinking
//! back in fields of its own that nothing reads, and it caches only the system prompt there, so
//! the whole conversation is billed again on every call. `/v1/messages` honors thinking, effort
//! and every cache breakpoint the Anthropic adapter places.
//!
//! The list is the account's own, fetched with the request's credential the first time it is
//! needed and again once it is an hour old, or sooner for a model it has not seen.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::stream::{BoxStream, StreamExt};
use luxe_core::message::ContentBlock;
use luxe_core::provider::{
    CompletionRequest, ModelInfo, Provider, ProviderError, ProviderEvent, ThinkingLevel, Usage,
};
use serde_json::Value;
use tracing::debug;

use crate::auth::{AuthSource, RequestAuth};

/// How long a fetched list is trusted before it is fetched again.
const FRESH_FOR: Duration = Duration::from_secs(60 * 60);
/// How soon a model the list does not name may send luxe back to ask, as it may be new.
const RECHECK_UNKNOWN_AFTER: Duration = Duration::from_secs(5 * 60);
/// How long the list may take before a request goes ahead without it.
const FETCH_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a reading of the plan's allowance stands before a request reads it again.
const PLAN_FRESH_FOR: Duration = Duration::from_secs(5 * 60);
/// Set to `0` to stop reading the plan's allowance.
const PLAN_USAGE_SWITCH: &str = "LUXE_COPILOT_PLAN_USAGE";
/// Copilot's `/models` API version.
const MODELS_API_VERSION: &str = "2026-06-01";
/// Where Copilot answers when the credential names no endpoint of its own.
const DEFAULT_BASE_URL: &str = "https://api.individual.githubcopilot.com";

/// Which wire API a model is spoken to in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// `POST /chat/completions`
    Chat,
    /// `POST /responses`
    Responses,
    /// `POST /v1/messages`, Anthropic's protocol
    Messages,
}

/// The account's models and the route each takes, as last fetched.
#[derive(Debug)]
struct Catalog {
    /// When luxe last asked, whether or not the answer arrived
    asked: Instant,
    routes: HashMap<String, Route>,
    /// The `reasoning_effort` values each model takes, for the models that list any
    efforts: HashMap<String, Vec<String>>,
}

/// What one request does about the model list.
enum Claim {
    /// Fetch it, holding the refresh lock until done
    Refresh(tokio::sync::OwnedMutexGuard<()>),
    /// Wait for the fetch already running
    Wait,
    /// Go by what is known
    Known,
}

/// The three adapters Copilot is spoken to through, one per wire API.
pub struct CopilotAdapters {
    /// Chat completions, and the fallback for anything the list does not name
    pub chat: Arc<dyn Provider + Send + Sync>,
    /// The Responses API
    pub responses: Arc<dyn Provider + Send + Sync>,
    /// Anthropic Messages, authenticated through [`messages_auth`]
    pub messages: Arc<dyn Provider + Send + Sync>,
}

/// GitHub Copilot, routed per model between its chat-completions, Responses and Messages endpoints.
pub struct CopilotProvider {
    adapters: CopilotAdapters,
    auth: Arc<dyn AuthSource>,
    /// The configured base URL, for a credential that does not carry its own
    base_url: Option<String>,
    catalog: tokio::sync::Mutex<Option<Catalog>>,
    /// Held while the list is fetched, so a request with no known route can wait for it
    refreshing: Arc<tokio::sync::Mutex<()>>,
    /// When the plan's allowance was last read, so a turn of many calls reads it once
    plan_read: std::sync::Mutex<Option<Instant>>,
}

impl CopilotProvider {
    /// Routes between adapters already built for Copilot, using `auth` to read the model list.
    pub fn new(
        adapters: CopilotAdapters,
        auth: Arc<dyn AuthSource>,
        base_url: Option<String>,
    ) -> Self {
        Self {
            adapters,
            auth,
            base_url,
            catalog: tokio::sync::Mutex::new(None),
            refreshing: Arc::default(),
            plan_read: std::sync::Mutex::new(None),
        }
    }

    /// The adapter a route is spoken through.
    fn adapter(&self, route: Route) -> &Arc<dyn Provider + Send + Sync> {
        match route {
            Route::Chat => &self.adapters.chat,
            Route::Responses => &self.adapters.responses,
            Route::Messages => &self.adapters.messages,
        }
    }

    /// The route already known for `model`, without fetching: the cache warmer asks from code that
    /// cannot wait on the network, and a request that went out has already settled its route.
    fn known_route(&self, model: &str) -> Route {
        self.catalog
            .try_lock()
            .ok()
            .and_then(|catalog| catalog.as_ref().and_then(|known| known.routes.get(model).copied()))
            .unwrap_or(Route::Chat)
    }

    /// The route for `model`, fetching the account's list when it is missing, old, or silent on
    /// this model and has not been asked lately.
    async fn route(&self, model: &str) -> Route {
        self.route_at(model, Instant::now()).await
    }

    /// [`Self::route`] as of `now`, so the hour can pass in a test.
    ///
    /// The list is fetched outside the catalog's lock and stamped asked for once the fetch is
    /// over: what is known stays readable meanwhile, a request with a route of its own goes by
    /// it, and one without waits for the fetch and looks again. A fetch abandoned partway stamps
    /// nothing, so the next request fetches.
    async fn route_at(&self, model: &str, now: Instant) -> Route {
        loop {
            match self.claim_refresh(model, now).await {
                Claim::Refresh(_refreshing) => {
                    let fetched = self.fetch_list().await;
                    let mut catalog = self.catalog.lock().await;
                    // a failed fetch keeps what was known, and waits its turn before asking again
                    let (routes, efforts) = match fetched {
                        Some(body) => (routes(&body), efforts(&body)),
                        None => catalog
                            .take()
                            .map(|known| (known.routes, known.efforts))
                            .unwrap_or_default(),
                    };
                    *catalog = Some(Catalog { asked: now, routes, efforts });
                    break;
                }
                Claim::Wait => drop(self.refreshing.lock().await),
                Claim::Known => break,
            }
        }
        let catalog = self.catalog.lock().await;
        catalog.as_ref().and_then(|known| known.routes.get(model).copied()).unwrap_or(Route::Chat)
    }

    /// What a request for `model` as of `now` does about the list: fetch it, when due and nobody
    /// is; wait for the fetch already running, having no route of its own; or go by what is known.
    async fn claim_refresh(&self, model: &str, now: Instant) -> Claim {
        let catalog = self.catalog.lock().await;
        if !stale(catalog.as_ref(), model, now) {
            return Claim::Known;
        }
        let known = catalog.as_ref().is_some_and(|known| known.routes.contains_key(model));
        match Arc::clone(&self.refreshing).try_lock_owned() {
            Ok(guard) => Claim::Refresh(guard),
            Err(_) if !known => Claim::Wait,
            Err(_) => Claim::Known,
        }
    }

    /// Whether the plan's allowance is due a reading as of `now`, marking it read if so.
    fn plan_due(&self, now: Instant) -> bool {
        if std::env::var(PLAN_USAGE_SWITCH).is_ok_and(|value| value.trim() == "0") {
            return false;
        }
        let mut read = self.plan_read.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let due = read.is_none_or(|at| now.saturating_duration_since(at) >= PLAN_FRESH_FOR);
        if due {
            *read = Some(now);
        }
        due
    }

    /// How much of the plan's monthly allowance is used, read with the account's GitHub sign-in.
    async fn read_plan(&self) -> Option<luxe_core::provider::PlanUsage> {
        let credential = self.auth.oauth_credential().await?;
        crate::auth::oauth::github_copilot::plan_usage(
            credential.refresh.expose(),
            credential.enterprise_domain.as_deref(),
        )
        .await
    }

    /// The `reasoning_effort` values the list last said `model` takes, if it said.
    async fn efforts(&self, model: &str) -> Option<Vec<String>> {
        let catalog = self.catalog.lock().await;
        catalog.as_ref().and_then(|known| known.efforts.get(model).cloned())
    }

    async fn fetch_list(&self) -> Option<Value> {
        let auth = self.auth.request_auth().await.ok()?;
        let base = endpoint(auth.base_url.as_deref(), self.base_url.as_deref());
        let url = format!("{base}/models");
        let request = crate::http::shared_client()
            .get(&url)
            .timeout(FETCH_TIMEOUT)
            .header("Accept", "application/json")
            .header("X-GitHub-Api-Version", MODELS_API_VERSION);
        let response = match auth.apply(request).send().await {
            Ok(response) if response.status().is_success() => response,
            Ok(response) => {
                debug!(status = %response.status(), "copilot model list refused");
                return None;
            }
            Err(error) => {
                debug!(%error, "copilot model list unavailable");
                return None;
            }
        };
        let body = response.json::<Value>().await.ok()?;
        debug!(models = routes(&body).len(), "copilot model routes");
        Some(body)
    }
}

#[async_trait::async_trait]
impl Provider for CopilotProvider {
    fn name(&self) -> &str {
        "github_copilot"
    }

    async fn stream(
        &self,
        req: CompletionRequest,
        cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<BoxStream<'static, ProviderEvent>, ProviderError> {
        let mut req = req;
        let route = self.route(&req.model.r#ref.id).await;
        if let Some(efforts) = self.efforts(&req.model.r#ref.id).await {
            adopt_efforts(&mut req.model, &efforts);
        }
        debug!(model = %req.model.r#ref.id, ?route, "copilot route");
        // Read beside the request rather than before it, so the reading costs the turn nothing
        let due = self.plan_due(Instant::now());
        let plan = async {
            match due {
                true => self.read_plan().await,
                false => None,
            }
        };
        let (plan, stream) = tokio::join!(plan, self.adapter(route).stream(req, cancel));
        let stream = stream?;
        Ok(match plan {
            Some(plan) => {
                let lead = futures::stream::once(async move { ProviderEvent::PlanUsage(plan) });
                Box::pin(lead.chain(stream))
            }
            None => stream,
        })
    }

    fn prompt_cache_ttl(&self, req: &CompletionRequest) -> Option<Duration> {
        self.adapter(self.known_route(&req.model.r#ref.id)).prompt_cache_ttl(req)
    }

    async fn warm_cache(&self, req: CompletionRequest) -> Result<Usage, ProviderError> {
        let route = self.route(&req.model.r#ref.id).await;
        self.adapter(route).warm_cache(req).await
    }
}

/// The host Copilot answers on: the credential's own, else the configured one, else the default.
fn endpoint(credential: Option<&str>, configured: Option<&str>) -> String {
    credential.or(configured).unwrap_or(DEFAULT_BASE_URL).trim_end_matches('/').to_string()
}

/// Auth for the Messages adapter: Copilot's own credential and headers, pointed at `/v1`.
///
/// The Anthropic adapter appends `/messages` to its base, the way Anthropic's own API is laid out,
/// while Copilot keeps Messages under `/v1` on a host that serves chat completions at its root. The
/// host comes from the credential (Copilot names a per-account endpoint), so the version segment has
/// to be added per request rather than baked into a URL at build time.
pub fn messages_auth(auth: Arc<dyn AuthSource>, base_url: Option<String>) -> Arc<dyn AuthSource> {
    Arc::new(UnderV1 { auth, base_url })
}

#[derive(Debug)]
struct UnderV1 {
    auth: Arc<dyn AuthSource>,
    base_url: Option<String>,
}

#[async_trait::async_trait]
impl AuthSource for UnderV1 {
    async fn request_auth(&self) -> Result<RequestAuth, ProviderError> {
        let mut auth = self.auth.request_auth().await?;
        let host = endpoint(auth.base_url.as_deref(), self.base_url.as_deref());
        auth.base_url = Some(format!("{host}/v1"));
        Ok(auth)
    }
}

/// Copilot's per-request headers: who started the call, what it is for, and whether it carries an
/// image.
///
/// Copilot expects the signals an editor's chat client sends, whichever wire API it is spoken in,
/// so every adapter that talks to it sends these from here and the three cannot drift. A call whose
/// last message is a tool result or an assistant reply is the agent continuing; anything else is the
/// person asking.
pub(crate) fn turn_headers(req: &CompletionRequest) -> Vec<(&'static str, &'static str)> {
    let initiator = match req.messages.last().map(|message| &message.role) {
        Some(luxe_core::message::Role::User) | None => "user",
        Some(_) => "agent",
    };
    let mut headers = vec![("X-Initiator", initiator), ("Openai-Intent", "conversation-edits")];
    let has_images = req.messages.iter().any(|message| {
        message.content.iter().any(|block| matches!(block, ContentBlock::Image { .. }))
    });
    if has_images {
        headers.push(("Copilot-Vision-Request", "true"));
    }
    headers
}

/// Whether `known` should be fetched again before routing `model` at `now`: never fetched, an hour
/// old, or silent on this model and not asked for five minutes.
fn stale(known: Option<&Catalog>, model: &str, now: Instant) -> bool {
    known.is_none_or(|known| {
        let age = now.saturating_duration_since(known.asked);
        age >= FRESH_FOR || (!known.routes.contains_key(model) && age >= RECHECK_UNKNOWN_AFTER)
    })
}

/// Each model's route, read off a `/models` body.
///
/// Each model goes to the richest protocol it takes. `/v1/messages` is Claude's, and the Anthropic
/// adapter keeps its thinking and cache breakpoints, which chat completions drops. `/responses`
/// carries a GPT's reasoning controls and its encrypted reasoning across tool calls, which is why
/// the two GPTs that take both go there. Chat completions is for the models that take nothing
/// else. A model naming none of them is left out and so stays on chat.
pub fn routes(body: &Value) -> HashMap<String, Route> {
    rows(body)
        .filter_map(|row| {
            let id = row.get("id").and_then(Value::as_str)?;
            let endpoints: Vec<&str> = row
                .get("supported_endpoints")
                .and_then(Value::as_array)?
                .iter()
                .filter_map(Value::as_str)
                .collect();
            let takes = |endpoint: &str| endpoints.contains(&endpoint);
            let route = if takes("/v1/messages") {
                Route::Messages
            } else if takes("/responses") {
                Route::Responses
            } else if takes("/chat/completions") {
                Route::Chat
            } else {
                return None;
            };
            Some((id.to_string(), route))
        })
        .collect()
}

/// The `reasoning_effort` values each model lists, read off a `/models` body.
pub fn efforts(body: &Value) -> HashMap<String, Vec<String>> {
    rows(body)
        .filter_map(|row| {
            let id = row.get("id").and_then(Value::as_str)?;
            let values: Vec<String> = row
                .pointer("/capabilities/supports/reasoning_effort")
                .and_then(Value::as_array)?
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect();
            (!values.is_empty()).then(|| (id.to_string(), values))
        })
        .collect()
}

fn rows(body: &Value) -> impl Iterator<Item = &Value> {
    body.get("data").and_then(Value::as_array).into_iter().flatten()
}

/// Holds `model`'s reasoning levels to the `reasoning_effort` values Copilot lists for it.
///
/// Copilot refuses any other value outright (`"medium" is not supported by model kimi-k3;
/// supported values: [low high max]`), and its list is the account's own and current, where the
/// catalog's may be neither. A level the list lacks clamps to one it has, the same way a catalog's
/// closed set does; `none` in the list is what says thinking can be switched off.
fn adopt_efforts(model: &mut ModelInfo, efforts: &[String]) {
    let offered = |wire: &str| efforts.iter().any(|value| value == wire);
    model.thinking_level_map = ThinkingLevel::ALL
        .iter()
        .filter(|level| **level != ThinkingLevel::Off)
        .map(|level| {
            let wire = match level {
                ThinkingLevel::Ultra => "max",
                other => other.as_str(),
            };
            (level.as_str().to_string(), offered(wire).then(|| wire.to_string()))
        })
        .collect();
    model.reasoning = true;
    model.thinking_controls.effort = true;
    model.thinking_controls.can_disable = offered("none");
}

#[cfg(test)]
mod tests;
