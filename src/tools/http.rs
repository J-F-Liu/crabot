//! Shared HTTP client and body reader for `fetch` and custom HTTP tools.

use std::sync::LazyLock;

use futures::StreamExt;
use tokio_util::sync::CancellationToken;

use super::CANCEL_REASON;

/// Shared client with no client timeout or proxy fallback when tools proxy is off.
pub(crate) fn client() -> Result<&'static reqwest::Client, String> {
    static CLIENT: LazyLock<Result<reqwest::Client, String>> = LazyLock::new(|| {
        let mut builder = reqwest::Client::builder().user_agent(crate::app_title());
        if !crate::tools::tools_proxy_active() {
            builder = builder.no_proxy();
        }
        builder
            .build()
            .map_err(|e| format!("Failed to build HTTP client: {e}"))
    });
    CLIENT.as_ref().map_err(Clone::clone)
}

/// Accept only HTTP(S) URLs; both web tools share this policy.
pub(crate) fn ensure_http_url(url: &reqwest::Url) -> Result<(), String> {
    match url.scheme() {
        "http" | "https" => Ok(()),
        scheme => Err(format!(
            "Unsupported URL scheme '{scheme}' (only http and https are allowed)"
        )),
    }
}

/// Read a response body with a hard byte cap, racing user cancellation.
pub(crate) async fn read_body(
    resp: reqwest::Response,
    max_bytes: usize,
    cancel: &CancellationToken,
) -> Result<Vec<u8>, String> {
    if let Some(len) = resp.content_length()
        && len > max_bytes as u64
    {
        return Err(format!(
            "Response body exceeds {max_bytes} bytes (got {len})"
        ));
    }

    // Chunked responses must not grow memory unbounded.
    let mut body: Vec<u8> = Vec::with_capacity(max_bytes.min(1 << 20));
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = tokio::select! {
        biased;
        _ = cancel.cancelled() => return Err(CANCEL_REASON.into()),
        chunk = stream.next() => chunk,
    } {
        let chunk = chunk.map_err(|e| format!("Failed to read response body: {e}"))?;
        if body.len() + chunk.len() > max_bytes {
            return Err(format!("Response body exceeds {max_bytes} bytes"));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}
