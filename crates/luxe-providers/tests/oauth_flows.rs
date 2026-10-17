//! OAuth flows end to end against a local mock server.
//!
//! The unit tests cover the pieces (URL building, JWT claims, poll classification); these drive the
//! real code paths over a real socket, so a wrong verb, a missing form field, or a mis-parsed
//! response fails here rather than in front of a user. Nothing in this file touches the network:
//! every flow is pointed at a `127.0.0.1` listener whose scripted replies are the shapes the
//! providers actually send.

#![allow(clippy::unwrap_used)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use luxe_providers::auth::credential::OAuthCredential;
use luxe_providers::auth::oauth::{github_copilot, openai_codex, openrouter};
use luxe_providers::auth::Credential;
use luxe_providers::config::ApiKey;

/// A request the mock server saw.
#[derive(Debug, Clone)]
struct Seen {
    method: String,
    path: String,
    body: String,
    authorization: Option<String>,
}

/// A scripted local HTTP server.
///
/// Replies come from a per-path queue, so a poll can answer "pending" once and then succeed —
/// which is the behavior that matters for a device flow.
struct MockServer {
    base: String,
    seen: Arc<Mutex<Vec<Seen>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl MockServer {
    /// Serve `replies` (path → queued `(status, body)` responses) on a free loopback port.
    async fn start(replies: HashMap<String, Vec<(u16, String)>>) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        let queues = Arc::new(Mutex::new(replies));

        let task = tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else { return };
                let Some(request) = read_request(&mut socket).await else { continue };
                log.lock().unwrap().push(request.clone());
                let (status, body) = queues
                    .lock()
                    .unwrap()
                    .get_mut(&request.path)
                    .and_then(|queue| {
                        // The last scripted reply repeats, so a poller can keep polling.
                        if queue.len() > 1 {
                            Some(queue.remove(0))
                        } else {
                            queue.first().cloned()
                        }
                    })
                    .unwrap_or_else(|| (404, "{}".to_string()));
                respond(&mut socket, status, &body).await;
            }
        });
        Self { base, seen, task }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    fn requests(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }

    fn request_to(&self, path: &str) -> Seen {
        self.requests()
            .into_iter()
            .find(|request| request.path == path)
            .unwrap_or_else(|| panic!("no request to {path}; saw {:?}", self.requests()))
    }
}

async fn read_request(socket: &mut tokio::net::TcpStream) -> Option<Seen> {
    use tokio::io::AsyncReadExt;
    let mut buffer = vec![0u8; 16 * 1024];
    let read = socket.read(&mut buffer).await.ok()?;
    let text = String::from_utf8_lossy(&buffer[..read]).to_string();
    let mut lines = text.lines();
    let start = lines.next()?;
    let mut parts = start.split_whitespace();
    let method = parts.next()?.to_string();
    let target = parts.next()?.to_string();
    let path = target.split('?').next().unwrap_or(&target).to_string();
    let authorization = text
        .lines()
        .find(|line| line.to_ascii_lowercase().starts_with("authorization:"))
        .map(|line| line[14..].trim().to_string());
    let body = text.split_once("\r\n\r\n").map(|(_, body)| body.to_string()).unwrap_or_default();
    Some(Seen { method, path, body, authorization })
}

async fn respond(socket: &mut tokio::net::TcpStream, status: u16, body: &str) {
    use tokio::io::AsyncWriteExt;
    let response = format!(
        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = socket.write_all(response.as_bytes()).await;
    let _ = socket.flush().await;
}

fn replies(pairs: &[(&str, &[(u16, &str)])]) -> HashMap<String, Vec<(u16, String)>> {
    pairs
        .iter()
        .map(|(path, queue)| {
            let queue = queue.iter().map(|(status, body)| (*status, (*body).to_string())).collect();
            ((*path).to_string(), queue)
        })
        .collect()
}

/// A ChatGPT access token carrying the account-id claim the Codex backend requires.
fn chatgpt_token(account: &str) -> String {
    let claims =
        serde_json::json!({ "https://api.openai.com/auth": { "chatgpt_account_id": account } });
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(serde_json::to_vec(&claims).unwrap());
    format!("header.{payload}.signature")
}

// === OpenAI Codex ===

#[tokio::test]
async fn codex_exchanges_an_authorization_code_for_a_credential() {
    let access = chatgpt_token("acct-42");
    let token_body = serde_json::json!({
        "access_token": access,
        "refresh_token": "refresh-1",
        "expires_in": 3600,
    })
    .to_string();
    let server =
        MockServer::start(replies(&[("/oauth/token", &[(200, token_body.as_str())])])).await;
    let endpoints = openai_codex::Endpoints::at(&server.url(""));

    let credential = openai_codex::exchange_code(
        &endpoints,
        "the-code",
        "the-verifier",
        "http://localhost:1455/auth/callback",
    )
    .await
    .expect("exchanges");

    assert_eq!(credential.account.as_deref(), Some("acct-42"));
    assert_eq!(credential.refresh.expose(), "refresh-1");
    assert!(credential.expires_at_ms > 0);

    // The wire contract: a form POST carrying the PKCE verifier and the redirect it was issued for.
    let request = server.request_to("/oauth/token");
    assert_eq!(request.method, "POST");
    for expected in [
        "grant_type=authorization_code",
        "code=the-code",
        "code_verifier=the-verifier",
        "client_id=",
    ] {
        assert!(request.body.contains(expected), "missing {expected} in {}", request.body);
    }
}

#[tokio::test]
async fn codex_refresh_keeps_the_old_token_when_the_response_omits_one() {
    let body = serde_json::json!({
        "access_token": chatgpt_token("acct-42"),
        "expires_in": 3600,
    })
    .to_string();
    let server = MockServer::start(replies(&[("/oauth/token", &[(200, body.as_str())])])).await;
    let endpoints = openai_codex::Endpoints::at(&server.url(""));

    let existing = OAuthCredential {
        access: ApiKey::new("stale"),
        refresh: ApiKey::new("keep-me"),
        expires_at_ms: 1,
        ..Default::default()
    };
    let refreshed = openai_codex::refresh_with(&endpoints, &existing).await.expect("refreshes");

    assert_eq!(
        refreshed.refresh.expose(),
        "keep-me",
        "a provider that doesn't rotate the refresh token must not lose it"
    );
    assert!(server.request_to("/oauth/token").body.contains("grant_type=refresh_token"));
}

#[tokio::test]
async fn codex_reports_the_providers_own_message_when_a_token_is_refused() {
    let body =
        serde_json::json!({ "error": "invalid_grant", "error_description": "expired" }).to_string();
    let server = MockServer::start(replies(&[("/oauth/token", &[(400, body.as_str())])])).await;
    let endpoints = openai_codex::Endpoints::at(&server.url(""));

    let error = openai_codex::exchange_code(&endpoints, "c", "v", "http://localhost/cb")
        .await
        .expect_err("must fail");
    assert!(error.to_string().contains("expired"), "unhelpful: {error}");
}

// === GitHub Copilot ===

#[tokio::test]
async fn copilot_exchanges_a_github_token_and_learns_its_endpoint() {
    let expires_at = (luxe_providers::auth::credential::now_ms() / 1000) + 3600;
    let body = serde_json::json!({
        "token": "tid=abc;exp=1;proxy-ep=proxy.business.githubcopilot.com",
        "expires_at": expires_at,
    })
    .to_string();
    let server =
        MockServer::start(replies(&[("/copilot_internal/v2/token", &[(200, body.as_str())])]))
            .await;
    // `Endpoints` builds `https://api.{domain}/…`; point every field at the mock instead.
    let endpoints = github_copilot::Endpoints {
        device_code: server.url("/login/device/code"),
        access_token: server.url("/login/oauth/access_token"),
        copilot_token: server.url("/copilot_internal/v2/token"),
    };

    let credential = github_copilot::exchange_copilot_token(&endpoints, "gho_token", None)
        .await
        .expect("exchanges");

    assert_eq!(credential.refresh.expose(), "gho_token", "the GitHub token is the refresh token");
    assert_eq!(
        credential.endpoint.as_deref(),
        Some("https://api.business.githubcopilot.com"),
        "the account's proxy endpoint decides where requests go"
    );
    assert!(credential.expires_at_ms > 0);

    // Copilot authenticates this call with the GitHub token and requires its editor headers.
    let request = server.request_to("/copilot_internal/v2/token");
    assert_eq!(request.authorization.as_deref(), Some("Bearer gho_token"));
}

#[tokio::test]
async fn copilot_explains_a_missing_subscription_rather_than_echoing_a_403() {
    let server = MockServer::start(replies(&[(
        "/copilot_internal/v2/token",
        &[(403, r#"{"message":"forbidden"}"#)],
    )]))
    .await;
    let endpoints = github_copilot::Endpoints {
        device_code: server.url("/login/device/code"),
        access_token: server.url("/login/oauth/access_token"),
        copilot_token: server.url("/copilot_internal/v2/token"),
    };

    let error = github_copilot::exchange_copilot_token(&endpoints, "gho_token", None)
        .await
        .expect_err("must fail");
    assert!(
        error.to_string().contains("Copilot subscription"),
        "the error should name the actual problem: {error}"
    );
}

// === OpenRouter ===

#[tokio::test]
async fn openrouter_mints_an_api_key_from_the_callback_code() {
    let server = MockServer::start(replies(&[(
        "/api/v1/auth/keys",
        &[(200, r#"{"key":"sk-or-v1-minted"}"#)],
    )]))
    .await;
    let endpoints = openrouter::Endpoints {
        authorize: server.url("/auth"),
        keys: server.url("/api/v1/auth/keys"),
    };

    // Drive the flow the way the UI does: it opens a URL, then the loopback callback answers.
    let interaction = CallbackDriver::new();
    let credential = openrouter::login_with(&endpoints, &interaction).await.expect("mints a key");

    match credential {
        Credential::ApiKey(api) => assert_eq!(api.key.expose(), "sk-or-v1-minted"),
        Credential::OAuth(_) => panic!("OpenRouter mints a durable key, not a token pair"),
    }
    let request = server.request_to("/api/v1/auth/keys");
    assert!(request.body.contains("code_verifier"), "PKCE verifier must be sent: {}", request.body);
    assert!(request.body.contains("\"code\""), "the callback code must be sent: {}", request.body);
}

/// An interaction that completes a loopback flow by fetching the redirect URL itself.
///
/// This is the browser's half of the handshake: the flow publishes an authorization URL carrying a
/// `callback_url`, and something has to actually hit that callback.
struct CallbackDriver {
    events: Mutex<Vec<luxe_core::auth::AuthEvent>>,
}

impl CallbackDriver {
    fn new() -> Self {
        Self { events: Mutex::new(Vec::new()) }
    }
}

#[async_trait::async_trait]
impl luxe_core::auth::AuthInteraction for CallbackDriver {
    async fn prompt(
        &self,
        _prompt: luxe_core::auth::AuthPrompt,
    ) -> Result<secrecy::SecretString, luxe_core::auth::AuthError> {
        // The paste fallback never wins here: the callback does. Wait rather than answering, so the
        // test exercises the loopback path and the race between them.
        std::future::pending().await
    }

    fn notify(&self, event: luxe_core::auth::AuthEvent) {
        if let luxe_core::auth::AuthEvent::OpenUrl { url, .. } = &event {
            // Extract the callback the flow asked the browser to return to, and hit it.
            if let Ok(parsed) = reqwest::Url::parse(url) {
                if let Some(callback) = parsed
                    .query_pairs()
                    .find(|(key, _)| key == "callback_url")
                    .map(|(_, value)| value.into_owned())
                {
                    tokio::spawn(async move {
                        // a page guessing the fixed path first must not end the sign-in
                        let mut guessed = reqwest::Url::parse(&callback).unwrap();
                        guessed.set_path("/openrouter/callback");
                        guessed.set_query(Some("error=forged"));
                        let forged = reqwest::get(guessed).await.unwrap();
                        assert_eq!(forged.status().as_u16(), 404, "only the issued path answers");
                        let redirect = format!("{callback}?code=the-code");
                        let _ = reqwest::get(&redirect).await;
                    });
                }
            }
        }
        self.events.lock().unwrap().push(event);
    }
}

// === GitHub Copilot: the login's question order ===

/// A scripted interaction that records the prompts it was asked, in order.
struct PromptRecorder {
    answers: Mutex<Vec<String>>,
    asked: Arc<Mutex<Vec<luxe_core::auth::AuthPrompt>>>,
}

impl PromptRecorder {
    fn new(answers: &[&str]) -> (Self, Arc<Mutex<Vec<luxe_core::auth::AuthPrompt>>>) {
        let asked = Arc::new(Mutex::new(Vec::new()));
        let recorder = Self {
            answers: Mutex::new(answers.iter().rev().map(|a| a.to_string()).collect()),
            asked: asked.clone(),
        };
        (recorder, asked)
    }
}

#[async_trait::async_trait]
impl luxe_core::auth::AuthInteraction for PromptRecorder {
    async fn prompt(
        &self,
        prompt: luxe_core::auth::AuthPrompt,
    ) -> Result<secrecy::SecretString, luxe_core::auth::AuthError> {
        self.asked.lock().unwrap().push(prompt);
        match self.answers.lock().unwrap().pop() {
            Some(answer) => Ok(secrecy::SecretString::from(answer)),
            // Out of scripted answers: stop the flow before it reaches the network.
            None => Err(luxe_core::auth::AuthError::Cancelled),
        }
    }

    fn notify(&self, _event: luxe_core::auth::AuthEvent) {}
}

/// Picking "GitHub" must not ask for a domain — that question only exists for Enterprise.
#[tokio::test]
async fn copilot_asks_which_github_before_anything_else() {
    let (recorder, asked) = PromptRecorder::new(&["github"]);
    let domain = github_copilot::choose_domain(&recorder).await.expect("resolves");
    assert_eq!(domain, None, "github.com is the default, expressed as no enterprise domain");

    let asked = asked.lock().unwrap();
    assert_eq!(asked.len(), 1, "github.com needs exactly one question: {asked:?}");
    match &asked[0].kind {
        luxe_core::auth::AuthPromptKind::Select(choices) => {
            let ids: Vec<&str> = choices.iter().map(|choice| choice.id.as_str()).collect();
            assert_eq!(ids, vec!["github", "enterprise"], "GitHub leads as the default");
        }
        other => panic!("expected a choice, got {other:?}"),
    }
}

/// Picking Enterprise asks for the domain, and only then.
#[tokio::test]
async fn copilot_asks_for_a_domain_only_when_enterprise_is_chosen() {
    let (recorder, asked) = PromptRecorder::new(&["enterprise", "company.ghe.com"]);
    let domain = github_copilot::choose_domain(&recorder).await.expect("resolves");
    assert_eq!(domain.as_deref(), Some("company.ghe.com"));

    let asked = asked.lock().unwrap();
    assert_eq!(asked.len(), 2, "{asked:?}");
    assert!(asked[1].message.contains("domain"), "{:?}", asked[1]);
    assert!(
        !asked[1].allow_empty,
        "having chosen Enterprise, an empty domain is a mistake rather than a default"
    );
}

/// An Enterprise choice with an unusable domain fails with a reason instead of silently using
/// github.com — which would send the credential to the wrong host.
#[tokio::test]
async fn copilot_refuses_an_unusable_enterprise_domain() {
    let (recorder, _asked) = PromptRecorder::new(&["enterprise", "not a domain"]);
    let error = github_copilot::choose_domain(&recorder).await.expect_err("must refuse");
    assert!(matches!(error, luxe_core::auth::AuthError::Input(_)), "got {error:?}");
}

/// The whole device flow over a socket: issue a code, poll once as pending, then authorize and
/// exchange. This is the path a user walks, minus GitHub.
#[tokio::test]
async fn copilot_device_flow_runs_end_to_end() {
    let expires_at = (luxe_providers::auth::credential::now_ms() / 1000) + 1800;
    let token_body = serde_json::json!({
        "token": "tid=abc;proxy-ep=proxy.individual.githubcopilot.com",
        "expires_at": expires_at,
    })
    .to_string();
    let server = MockServer::start(replies(&[
        (
            "/login/device/code",
            &[(
                200,
                r#"{"device_code":"dev-1","user_code":"WXYZ-9876",
                    "verification_uri":"https://github.com/login/device",
                    "interval":1,"expires_in":900}"#,
            )],
        ),
        (
            "/login/oauth/access_token",
            &[
                // GitHub answers a not-yet-authorized poll with HTTP 200 and an error field.
                (200, r#"{"error":"authorization_pending"}"#),
                (200, r#"{"access_token":"gho_authorized"}"#),
            ],
        ),
        ("/copilot_internal/v2/token", &[(200, token_body.as_str())]),
    ]))
    .await;
    let endpoints = github_copilot::Endpoints {
        device_code: server.url("/login/device/code"),
        access_token: server.url("/login/oauth/access_token"),
        copilot_token: server.url("/copilot_internal/v2/token"),
    };

    let recorder = EventRecorder::default();
    let credential =
        github_copilot::run_device_flow(&endpoints, None, &recorder).await.expect("completes");

    assert_eq!(credential.refresh.expose(), "gho_authorized");
    assert_eq!(credential.endpoint.as_deref(), Some("https://api.individual.githubcopilot.com"));
    // The user has to be shown the code and where to type it.
    let codes: Vec<String> = recorder
        .events
        .lock()
        .unwrap()
        .iter()
        .filter_map(|event| match event {
            luxe_core::auth::AuthEvent::DeviceCode { user_code, .. } => Some(user_code.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(codes, vec!["WXYZ-9876".to_string()]);
    // And the pending poll must not have aborted the flow.
    assert!(
        server.requests().iter().filter(|r| r.path == "/login/oauth/access_token").count() >= 2,
        "the poller should have retried after `authorization_pending`"
    );
}

/// Records events; answers no prompts (this flow asks none once the domain is settled).
#[derive(Default)]
struct EventRecorder {
    events: Mutex<Vec<luxe_core::auth::AuthEvent>>,
}

#[async_trait::async_trait]
impl luxe_core::auth::AuthInteraction for EventRecorder {
    async fn prompt(
        &self,
        _prompt: luxe_core::auth::AuthPrompt,
    ) -> Result<secrecy::SecretString, luxe_core::auth::AuthError> {
        Err(luxe_core::auth::AuthError::Cancelled)
    }

    fn notify(&self, event: luxe_core::auth::AuthEvent) {
        self.events.lock().unwrap().push(event);
    }
}
