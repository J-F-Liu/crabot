//! Genai client construction with custom auth, endpoint, and adapter kind.

use std::sync::{Arc, LazyLock};

use genai::adapter::AdapterKind;
use genai::resolver::{AuthData, Endpoint, ServiceTargetResolver};
use genai::{Client, ModelIden, ServiceTarget};

use crabot::model::resolve_api_key;

/// Build a genai `Client` with custom auth, endpoint, and adapter kind.
pub(super) fn build_client(base_url: &str, api_key: &str, api_type: &str) -> Client {
    let adapter_kind = AdapterKind::from_lower_str(api_type).unwrap_or(AdapterKind::OpenAI);
    let has_custom_endpoint = !base_url.is_empty();
    let has_custom_key = !api_key.is_empty();

    let mut builder = Client::builder();
    // LLM proxy off → no_proxy client so the registry/env proxy can't route LLM traffic.
    if !crate::tools::llm_proxy_enabled() {
        builder = builder.with_reqwest(direct_client().clone());
    }

    if !has_custom_endpoint && !has_custom_key {
        return builder.build();
    }

    let mut base_url = base_url.to_string();
    // Ensure trailing slash so genai's URL join appends rather than replaces
    // the last path segment (e.g. "/v1/" + "chat/completions" → "/v1/chat/completions").
    if !base_url.ends_with('/') {
        base_url.push('/');
    }

    let api_key = resolve_api_key(api_key);

    let target_resolver = ServiceTargetResolver::from_resolver_fn(
        move |target: ServiceTarget| -> Result<ServiceTarget, genai::resolver::Error> {
            let ServiceTarget {
                endpoint: default_endpoint,
                auth: default_auth,
                model,
            } = target;

            let endpoint = if has_custom_endpoint {
                Endpoint::from_owned(Arc::from(base_url.as_str()))
            } else {
                default_endpoint
            };

            let auth = if has_custom_key {
                AuthData::from_single(api_key.as_str())
            } else {
                default_auth
            };
            Ok(ServiceTarget {
                endpoint,
                auth,
                model: ModelIden::new(adapter_kind, model.model_name),
            })
        },
    );

    builder
        .with_service_target_resolver(target_resolver)
        .build()
}

/// Shared direct reqwest client with genai's default WebConfig tuning;
/// `no_proxy()` disables the registry/env proxy fallback.
fn direct_client() -> &'static reqwest::Client {
    static DIRECT: LazyLock<reqwest::Client> = LazyLock::new(|| {
        genai::WebConfig::default()
            .apply_to_builder(reqwest::Client::builder().no_proxy())
            .build()
            .expect("failed to build direct reqwest client")
    });
    &DIRECT
}
