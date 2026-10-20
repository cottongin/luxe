use super::*;
use luxe_providers::config::ApiKey;

#[test]
fn the_catalog_decides_which_adapter_a_known_provider_uses() {
    assert_eq!(resolve_api("anthropic", None), ApiKind::AnthropicMessages);
    assert_eq!(resolve_api("openai", None), ApiKind::OpenaiResponses);
    assert_eq!(resolve_api("openai-codex", None), ApiKind::OpenaiCodex);
    assert_eq!(resolve_api("groq", None), ApiKind::OpenaiCompletions);
    // luxe's historical default provider name is an alias of `openai`.
    assert_eq!(resolve_api("openai_compat", None), ApiKind::OpenaiResponses);
    // An unknown provider is assumed to speak the most common dialect.
    assert_eq!(resolve_api("some-proxy", None), ApiKind::OpenaiCompletions);
}

#[test]
fn an_explicit_api_in_config_wins_over_the_catalog() {
    let pinned = ProviderConfig { api: Some("openai-completions".into()), ..Default::default() };
    assert_eq!(resolve_api("openai", Some(&pinned)), ApiKind::OpenaiCompletions);
    // An unparseable value falls through rather than failing the launch.
    let nonsense = ProviderConfig { api: Some("carrier-pigeon".into()), ..Default::default() };
    assert_eq!(resolve_api("openai", Some(&nonsense)), ApiKind::OpenaiResponses);
}

/// The pre-catalog way of asking for `/responses` has to keep working for a user who set it.
#[test]
fn the_legacy_compat_flag_still_selects_the_responses_api() {
    let legacy = ProviderConfig {
        compat: CompatFlags { use_responses_api: true, ..Default::default() },
        ..Default::default()
    };
    assert_eq!(resolve_api("my-proxy", Some(&legacy)), ApiKind::OpenaiResponses);
}

#[test]
fn base_urls_and_compat_fall_back_to_the_catalog() {
    assert_eq!(
        resolve_base_url("anthropic", None).as_deref(),
        Some("https://api.anthropic.com/v1")
    );
    assert!(resolve_compat("anthropic", None).cache_control_format.is_some());

    // A user's own values win.
    let mine = ProviderConfig {
        base_url: Some("https://proxy.local/v1".into()),
        compat: CompatFlags::default(),
        ..Default::default()
    };
    assert_eq!(
        resolve_base_url("anthropic", Some(&mine)).as_deref(),
        Some("https://proxy.local/v1")
    );
    assert!(resolve_compat("anthropic", Some(&mine)).cache_control_format.is_none());

    // An unknown provider with no config has no base URL; the adapter's own default applies.
    assert!(resolve_base_url("mystery", None).is_none());
}

/// One header's value out of a raw request head, by exact name.
fn header_value(request: &str, name: &str) -> Option<String> {
    request.lines().skip(1).take_while(|line| !line.trim().is_empty()).find_map(|line| {
        let (header, value) = line.split_once(':')?;
        header.trim().eq_ignore_ascii_case(name).then(|| value.trim().to_string())
    })
}

/// Accepts one request, hands back its head, and answers with `body`.
fn serve_once(body: &'static str) -> (u16, std::sync::mpsc::Receiver<String>) {
    serve(body, 1)
}

/// Accepts `count` requests in turn, handing back each head and answering each with `body`.
fn serve(body: &'static str, count: usize) -> (u16, std::sync::mpsc::Receiver<String>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let (send, receive) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for _ in 0..count {
            answer(&listener, body, &send);
        }
    });
    (port, receive)
}

/// Accepts one request on `listener`, sends its head to `heads`, and answers with `body`.
fn answer(listener: &std::net::TcpListener, body: &str, heads: &std::sync::mpsc::Sender<String>) {
    use std::io::{Read, Write};

    let (mut socket, _) = listener.accept().expect("accept");
    let mut seen = Vec::new();
    let mut chunk = [0u8; 4096];
    // Read until the blank line that ends the head; the body follows on the same read.
    while !seen.windows(4).any(|window| window == b"\r\n\r\n") {
        match socket.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(read) => seen.extend_from_slice(&chunk[..read]),
        }
    }
    let _ = heads.send(String::from_utf8_lossy(&seen).to_string());
    let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n";
    let _ = socket.write_all(head.as_bytes());
    let _ = socket.write_all(body.as_bytes());
    let _ = socket.flush();
}

/// Drive one turn through `provider` and return the request it put on the wire.
async fn wire_request(
    provider: &std::sync::Arc<dyn Provider + Send + Sync>,
    model_id: &str,
    head: std::sync::mpsc::Receiver<String>,
) -> String {
    use futures::StreamExt;

    let request = luxe_core::provider::CompletionRequest {
        model: ModelInfo {
            r#ref: luxe_core::provider::ModelRef::new("foundry", model_id),
            max_tokens: 1024,
            ..ModelInfo::default()
        },
        messages: vec![luxe_core::message::Message::user("ping")],
        system: Some("be brief".into()),
        tools: Vec::new(),
        thinking: luxe_core::provider::ThinkingLevel::Off,
        max_tokens: Some(1024),
        ..Default::default()
    };
    let (_tx, cancel) = tokio::sync::watch::channel(false);
    if let Ok(mut stream) = provider.stream(request, cancel).await {
        while stream.next().await.is_some() {}
    }
    head.recv_timeout(std::time::Duration::from_secs(5)).expect("the adapter sent a request")
}

/// The header `name` that `provider` sends, or `None` when it sent no request at all (a
/// provider with no credential fails before reaching the wire).
fn sent_header(
    runtime: &tokio::runtime::Runtime,
    provider: &std::sync::Arc<dyn Provider + Send + Sync>,
    heads: &std::sync::mpsc::Receiver<String>,
    name: &str,
) -> Option<String> {
    let request = luxe_core::provider::CompletionRequest {
        model: ModelInfo { max_tokens: 1024, ..ModelInfo::default() },
        messages: vec![luxe_core::message::Message::user("ping")],
        max_tokens: Some(1024),
        ..Default::default()
    };
    let (_tx, cancel) = tokio::sync::watch::channel(false);
    runtime.block_on(async {
        use futures::StreamExt;
        if let Ok(mut stream) = provider.stream(request, cancel).await {
            while stream.next().await.is_some() {}
        }
    });
    let head = heads.recv_timeout(std::time::Duration::from_millis(500)).ok()?;
    header_value(&head, name)
}

/// The three endpoints a Foundry resource publishes, composed by the code that actually builds
/// the adapters. The paths are the ones on a deployment's Details tab; two of them share
/// `/openai/v1` and differ only in the adapter's own endpoint, and the Anthropic one
/// authenticates the Azure way while still speaking Anthropic's protocol — the exact pairing
/// that makes Foundry three providers instead of one.
#[tokio::test]
async fn each_foundry_surface_reaches_its_own_documented_endpoint() {
    for (id, model, expected_line, body) in [
        (
            "foundry-anthropic",
            "claude-sonnet-5",
            "POST /anthropic/v1/messages HTTP/1.1",
            "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
        ),
        (
            "foundry-openai",
            "gpt-5.6-sol",
            "POST /openai/v1/responses HTTP/1.1",
            "event: response.completed\ndata: {\"type\":\"response.completed\"}\n\n",
        ),
        (
            "foundry-completions",
            "deepseek-v4-pro",
            "POST /openai/v1/chat/completions HTTP/1.1",
            "data: {\"choices\":[{\"finish_reason\":\"stop\",\"delta\":{}}]}\n\ndata: [DONE]\n\n",
        ),
    ] {
        let descriptor = provider_descriptor(id).expect("in catalog");
        let (port, head) = serve_once(body);
        let base_url = descriptor.endpoint_for(&format!("http://127.0.0.1:{port}"));
        let auth = luxe_providers::auth::StaticAuth::for_style(
            resolve_key_style(id, None),
            Some(ApiKey::new("fk-live")),
        )
        .into_arc();
        let adapter = build_provider_adapter(
            resolve_api(id, None),
            ModelInfo::default(),
            auth,
            Some(base_url),
            resolve_compat(id, None),
        );

        let sent = wire_request(&adapter, model, head).await;
        assert!(sent.starts_with(expected_line), "{id} sent:\n{sent}");
        assert!(sent.contains(model), "{id} must send the deployment name:\n{sent}");

        // Header *names* are parsed rather than matched as substrings: `x-api-key` contains
        // `api-key`, so a substring check passes whether or not the second header was sent.
        let sent_header = |name: &str| header_value(&sent, name);
        match id {
            "foundry-anthropic" => {
                assert_eq!(sent_header("x-api-key").as_deref(), Some("fk-live"), "{sent}");
                assert_eq!(sent_header("api-key").as_deref(), Some("fk-live"), "{sent}");
                assert_eq!(sent_header("authorization"), None, "a bearer 401s here:\n{sent}");
                assert_eq!(
                    sent_header("anthropic-version").as_deref(),
                    Some("2023-06-01"),
                    "{sent}"
                );
            }
            _ => {
                assert_eq!(
                    sent_header("authorization").as_deref(),
                    Some("Bearer fk-live"),
                    "{sent}"
                );
                assert_eq!(sent_header("x-api-key"), None, "{sent}");
                assert_eq!(sent_header("api-key"), None, "{sent}");
            }
        }
    }
}

/// The launch provider's `!command` key runs for each request, not once at startup, so a
/// short-lived token (Entra, `az account get-access-token`) is fresh for every turn of a long
/// session. `cat` stands in for the token command: the file changes between two requests.
#[cfg(unix)]
#[tokio::test]
async fn the_launch_providers_command_key_runs_for_each_request() {
    let (_dir, token, auth) = {
        let _guard = super::super::test_env::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = super::super::test_env::isolated_env();
        let token = dir.path().join("token");
        std::fs::write(&token, "token-one").expect("token");
        std::fs::create_dir_all(dir.path().join("luxe")).expect("config dir");
        let models = serde_json::json!({
            "providers": { "anthropic": { "api_key": format!("!cat {}", token.display()) } }
        });
        std::fs::write(dir.path().join("luxe").join("models.json"), models.to_string())
            .expect("models.json");
        let overrides =
            super::super::CliOverrides { provider: Some("anthropic".into()), ..Default::default() };
        let settings = AgentSettings::resolve(dir.path().to_path_buf(), overrides);
        let config = load_luxe_config();
        let auth = active_auth(&settings, config.providers.get("anthropic"));
        (dir, token, auth)
    };
    let key = |auth: luxe_providers::auth::RequestAuth| {
        auth.headers.into_iter().find(|(name, _)| name == "x-api-key").map(|(_, key)| key)
    };

    let first = key(auth.request_auth().await.expect("first request"));
    std::fs::write(&token, "token-two").expect("token");
    let second = key(auth.request_auth().await.expect("second request"));
    assert_eq!(first.as_ref().map(ApiKey::expose), Some("token-one"));
    assert_eq!(second.as_ref().map(ApiKey::expose), Some("token-two"), "run again, not cached");
}

/// Launch and a `/model` switch find a provider's key by one chain: a sign-in first, then the
/// provider's own variable, then the key its block names, and for a provider luxe does not know,
/// its block before `OPENAI_API_KEY`. So a configured key always beats a key meant for another
/// provider, and which one goes out never depends on how the session reached the provider.
#[test]
fn launch_and_a_model_switch_send_the_same_key() {
    let _guard = super::super::test_env::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = super::super::test_env::isolated_env();
    let runtime =
        tokio::runtime::Builder::new_current_thread().enable_all().build().expect("runtime");
    let completions =
        "data: {\"choices\":[{\"finish_reason\":\"stop\",\"delta\":{}}]}\n\ndata: [DONE]\n\n";
    let messages = "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";
    let (gateway_port, gateway_heads) = serve(completions, 2);
    let (keyed_port, keyed_heads) = serve(completions, 2);
    let (anthropic_port, anthropic_heads) = serve(messages, 2);
    std::fs::create_dir_all(home.path().join("luxe")).expect("config dir");
    let models = serde_json::json!({ "providers": {
        "gateway": { "base_url": format!("http://127.0.0.1:{gateway_port}/v1") },
        "keyed": { "base_url": format!("http://127.0.0.1:{keyed_port}/v1"), "api_key": "keyed-key" },
        "anthropic": { "base_url": format!("http://127.0.0.1:{anthropic_port}/v1") },
    }});
    std::fs::write(home.path().join("luxe").join("models.json"), models.to_string())
        .expect("models.json");
    AuthStore::default_path()
        .put("anthropic", &luxe_providers::auth::Credential::api_key("sk-ant-signed-in"))
        .expect("sign in");
    std::env::set_var("OPENAI_API_KEY", "sk-openai");
    std::env::set_var("ANTHROPIC_API_KEY", "sk-ant-env");

    let launched_on = |provider: &str| {
        let overrides =
            super::super::CliOverrides { provider: Some(provider.into()), ..Default::default() };
        AgentSettings::resolve(home.path().to_path_buf(), overrides)
    };
    let elsewhere = launched_on("lmstudio");
    let registry = build_provider_registry(&elsewhere, &build_active_adapter(&elsewhere));
    let mut sent = Vec::new();
    for (provider, heads, header) in [
        ("gateway", &gateway_heads, "authorization"),
        ("keyed", &keyed_heads, "authorization"),
        ("anthropic", &anthropic_heads, "x-api-key"),
    ] {
        let at_launch = build_active_adapter(&launched_on(provider));
        let launch = sent_header(&runtime, &at_launch, heads, header);
        let switched = sent_header(&runtime, &registry[provider], heads, header);
        sent.push((provider, launch, switched));
    }
    std::env::remove_var("OPENAI_API_KEY");
    std::env::remove_var("ANTHROPIC_API_KEY");

    let both = |key: &str| (Some(key.to_string()), Some(key.to_string()));
    let sent: Vec<_> =
        sent.into_iter().map(|(name, launch, switch)| (name, (launch, switch))).collect();
    assert_eq!(
        sent,
        [
            ("gateway", both("Bearer sk-openai")),
            ("keyed", both("Bearer keyed-key")),
            ("anthropic", both("sk-ant-signed-in")),
        ]
    );
}

/// Which key each kind of provider settles on with nothing stored: a provider luxe does not know
/// takes the key written for it before the `OPENAI_API_KEY` fallback, and one it knows takes its
/// own variable first.
#[test]
fn a_configured_key_beats_the_openai_fallback_for_an_unknown_provider() {
    let _guard = super::super::test_env::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = super::super::test_env::isolated_env();
    let runtime =
        tokio::runtime::Builder::new_current_thread().enable_all().build().expect("runtime");
    let store = AuthStore::new(home.path().join("auth.json"));
    let key = |provider: &str, config: Option<&ProviderConfig>| {
        let auth = provider_auth(store.clone(), provider, config);
        let resolved = runtime.block_on(auth.request_auth()).ok()?;
        resolved.headers.into_iter().next().map(|(_, key)| key.expose().to_string())
    };
    std::env::set_var("OPENAI_API_KEY", "sk-openai-fallback");
    let written = ProviderConfig { api_key: Some(ApiKey::new("gw-key")), ..Default::default() };
    let gateway = key("gateway", Some(&written));
    let unwritten = key("gateway", None);
    let openai = key("openai", Some(&written));
    std::env::remove_var("OPENAI_API_KEY");
    assert_eq!(gateway.as_deref(), Some("Bearer gw-key"), "the configured key");
    assert_eq!(unwritten.as_deref(), Some("Bearer sk-openai-fallback"), "nothing configured");
    assert_eq!(
        openai.as_deref(),
        Some("Bearer sk-openai-fallback"),
        "a known provider's own variable"
    );
}
