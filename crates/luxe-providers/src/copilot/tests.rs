use super::*;
use crate::auth::RequestAuth;
use crate::test_server::{http_reply, read_http_request};
use luxe_core::provider::{ModelInfo, ModelRef};
use std::io::Write;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Rows copied whole out of a live `GET /models` for a Copilot Pro account.
const MODELS: &str = include_str!("models.json");

/// Every shape the live list has: Responses only, both, chat and messages, chat only, a disabled
/// model, and a legacy one that names no endpoint at all. Claude lists Messages beside chat
/// completions and goes to Messages; a GPT that takes both chat and Responses goes to Responses.
#[test]
fn each_model_takes_the_route_copilot_lists_for_it() {
    let routes = routes(&serde_json::from_str(MODELS).unwrap());
    for (model, route) in [
        ("gpt-5.4-mini", Some(Route::Responses)),
        ("grok-4.5", Some(Route::Responses)),
        ("gpt-5.6-sol", Some(Route::Responses)),
        ("gpt-5-mini", Some(Route::Responses)),
        ("gpt-5.4", Some(Route::Responses)),
        ("claude-sonnet-5", Some(Route::Messages)),
        ("gemini-3.8-flash", Some(Route::Chat)),
        ("kimi-k3-base", Some(Route::Chat)),
        ("gpt-4o", None),
    ] {
        assert_eq!(routes.get(model).copied(), route, "{model}");
    }
}

/// A provider that only remembers which models it was asked for.
#[derive(Debug, Default)]
struct Recorder {
    asked: std::sync::Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl Provider for Recorder {
    fn name(&self) -> &str {
        "recorder"
    }

    async fn stream(
        &self,
        req: CompletionRequest,
        _cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<BoxStream<'static, ProviderEvent>, ProviderError> {
        self.asked.lock().unwrap().push(req.model.r#ref.id.clone());
        Ok(Box::pin(futures::stream::empty()))
    }
}

#[derive(Debug)]
struct At(String);

#[async_trait::async_trait]
impl AuthSource for At {
    async fn request_auth(&self) -> Result<RequestAuth, ProviderError> {
        Ok(RequestAuth { headers: Vec::new(), base_url: Some(self.0.clone()) })
    }
}

/// Serves `/models` from [`MODELS`] on a local port, counting how often it is asked.
fn serve_models() -> (String, Arc<AtomicUsize>) {
    serve(Vec::new())
}

/// Serves `/models`: each of `replies` once, as `(status, body)`, then [`MODELS`] from then on.
fn serve(replies: Vec<(u16, &'static str)>) -> (String, Arc<AtomicUsize>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let hits = Arc::new(AtomicUsize::new(0));
    let counted = hits.clone();
    std::thread::spawn(move || {
        for socket in listener.incoming() {
            let Ok(mut socket) = socket else { break };
            assert!(read_http_request(&mut socket).starts_with("GET /models "));
            let served = counted.fetch_add(1, Ordering::SeqCst);
            let (status, body) = replies.get(served).copied().unwrap_or((200, MODELS));
            let reply = http_reply(&format!("{status} X"), "application/json", body);
            let _ = socket.write_all(reply.as_bytes());
        }
    });
    (format!("http://127.0.0.1:{port}"), hits)
}

fn request(model: &str) -> CompletionRequest {
    CompletionRequest {
        model: ModelInfo { r#ref: ModelRef::new("github-copilot", model), ..Default::default() },
        ..Default::default()
    }
}

async fn send(copilot: &CopilotProvider, model: &str) {
    let (_cancel, cancelled) = tokio::sync::watch::channel(false);
    let _ = copilot.stream(request(model), cancelled).await.unwrap();
}

/// Three recorders standing in for Copilot's three adapters.
struct Recorders {
    chat: Arc<Recorder>,
    responses: Arc<Recorder>,
    messages: Arc<Recorder>,
}

impl Recorders {
    fn new() -> Self {
        Self {
            chat: Arc::new(Recorder::default()),
            responses: Arc::new(Recorder::default()),
            messages: Arc::new(Recorder::default()),
        }
    }

    fn copilot(&self, base: String) -> CopilotProvider {
        let adapters = CopilotAdapters {
            chat: self.chat.clone(),
            responses: self.responses.clone(),
            messages: self.messages.clone(),
        };
        CopilotProvider::new(adapters, Arc::new(At(base)), None)
    }
}

/// Each model reaches the adapter for the richest endpoint it takes — Claude to Messages, GPT and
/// Grok to Responses, Gemini to chat — from one fetch of the list however many requests follow.
#[tokio::test]
async fn requests_reach_the_endpoint_their_model_takes() {
    let (base, hits) = serve_models();
    let recorders = Recorders::new();
    let copilot = recorders.copilot(base);
    let (chat, responses, messages) = (&recorders.chat, &recorders.responses, &recorders.messages);
    for model in ["grok-4.5", "gpt-5-mini", "gpt-5.4-mini", "claude-sonnet-5", "gpt-5.4-mini"] {
        send(&copilot, model).await;
    }
    assert_eq!(
        *responses.asked.lock().unwrap(),
        ["grok-4.5", "gpt-5-mini", "gpt-5.4-mini", "gpt-5.4-mini"]
    );
    assert_eq!(*messages.asked.lock().unwrap(), ["claude-sonnet-5"]);
    send(&copilot, "gemini-3.8-flash").await;
    assert_eq!(*chat.asked.lock().unwrap(), ["gemini-3.8-flash"]);
    assert_eq!(hits.load(Ordering::SeqCst), 1, "the list is fetched once, not per request");

    // a model the list does not name stays on chat, and does not send luxe straight back to ask
    send(&copilot, "gpt-4o").await;
    assert_eq!(chat.asked.lock().unwrap().last().map(String::as_str), Some("gpt-4o"));
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}

/// With no list to be had, every model goes where every Copilot model went before — Claude too,
/// which chat completions serves.
#[tokio::test]
async fn without_the_list_everything_stays_on_chat() {
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", closed.local_addr().unwrap());
    drop(closed);
    let recorders = Recorders::new();
    let copilot = recorders.copilot(base);
    send(&copilot, "grok-4.5").await;
    send(&copilot, "claude-sonnet-5").await;
    assert_eq!(*recorders.chat.asked.lock().unwrap(), ["grok-4.5", "claude-sonnet-5"]);
    assert!(recorders.responses.asked.lock().unwrap().is_empty());
    assert!(recorders.messages.asked.lock().unwrap().is_empty());
}

/// The list is trusted for an hour, and a model it does not name sends luxe back to ask once it is
/// five minutes old, since that model may be new.
#[test]
fn the_list_is_asked_again_when_old_or_silent() {
    use std::time::Duration;
    let asked = Instant::now();
    let routes = HashMap::from([("grok-4.5".into(), Route::Responses)]);
    let known = Catalog { asked, routes, efforts: HashMap::new() };
    let at = |minutes: u64| asked + Duration::from_secs(minutes * 60);
    assert!(stale(None, "grok-4.5", asked), "never fetched");
    assert!(!stale(Some(&known), "grok-4.5", at(59)));
    assert!(stale(Some(&known), "grok-4.5", at(60)));
    assert!(!stale(Some(&known), "new-model", at(4)), "not straight back for every unknown id");
    assert!(stale(Some(&known), "new-model", at(5)));
}

/// A list that cannot be had when it is due again leaves the routes luxe knew in place; an error
/// Copilot answers in JSON is not a list with nothing in it.
#[tokio::test]
async fn a_failed_refresh_keeps_the_routes_it_had() {
    let (base, hits) = serve(vec![(200, MODELS), (500, r#"{"error":{"message":"try again"}}"#)]);
    let copilot = Recorders::new().copilot(base);
    let now = Instant::now();
    assert_eq!(copilot.route_at("grok-4.5", now).await, Route::Responses);
    let later = now + std::time::Duration::from_secs(2 * 60 * 60);
    assert_eq!(copilot.route_at("grok-4.5", later).await, Route::Responses);
    assert_eq!(hits.load(Ordering::SeqCst), 2, "it did ask again");
    assert_eq!(copilot.name(), "github_copilot");
}

/// Messages lives under `/v1` on whichever host the credential names, else the configured one,
/// else Copilot's default.
#[tokio::test]
async fn messages_are_asked_for_under_v1_on_the_credentials_host() {
    let base = |auth: Arc<dyn AuthSource>, configured: Option<&str>| {
        let configured = configured.map(str::to_string);
        async move { messages_auth(auth, configured).request_auth().await.unwrap().base_url }
    };
    let per_account = Arc::new(At("https://api.business.githubcopilot.com/".into()));
    assert_eq!(
        base(per_account, Some("https://ignored.example")).await.as_deref(),
        Some("https://api.business.githubcopilot.com/v1")
    );
    let unnamed: Arc<dyn AuthSource> = crate::auth::StaticAuth::bearer(None).into_arc();
    assert_eq!(
        base(unnamed.clone(), Some("https://proxy.example")).await.as_deref(),
        Some("https://proxy.example/v1")
    );
    assert_eq!(
        base(unnamed, None).await.as_deref(),
        Some("https://api.individual.githubcopilot.com/v1")
    );
}

/// A Claude turn through Copilot, end to end through the real Anthropic adapter: the list is read,
/// the call goes to `/v1/messages` on the credential's host with Copilot's bearer and its turn
/// headers, and the body carries the cache breakpoints chat completions would have dropped.
#[tokio::test]
async fn claude_reaches_copilots_messages_endpoint_with_its_headers_and_breakpoints() {
    use futures::StreamExt;

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let (seen_tx, seen_rx) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || {
        for socket in listener.incoming() {
            let Ok(mut socket) = socket else { break };
            let request = read_http_request(&mut socket);
            let (content_type, body) = if request.starts_with("GET /models ") {
                ("application/json", MODELS.to_string())
            } else {
                let _ = seen_tx.send(request);
                (
                    "text/event-stream",
                    "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n".to_string(),
                )
            };
            let reply = http_reply("200 OK", content_type, &body);
            let _ = socket.write_all(reply.as_bytes());
        }
    });

    let auth: Arc<dyn AuthSource> = Arc::new(Copilot(base));
    let compat = crate::auth::provider_descriptor("github-copilot").expect("in catalog").compat();
    let messages = crate::config::anthropic_provider(
        crate::config::ApiKey::default(),
        None,
        ModelInfo::default(),
        compat,
    )
    .with_auth(messages_auth(auth.clone(), None));
    let adapters = CopilotAdapters {
        chat: Arc::new(Recorder::default()),
        responses: Arc::new(Recorder::default()),
        messages: Arc::new(messages),
    };
    let copilot = CopilotProvider::new(adapters, auth, None);
    let mut turn = request("claude-sonnet-5");
    turn.model.max_tokens = 1024;
    turn.messages = vec![luxe_core::message::Message::user("hi")];
    turn.system = Some("be brief".into());
    let (_cancel, cancelled) = tokio::sync::watch::channel(false);
    let mut events = copilot.stream(turn, cancelled).await.unwrap();
    while events.next().await.is_some() {}

    let sent = seen_rx.recv_timeout(std::time::Duration::from_secs(5)).expect("a request");
    assert!(sent.starts_with("POST /v1/messages HTTP/1.1"), "{sent}");
    let header = |name: &str| {
        sent.lines().skip(1).take_while(|line| !line.trim().is_empty()).find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.trim().eq_ignore_ascii_case(name).then(|| value.trim().to_string())
        })
    };
    assert_eq!(header("authorization").as_deref(), Some("Bearer copilot-token"), "{sent}");
    assert_eq!(header("x-api-key"), None, "Copilot authenticates with its bearer");
    assert_eq!(header("x-initiator").as_deref(), Some("user"), "{sent}");
    assert_eq!(header("openai-intent").as_deref(), Some("conversation-edits"), "{sent}");
    assert_eq!(header("anthropic-version").as_deref(), Some("2023-06-01"), "{sent}");
    let body = sent.split("\r\n\r\n").nth(1).unwrap_or_default();
    assert!(body.contains("\"cache_control\""), "breakpoints reach the wire: {body}");
}

/// Copilot's credential: a bearer and the host it names.
#[derive(Debug)]
struct Copilot(String);

#[async_trait::async_trait]
impl AuthSource for Copilot {
    async fn request_auth(&self) -> Result<RequestAuth, ProviderError> {
        Ok(RequestAuth::bearer(&crate::config::ApiKey::new("copilot-token"))
            .with_base_url(self.0.clone()))
    }
}

/// The effort values a model lists are the only ones luxe sends it: Copilot refuses any other with
/// a 400. Values copied from the live list.
#[test]
fn a_models_reasoning_levels_are_the_ones_copilot_lists() {
    let efforts = efforts(&serde_json::from_str(MODELS).unwrap());
    let kimi = efforts.get("kimi-k3-base").expect("listed");
    assert_eq!(kimi, &["low", "high", "max"]);
    assert!(!efforts.contains_key("gpt-4o"), "a model listing none is left to the catalog");

    let mut model = ModelInfo::default();
    adopt_efforts(&mut model, kimi);
    assert!(model.reasoning && model.thinking_controls.effort);
    assert!(!model.thinking_controls.can_disable, "no `none`, so thinking cannot be switched off");
    assert_eq!(model.thinking_level_value(ThinkingLevel::Ultra), Some("max"));
    assert_eq!(model.effective_thinking_level(ThinkingLevel::Medium), ThinkingLevel::High);
    assert_eq!(model.thinking_level_value(ThinkingLevel::Medium), None);

    let mut gpt = ModelInfo::default();
    adopt_efforts(&mut gpt, efforts.get("gpt-5.4").expect("listed"));
    assert!(gpt.thinking_controls.can_disable, "`none` is listed");
    assert_eq!(gpt.thinking_level_value(ThinkingLevel::XHigh), Some("xhigh"));
}

/// Effort reaches a chat-completions model only as a value it takes, and never as `none` to one
/// that cannot stop thinking.
#[test]
fn chat_completions_effort_is_held_to_the_models_list() {
    let compat = crate::auth::provider_descriptor("github-copilot").expect("in catalog").compat();
    let adapter =
        crate::config::openai_compat_provider(crate::config::ProviderConfig::default(), compat);
    let mut kimi = request("kimi-k3");
    adopt_efforts(&mut kimi.model, &["low".into(), "high".into(), "max".into()]);
    let body = |thinking| {
        let mut turn = kimi.clone();
        turn.thinking = thinking;
        adapter.body_for_tests(&turn)
    };
    assert_eq!(body(ThinkingLevel::Ultra)["reasoning_effort"], "max");
    assert_eq!(body(ThinkingLevel::Medium)["reasoning_effort"], "high");
    assert!(body(ThinkingLevel::Off).get("reasoning_effort").is_none());
}

/// The plan's allowance is read once every five minutes however many requests a turn makes, and
/// not at all when switched off.
#[test]
fn the_plan_is_read_at_most_every_five_minutes() {
    let copilot = Recorders::new().copilot("http://127.0.0.1:9".into());
    let start = Instant::now();
    let at = |minutes: u64| start + std::time::Duration::from_secs(minutes * 60);
    assert!(copilot.plan_due(start), "never read");
    assert!(!copilot.plan_due(at(1)));
    assert!(!copilot.plan_due(at(4)));
    assert!(copilot.plan_due(at(5)));
    assert!(!copilot.plan_due(at(6)));
}

/// Serves the first `prompt` requests for the list at once, then holds each later one until
/// `release` says so.
fn serve_then_hold(prompt: usize) -> (String, Arc<AtomicUsize>, std::sync::mpsc::Sender<()>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let hits = Arc::new(AtomicUsize::new(0));
    let counted = hits.clone();
    let (release, released) = std::sync::mpsc::channel::<()>();
    std::thread::spawn(move || {
        for socket in listener.incoming() {
            let Ok(mut socket) = socket else { break };
            read_http_request(&mut socket);
            if counted.fetch_add(1, Ordering::SeqCst) >= prompt {
                let _ = released.recv();
            }
            let reply = http_reply("200 OK", "application/json", MODELS);
            let _ = socket.write_all(reply.as_bytes());
        }
    });
    (format!("http://127.0.0.1:{port}"), hits, release)
}

/// While the list is being fetched again, the routes already known still answer, and a second
/// request does not fetch it a second time.
///
/// The break this catches: the fetch ran under the catalog's lock, so for up to ten seconds the
/// cache warmer, which cannot wait, read no route at all and sent a Claude request to chat
/// completions.
#[tokio::test(flavor = "multi_thread")]
async fn a_refresh_in_flight_leaves_the_known_routes_readable() {
    let (base, hits, release) = serve_then_hold(1);
    let copilot = Arc::new(Recorders::new().copilot(base));
    let now = Instant::now();
    assert_eq!(copilot.route_at("claude-sonnet-5", now).await, Route::Messages);
    let later = now + std::time::Duration::from_secs(2 * 60 * 60);
    let refreshing = {
        let copilot = copilot.clone();
        tokio::spawn(async move { copilot.route_at("claude-sonnet-5", later).await })
    };
    while hits.load(Ordering::SeqCst) < 2 {
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }

    assert_eq!(copilot.known_route("claude-sonnet-5"), Route::Messages, "readable mid-fetch");
    assert_eq!(copilot.route_at("claude-sonnet-5", later).await, Route::Messages);
    release.send(()).unwrap();
    assert_eq!(refreshing.await.unwrap(), Route::Messages);
    assert_eq!(hits.load(Ordering::SeqCst), 2, "one refresh, not one per request");
}

/// Requests arriving while the first fetch of the list is under way wait for it, rather than
/// going down chat completions for want of a route.
///
/// The break this catches: the first request marked the list asked for and fetched it outside the
/// lock, so a second one found an empty list it was not due to refresh and sent Claude to chat.
#[tokio::test(flavor = "multi_thread")]
async fn requests_during_the_first_fetch_wait_for_its_routes() {
    let (base, hits, release) = serve_then_hold(0);
    let copilot = Arc::new(Recorders::new().copilot(base));
    let now = Instant::now();
    let first = {
        let copilot = copilot.clone();
        tokio::spawn(async move { copilot.route_at("claude-sonnet-5", now).await })
    };
    while hits.load(Ordering::SeqCst) < 1 {
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    let second = {
        let copilot = copilot.clone();
        tokio::spawn(async move { copilot.route_at("claude-sonnet-5", now).await })
    };
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert!(!second.is_finished(), "it waits for the list rather than guessing");
    release.send(()).unwrap();
    assert_eq!(first.await.unwrap(), Route::Messages);
    assert_eq!(second.await.unwrap(), Route::Messages);
    assert_eq!(hits.load(Ordering::SeqCst), 1, "one fetch for both");
}

/// A fetch abandoned partway, as a cancelled turn abandons it, leaves the list due: a request
/// that was waiting on it fetches it rather than giving up, and so does the next one.
///
/// The break this catches: the "asked" mark went on before the fetch, so a cancelled first fetch
/// left an empty list marked fresh, and every request for five minutes went to chat completions.
#[tokio::test(flavor = "multi_thread")]
async fn an_abandoned_fetch_leaves_the_list_due() {
    let (base, hits, release) = serve_then_hold(0);
    let copilot = Arc::new(Recorders::new().copilot(base));
    let now = Instant::now();
    let spawn_route = || {
        let copilot = copilot.clone();
        tokio::spawn(async move { copilot.route_at("claude-sonnet-5", now).await })
    };
    let abandoned = spawn_route();
    while hits.load(Ordering::SeqCst) < 1 {
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    let waiting = spawn_route();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    abandoned.abort();
    let _ = abandoned.await;
    // the server answers one connection at a time: let the abandoned one go first
    release.send(()).unwrap();
    let asked_again = async {
        while hits.load(Ordering::SeqCst) < 2 && !waiting.is_finished() {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    };
    tokio::time::timeout(std::time::Duration::from_secs(5), asked_again).await.unwrap();
    release.send(()).unwrap();

    assert_eq!(waiting.await.unwrap(), Route::Messages, "the waiter fetched it itself");
    assert_eq!(copilot.route_at("claude-sonnet-5", now).await, Route::Messages);
    assert_eq!(hits.load(Ordering::SeqCst), 2, "and nobody asked a third time");
}

/// A fetch that failed waits its turn: the next request does not ask again straight away.
#[tokio::test]
async fn a_failed_first_fetch_is_not_retried_at_once() {
    let (base, hits) = serve(vec![(500, r#"{"error":{"message":"try again"}}"#)]);
    let copilot = Recorders::new().copilot(base);
    let now = Instant::now();
    assert_eq!(copilot.route_at("grok-4.5", now).await, Route::Chat);
    assert_eq!(copilot.route_at("grok-4.5", now).await, Route::Chat);
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}
