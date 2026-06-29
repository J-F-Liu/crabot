use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelList {
    pub providers: IndexMap<String, Provider>,
    pub models: IndexMap<String, ModelConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelConfig {
    pub provider_id: String,
    pub model_id: String,
    pub thinking: bool,
    pub thinking_level: String,
}

// ── Provider ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Provider {
    pub name: String,
    pub base_url: String,
    pub api_key: String,
    pub api_type: String,
    pub auth: String,
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
    pub context_window: u64,
    pub max_tokens: u64,
    pub cost: Cost,
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
}

impl Cost {
    /// Calculate cost breakdown from token usage.
    /// Prices are per million tokens; token counts are raw integers.
    pub fn calculate(&self, tokens: &TokenAmount) -> f64 {
        let input_cost = (tokens.input - tokens.cached).max(0) as f64 / 1_000_000.0 * self.input;
        let cached_cost = tokens.cached as f64 / 1_000_000.0 * self.cache_read;
        let output_cost = tokens.output as f64 / 1_000_000.0 * self.output;
        input_cost + cached_cost + output_cost
    }
}

/// Accumulated token counts for a session or single response.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct TokenAmount {
    pub input: i32,
    pub cached: i32,
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
        let prompt = usage.prompt_tokens.unwrap_or(0);
        Self {
            input: prompt, // total input (cached + uncached)
            cached,
            output: usage.completion_tokens.unwrap_or(0),
        }
    }
    /// Accumulate `incoming` into `self` in place.
    pub fn accumulate(&mut self, incoming: &TokenAmount) {
        self.input += incoming.input;
        self.cached += incoming.cached;
        self.output += incoming.output;
    }
}

// ── loaders ─────────────────────────────────────────────────────────

/// Loads providers and their models from `~/.omp/agent/models.yml`.
fn try_load_models_from_omp() -> Result<ModelList, Box<dyn std::error::Error>> {
    use serde_yaml::Value;

    fn omp_models_path() -> Result<PathBuf, Box<dyn std::error::Error>> {
        let home = home::home_dir().ok_or("cannot determine home directory")?;
        Ok(home.join(".omp").join("agent").join("models.yml"))
    }

    fn parse_cost(v: &Value) -> Cost {
        Cost {
            input: v.get("input").and_then(|v| v.as_f64()).unwrap_or(0.0),
            output: v.get("output").and_then(|v| v.as_f64()).unwrap_or(0.0),
            cache_read: v.get("cacheRead").and_then(|v| v.as_f64()).unwrap_or(0.0),
            cache_write: v.get("cacheWrite").and_then(|v| v.as_f64()).unwrap_or(0.0),
        }
    }

    fn parse_model(v: &Value) -> Model {
        Model {
            id: v.get("id").and_then(|v| v.as_str()).unwrap_or("").into(),
            name: v.get("name").and_then(|v| v.as_str()).unwrap_or("").into(),
            thinking: v
                .get("reasoning")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            thinking_levels: v
                .get("reasoningLevels")
                .and_then(|v| v.as_sequence())
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default(),
            input: v
                .get("input")
                .and_then(|v| v.as_sequence())
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default(),
            context_window: v.get("contextWindow").and_then(|v| v.as_u64()).unwrap_or(0),
            max_tokens: v.get("maxTokens").and_then(|v| v.as_u64()).unwrap_or(0),
            cost: v.get("cost").map(parse_cost).unwrap_or_default(),
        }
    }

    fn parse_provider(key: &str, v: &Value) -> Provider {
        Provider {
            name: v.get("name").and_then(|v| v.as_str()).unwrap_or(key).into(),
            base_url: v
                .get("baseUrl")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .into(),
            api_key: v
                .get("apiKey")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .into(),
            api_type: v.get("api").and_then(|v| v.as_str()).unwrap_or("").into(),
            auth: v.get("auth").and_then(|v| v.as_str()).unwrap_or("").into(),
            headers: v
                .get("headers")
                .and_then(|v| v.as_mapping())
                .map(|m| {
                    m.iter()
                        .map(|(k, v)| {
                            (
                                k.as_str().unwrap_or("").into(),
                                v.as_str().unwrap_or("").into(),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default(),
            models: v
                .get("models")
                .and_then(|v| v.as_sequence())
                .map(|a| a.iter().map(parse_model).collect())
                .unwrap_or_default(),
        }
    }

    let raw = std::fs::read_to_string(omp_models_path()?)?;
    let tree: Value = serde_yaml::from_str(&raw)?;

    let providers: IndexMap<String, Provider> = tree
        .get("providers")
        .and_then(|v| v.as_mapping())
        .map(|m| {
            m.iter()
                .map(|(k, v)| {
                    let id = k.as_str().unwrap_or("").to_string();
                    (id.clone(), parse_provider(&id, v))
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(ModelList {
        providers,
        models: IndexMap::new(),
    })
}

/// Loads providers and their models from `~/.pi/agent/models.json`.
fn try_load_models_from_pi() -> Result<ModelList, Box<dyn std::error::Error>> {
    use serde_json::Value;

    fn pi_models_path() -> Result<PathBuf, Box<dyn std::error::Error>> {
        let home = home::home_dir().ok_or("cannot determine home directory")?;
        Ok(home.join(".pi").join("agent").join("models.json"))
    }

    fn parse_cost(v: &Value) -> Cost {
        Cost {
            input: v.get("input").and_then(|v| v.as_f64()).unwrap_or(0.0),
            output: v.get("output").and_then(|v| v.as_f64()).unwrap_or(0.0),
            cache_read: v.get("cacheRead").and_then(|v| v.as_f64()).unwrap_or(0.0),
            cache_write: v.get("cacheWrite").and_then(|v| v.as_f64()).unwrap_or(0.0),
        }
    }

    fn parse_model(v: &Value) -> Model {
        Model {
            id: v.get("id").and_then(|v| v.as_str()).unwrap_or("").into(),
            name: v.get("name").and_then(|v| v.as_str()).unwrap_or("").into(),
            thinking: v
                .get("reasoning")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            thinking_levels: v
                .get("thinkingLevelMap")
                .and_then(|v| v.as_object())
                .map(|m| {
                    m.values()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default(),
            input: v
                .get("input")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default(),
            context_window: v.get("contextWindow").and_then(|v| v.as_u64()).unwrap_or(0),
            max_tokens: v.get("maxTokens").and_then(|v| v.as_u64()).unwrap_or(0),
            cost: v.get("cost").map(parse_cost).unwrap_or_default(),
        }
    }

    fn parse_provider(key: &str, v: &Value) -> Provider {
        Provider {
            name: v.get("name").and_then(|v| v.as_str()).unwrap_or(key).into(),
            base_url: v
                .get("baseUrl")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .into(),
            api_key: v
                .get("apiKey")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .into(),
            api_type: v.get("api").and_then(|v| v.as_str()).unwrap_or("").into(),
            auth: v.get("auth").and_then(|v| v.as_str()).unwrap_or("").into(),
            headers: v
                .get("headers")
                .and_then(|v| v.as_object())
                .map(|m| {
                    m.iter()
                        .map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").into()))
                        .collect()
                })
                .unwrap_or_default(),
            models: v
                .get("models")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().map(parse_model).collect())
                .unwrap_or_default(),
        }
    }

    let raw = std::fs::read_to_string(pi_models_path()?)?;
    let tree: Value = serde_json::from_str(&raw)?;

    let providers: IndexMap<String, Provider> = tree
        .get("providers")
        .and_then(|v| v.as_object())
        .map(|m| {
            m.iter()
                .map(|(k, v)| (k.clone(), parse_provider(k, v)))
                .collect()
        })
        .unwrap_or_default();
    Ok(ModelList {
        providers,
        models: IndexMap::new(),
    })
}

/// Loads providers from `~/.crabot/models.ron`, falling back to
/// OMP (`~/.omp/agent/models.yml`) then PI (`~/.pi/agent/models.json`).
/// On a successful OMP or PI load the result is persisted to models.ron.
pub fn load_models() -> ModelList {
    let ron_exists = models_ron_path().map(|p| p.exists()).unwrap_or(false);
    if ron_exists {
        if let Ok(list) = try_load_models_from_ron() {
            return list;
        }
    } else {
        if let Ok(list) = try_load_models_from_omp() {
            save_models_to_ron(&list);
            return list;
        }
        if let Ok(list) = try_load_models_from_pi() {
            save_models_to_ron(&list);
            return list;
        }
    }
    ModelList::default()
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
