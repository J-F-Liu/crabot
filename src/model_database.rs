use serde::Deserialize;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use crate::model::{Cost, Model};

// ── Raw deserialisation types ───────────────────────────────────────

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawDatabase {
    models: Vec<RawModel>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawModel {
    id: String,
    model: String,
    offers: Vec<RawOffer>,
    context_length: Option<u32>,
    max_output_tokens: Option<u32>,
    reasoning: Option<bool>,
    tool_calling: Option<bool>,
    input_modalities: Vec<String>,
    author: String,
    author_id: String,
    alias_id: Vec<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawOffer {
    source: Option<String>,
    currency: String,
    prices: Vec<RawPrice>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawPrice {
    input: Option<RawAmount>,
    output: Option<RawAmount>,
    cache_read: Option<RawAmount>,
    cache_write: Option<RawAmount>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawAmount {
    amount: f64,
}

// ── Static cache ────────────────────────────────────────────────────

/// Parsed model database, initialised once on first access.
static MODEL_DB: OnceLock<Arc<HashMap<String, Model>>> = OnceLock::new();

// ── ModelDatabase ───────────────────────────────────────────────────

/// A read‑only database of models (with full details) indexed by model ID.
///
/// Built from the embedded `assets/models.json` so that even models not yet
/// added to a provider can be inspected (pricing, context window, etc.).
///
/// Cheap to clone — the inner map is `Arc`-shared and parsed only once.
#[derive(Debug, Clone, Default)]
pub struct ModelDatabase {
    models: Arc<HashMap<String, Model>>,
}

impl ModelDatabase {
    /// Load the database from the embedded `assets/models.json`.
    pub fn load_embedded() -> Self {
        let models = MODEL_DB.get_or_init(|| {
            let raw = crate::setup::ASSETS
                .get_file("models.json")
                .and_then(|f| f.contents_utf8())
                .unwrap_or("");
            if raw.is_empty() {
                return Arc::new(HashMap::new());
            }
            match serde_json::from_str::<RawDatabase>(raw) {
                Ok(json) => Arc::new(parse_models(json)),
                Err(e) => {
                    eprintln!("Failed to parse embedded models.json: {e}");
                    Arc::new(HashMap::new())
                }
            }
        });
        Self {
            models: Arc::clone(models),
        }
    }

    /// Look up a model by its primary ID or alias.
    pub fn get(&self, model_id: &str) -> Option<&Model> {
        self.models.get(model_id).or_else(|| {
            ["-free", ":free"]
                .iter()
                .find_map(|s| model_id.strip_suffix(s))
                .and_then(|stripped| self.models.get(stripped))
        })
    }
}

// ── Parsing ─────────────────────────────────────────────────────────

fn parse_models(raw: RawDatabase) -> HashMap<String, Model> {
    let mut models: HashMap<String, Model> = HashMap::new();
    // Keep aliases deferred so they never shadow another model's primary ID.
    let mut pending_aliases: Vec<(String, Model)> = Vec::new();
    for m in raw.models {
        let (cost, offers) = extract_costs(&m.offers);
        let thinking = m.reasoning.unwrap_or(false);
        let thinking_levels = if thinking {
            vec![
                "low".to_string(),
                "medium".to_string(),
                "high".to_string(),
                "max".to_string(),
            ]
        } else {
            Vec::new()
        };
        let model = Model {
            id: m.id.clone(),
            name: m.model,
            thinking,
            thinking_levels,
            input: m.input_modalities,
            context_window: m.context_length.unwrap_or(0),
            max_tokens: m.max_output_tokens.unwrap_or(0),
            cost,
            offers,
        };
        // Primary ID first — first-wins among duplicates.
        models.entry(m.id).or_insert_with(|| model.clone());
        // Defer aliases until after all primaries are registered.
        for alias in m.alias_id {
            pending_aliases.push((alias, model.clone()));
        }
    }
    // Now insert aliases — they never override primary IDs.
    for (alias, model) in pending_aliases {
        models.entry(alias).or_insert(model);
    }
    models
}

/// Convert raw offers into a primary `Cost` (first offer) and the full list.
fn extract_costs(offers: &[RawOffer]) -> (Cost, Vec<Cost>) {
    let all: Vec<Cost> = offers
        .iter()
        .filter_map(|offer| {
            offer.prices.first().map(|price| Cost {
                input: price.input.as_ref().map(|a| a.amount).unwrap_or(0.0),
                output: price.output.as_ref().map(|a| a.amount).unwrap_or(0.0),
                cache_read: price.cache_read.as_ref().map(|a| a.amount).unwrap_or(0.0),
                cache_write: price.cache_write.as_ref().map(|a| a.amount).unwrap_or(0.0),
                currency: offer.currency.clone(),
                source: offer.source.clone().unwrap_or_default(),
            })
        })
        .collect();
    let primary = all.first().cloned().unwrap_or_default();
    (primary, all)
}
