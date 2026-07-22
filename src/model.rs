use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelList {
    pub providers: IndexMap<String, Provider>,
    pub models: IndexMap<String, ModelConfig>,
}

impl ModelList {
    pub fn ensure_valid_name(&self, name: &str) -> String {
        let valid_name = if self.models.contains_key(name) {
            name
        } else if let Some(name) = self.models.keys().next() {
            name
        } else {
            if !name.is_empty() { name } else { "Model" }
        };
        valid_name.to_string()
    }

    pub fn get_config(&self, name: &str) -> Option<&ModelConfig> {
        self.models
            .get(name)
            .or_else(|| self.models.values().next())
    }

    pub fn get_config_mut(&mut self, name: &str) -> Option<&mut ModelConfig> {
        if !name.is_empty() && !self.models.contains_key(name) {
            self.models.insert(name.to_string(), ModelConfig::default());
        }
        self.models.get_mut(name)
    }

    pub fn get_provider(&self, name: &str) -> Option<&Provider> {
        let config = self.get_config(name)?;
        self.providers.get(&config.provider_id)
    }

    pub fn get_model(&self, config: &ModelConfig) -> Option<&Model> {
        let provider = self.providers.get(&config.provider_id)?;
        provider
            .models
            .iter()
            .find(|model| model.id == config.model_id)
    }

    pub fn get_model_info(&self, config: &ModelConfig) -> Option<ModelInfo> {
        let provider = self.providers.get(&config.provider_id)?;
        let model = provider.models.iter().find(|m| m.id == config.model_id)?;
        Some(ModelInfo {
            base_url: provider.base_url.clone(),
            api_type: provider.api_type.clone(),
            api_key: provider.api_key.clone(),
            strict: provider.strict_mode,
            model_id: model.id.clone(),
            max_tokens: model.max_tokens,
            thinking: config.thinking,
            thinking_level: config.thinking_level.clone(),
        })
    }

    /// Persist to `~/.crabot/models.ron`.
    pub fn save(&self) {
        save_models_to_ron(self);
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelConfig {
    pub provider_id: String,
    pub model_id: String,
    pub thinking: bool,
    pub thinking_level: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub base_url: String,
    pub api_type: String,
    pub api_key: String,
    pub strict: bool,
    pub model_id: String,
    /// Output token cap; 0 means unset (provider/genai default applies).
    pub max_tokens: u32,
    pub thinking: bool,
    pub thinking_level: String,
}

// ── Provider ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Provider {
    pub name: String,
    pub base_url: String,
    pub api_type: String,
    pub auth: String,
    pub api_key: String,
    #[serde(default)]
    pub strict_mode: bool,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    pub models: Vec<Model>,
}

impl std::fmt::Display for Provider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name)
    }
}

// ── Model ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Model {
    pub id: String,
    pub name: String,
    pub thinking: bool,
    pub thinking_levels: Vec<String>,
    pub input: Vec<String>,
    pub context_window: u32,
    pub max_tokens: u32,
    pub cost: Cost,
    /// All pricing offers (different currencies / providers).
    #[serde(default, skip)]
    pub offers: Vec<Cost>,
}

impl std::fmt::Display for Model {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name)
    }
}

impl PartialEq for Model {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

// ── Cost ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Cost {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
    #[serde(default)]
    pub currency: String,
    /// Offer source (e.g. "openrouter", "litellm", "bailian").
    #[serde(default, skip)]
    pub source: String,
}

impl Cost {
    /// Calculate cost breakdown from token usage.
    /// Prices are per million tokens; token counts are raw integers.
    pub fn calculate(&self, tokens: &TokenAmount) -> f64 {
        let regular_input = (tokens.input - tokens.cached - tokens.cache_write).max(0);
        let input_cost = regular_input as f64 / 1_000_000.0 * self.input;
        let cached_read_cost = tokens.cached as f64 / 1_000_000.0 * self.cache_read;
        let cache_write_cost = tokens.cache_write as f64 / 1_000_000.0 * self.cache_write;
        let output_cost = tokens.output as f64 / 1_000_000.0 * self.output;
        input_cost + cached_read_cost + cache_write_cost + output_cost
    }
}

/// Accumulated token counts for a session or single response.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct TokenAmount {
    pub input: i32,
    pub cached: i32,
    #[serde(default)]
    pub cache_write: i32,
    pub output: i32,
}

impl TokenAmount {
    /// Extract token counts from a `genai::chat::Usage`.
    pub fn from_genai(usage: &genai::chat::Usage) -> Self {
        let cached = usage
            .prompt_tokens_details
            .as_ref()
            .and_then(|d| d.cached_tokens)
            .unwrap_or(0);
        let cache_write = usage
            .prompt_tokens_details
            .as_ref()
            .and_then(|d| d.cache_creation_tokens)
            .unwrap_or(0);
        let prompt = usage.prompt_tokens.unwrap_or(0);
        Self {
            input: prompt, // total input (cached + uncached + cache-write)
            cached,
            cache_write,
            output: usage.completion_tokens.unwrap_or(0),
        }
    }
    /// Percentage of `context_window` used by the estimated next-turn
    /// context size: last prompt plus the response just generated.
    pub fn window_used(&self, context_window_size: u32) -> f32 {
        (self.input + self.output) as f32 * 100.0 / context_window_size as f32
    }
    /// Accumulate `incoming` into `self` in place.
    pub fn accumulate(&mut self, incoming: &TokenAmount) {
        self.input += incoming.input;
        self.cached += incoming.cached;
        self.cache_write += incoming.cache_write;
        self.output += incoming.output;
    }
}

/// Map common ISO 4217 currency codes to their symbols.
pub fn currency_symbol(currency: &str) -> &str {
    match currency {
        "USD" => "$",
        "CNY" => "¥",
        "EUR" => "€",
        "GBP" => "£",
        other => other,
    }
}

// ── loaders ─────────────────────────────────────────────────────────

/// Loads providers from `~/.crabot/models.ron`.
pub fn load_models() -> ModelList {
    let ron_exists = models_ron_path().map(|p| p.exists()).unwrap_or(false);
    if ron_exists {
        if let Ok(list) = try_load_models_from_ron() {
            return list;
        }
    } else {
        let _ = std::fs::write(models_ron_path().unwrap(), crate::setup::default_models());
        return try_load_models_from_ron().unwrap_or_default();
    }
    ModelList::default()
}

/// If `api_key` is an environment variable name, resolve it to the actual value.
/// Otherwise return the literal string as-is.
pub fn resolve_api_key(api_key: &str) -> String {
    std::env::var(api_key).unwrap_or_else(|_| api_key.to_string())
}

// ── Fetch models from provider ──────────────────────────────────────

/// Fetch available model IDs from a provider's `/models` endpoint.
pub async fn fetch_available_models(base_url: &str, api_key: &str) -> Result<Vec<String>, String> {
    let api_key = resolve_api_key(api_key);
    let url = if base_url.ends_with('/') {
        format!("{}models", base_url)
    } else {
        format!("{}/models", base_url)
    };
    let client = reqwest::Client::builder()
        .user_agent(crate::app_title())
        .build()
        .map_err(|e| format!("Failed to build HTTP client: {e}"))?;
    let resp = client
        .get(&url)
        .header("Content-Type", "application/json")
        .bearer_auth(&api_key)
        .send()
        .await
        .map_err(|e| format!("HTTP request failed: {e}"))?;
    let body = resp
        .text()
        .await
        .map_err(|e| format!("Failed to read response: {e}"))?;
    let json: serde_json::Value =
        serde_json::from_str(&body).map_err(|e| format!("Invalid JSON: {e}"))?;
    let models = json["data"]
        .as_array()
        .ok_or_else(|| "Invalid response format: missing 'data' array".to_string())?;
    let ids: Vec<String> = models
        .iter()
        .filter_map(|m| m["id"].as_str().map(String::from))
        .collect();
    Ok(ids)
}

// ── RON load / save ─────────────────────────────────────────────────

fn models_ron_path() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let home = home::home_dir().ok_or("cannot determine home directory")?;
    Ok(home.join(".crabot").join("models.ron"))
}

fn try_load_models_from_ron() -> Result<ModelList, Box<dyn std::error::Error>> {
    let path = models_ron_path()?;
    let text = std::fs::read_to_string(&path)?;
    let list: ModelList = ron::from_str(&text)?;
    Ok(list)
}

fn save_models_to_ron(list: &ModelList) {
    let path = match models_ron_path() {
        Ok(p) => p,
        Err(_) => return,
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(text) = ron::ser::to_string_pretty(list, ron::ser::PrettyConfig::default()) {
        let _ = std::fs::write(&path, text);
    }
}
