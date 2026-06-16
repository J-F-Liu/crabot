use serde::Serialize;
use std::collections::BTreeMap;
use std::path::PathBuf;

// ── Provider ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize)]
pub struct Provider {
    pub id: String,
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

impl PartialEq for Provider {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

// ── Model ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize)]
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

#[derive(Debug, Clone, Default, Serialize)]
pub struct Cost {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
}

// ── loaders ─────────────────────────────────────────────────────────

/// Loads providers and their models from `~/.omp/agent/models.yml`.
pub fn try_load_models_from_omp() -> Result<Vec<Provider>, Box<dyn std::error::Error>> {
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
            id: key.into(),
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

    let providers = tree
        .get("providers")
        .and_then(|v| v.as_mapping())
        .map(|m| {
            m.iter()
                .map(|(k, v)| parse_provider(k.as_str().unwrap_or(""), v))
                .collect()
        })
        .unwrap_or_default();
    Ok(providers)
}

/// Loads providers and their models from `~/.pi/agent/models.json`.
pub fn try_load_models_from_pi() -> Result<Vec<Provider>, Box<dyn std::error::Error>> {
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
            id: key.into(),
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

    let providers = tree
        .get("providers")
        .and_then(|v| v.as_object())
        .map(|m| m.iter().map(|(k, v)| parse_provider(k, v)).collect())
        .unwrap_or_default();
    Ok(providers)
}
