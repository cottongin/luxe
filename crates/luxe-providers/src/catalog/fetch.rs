//! Conditional models.dev fetch.
//!
//! The catalog is a few hundred KB gzipped and changes maybe daily, but it also serves an
//! `ETag`, so a revalidation that finds nothing new costs a `304` and zero bytes. Refreshing
//! often is therefore cheap; the only rule is that it never happens on a hot path.

use futures::StreamExt;

/// The public models.dev catalog endpoint.
pub const MODELS_DEV_API_URL: &str = "https://models.dev/api.json";

/// Hard ceiling on the decoded response body. The catalog is ~3 MB; this is remote, untrusted
/// JSON, so a server that streams forever gets cut off rather than exhausting memory.
const MAX_BODY_BYTES: usize = 32 * 1024 * 1024;

/// How long a single refresh may take, start to finish.
const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

/// Why a catalog refresh failed. Always non-fatal — the caller falls back to cache or snapshot.
#[derive(Debug, thiserror::Error)]
pub enum FetchError {
    /// The request could not be built or sent.
    #[error("models.dev request failed: {0}")]
    Request(#[from] reqwest::Error),
    /// The server answered with a non-success, non-304 status.
    #[error("models.dev returned {0}")]
    Status(reqwest::StatusCode),
    /// The body exceeded `MAX_BODY_BYTES`.
    #[error("models.dev response exceeded {MAX_BODY_BYTES} bytes")]
    TooLarge,
    /// The body was not valid UTF-8.
    #[error("models.dev response was not valid UTF-8")]
    NotUtf8,
}

/// The result of a conditional fetch.
#[derive(Debug)]
pub enum Fetched {
    /// The catalog changed; here is the new body and its entity tag.
    Changed {
        /// Raw `api.json` body.
        body: String,
        /// `ETag` to send back on the next revalidation.
        etag: Option<String>,
    },
    /// `304 Not Modified` — the cached copy is still current.
    Unchanged,
}

/// Fetch the catalog, revalidating against `etag` when one is known.
pub async fn fetch(url: &str, etag: Option<&str>) -> Result<Fetched, FetchError> {
    let client = reqwest::Client::builder()
        .timeout(TIMEOUT)
        .user_agent(concat!("luxe/", env!("CARGO_PKG_VERSION")))
        .build()?;
    let mut request = client.get(url);
    if let Some(etag) = etag {
        request = request.header(reqwest::header::IF_NONE_MATCH, etag);
    }

    let response = request.send().await?;
    if response.status() == reqwest::StatusCode::NOT_MODIFIED {
        return Ok(Fetched::Unchanged);
    }
    if !response.status().is_success() {
        return Err(FetchError::Status(response.status()));
    }

    let etag = response
        .headers()
        .get(reqwest::header::ETAG)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);

    // Bound the decoded stream rather than trusting Content-Length, which `gzip` strips anyway.
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        if body.len() + chunk.len() > MAX_BODY_BYTES {
            return Err(FetchError::TooLarge);
        }
        body.extend_from_slice(&chunk);
    }

    let body = String::from_utf8(body).map_err(|_| FetchError::NotUtf8)?;
    Ok(Fetched::Changed { body, etag })
}
