//! Regenerate `assets/models.json` from models.dev and models-cn.
//!
//! models.dev supplies the base model catalog plus USD offers. models-cn
//! overlays official CNY offers for mainland Chinese providers.
//!
//! Usage:
//!   cargo run --example update_models
//!   cargo run --example update_models -- --models-dev tmp/models_dev.json --models-cn tmp/models_cn.json
//!   cargo run --example update_models -- --output /tmp/models.json
//!
//! The output keeps one compact model per line: small enough for the binary,
//! while still producing readable diffs.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

const MODELS_DEV_URL: &str = "https://models.dev/api.json";
const MODELS_CN_URL: &str = "https://null-object-0000.github.io/models-cn/v1/api.json";
const DEFAULT_OUTPUT: &str = "assets/models.json";
/// Keep the offer picker manageable while retaining the canonical provider.
const MAX_USD_OFFERS: usize = 8;

// ── models.dev input ────────────────────────────────────────────────

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct DevProvider {
    models: BTreeMap<String, DevModel>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct DevModel {
    id: String,
    name: String,
    reasoning: bool,
    reasoning_options: Vec<DevReasoningOption>,
    tool_call: bool,
    modalities: DevModalities,
    limit: DevLimit,
    cost: Option<DevPrice>,
    canonical_model_id: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct DevModalities {
    input: Vec<String>,
    output: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct DevLimit {
    context: Option<u32>,
    output: Option<u32>,
}

#[derive(Debug, Default, Clone, Copy, Deserialize)]
#[serde(default)]
struct DevPrice {
    input: f64,
    output: f64,
    cache_read: f64,
    cache_write: f64,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct DevReasoningOption {
    #[serde(rename = "type")]
    kind: String,
    values: Vec<Option<String>>,
}

// ── models-cn input ─────────────────────────────────────────────────

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct CnDatabase {
    providers: Vec<CnProvider>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct CnProvider {
    id: String,
    models: Vec<CnModel>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct CnModel {
    id: String,
    name: String,
    aliases: Vec<String>,
    capabilities: CnCapabilities,
    limits: CnLimits,
    prices: Vec<CnPrice>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct CnCapabilities {
    thinking: bool,
    tool_calls: bool,
    input_modalities: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct CnLimits {
    context_tokens: Option<u32>,
    max_output_tokens: Option<u32>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct CnPrice {
    currency: String,
    unit: String,
    rate_type: String,
    daily_time_range: Option<CnTimeRange>,
    input: Option<CnInputPrice>,
    output: Option<f64>,
    input_token_range: Option<CnTokenRange>,
    source_url: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct CnInputPrice {
    standard: Option<f64>,
    cache_hit: Option<f64>,
    explicit_cache_creation: Option<f64>,
    explicit_cache_hit: Option<f64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct CnTimeRange {
    label: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct CnTokenRange {
    max_inclusive: Option<u32>,
}

// ── output schema (parsed by src/model_database.rs) ─────────────────

#[derive(Debug, Default)]
struct Catalog {
    models: Vec<OutputModel>,
}

#[derive(Debug, Default, Serialize)]
struct OutputModel {
    id: String,
    model: String,
    offers: Vec<OutputOffer>,
    #[serde(skip_serializing_if = "Option::is_none")]
    context_length: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<u32>,
    reasoning: Option<bool>,
    tool_calling: Option<bool>,
    input_modalities: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    thinking_levels: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    alias_id: Vec<String>,
    /// Canonical-lab prefix, used to order offers and never serialized.
    #[serde(skip)]
    vendor: String,
}

#[derive(Debug, Serialize)]
struct OutputOffer {
    source: String,
    currency: String,
    prices: Vec<OutputPrice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    url: Option<String>,
}

#[derive(Debug, Serialize)]
struct OutputPrice {
    #[serde(skip_serializing_if = "Option::is_none")]
    input: Option<OutputAmount>,
    #[serde(skip_serializing_if = "Option::is_none")]
    output: Option<OutputAmount>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_read: Option<OutputAmount>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_write: Option<OutputAmount>,
    #[serde(skip_serializing_if = "is_false")]
    double_on_peak_hour: bool,
}

#[derive(Debug, Serialize)]
struct OutputAmount {
    amount: f64,
}

/// One models.dev provider-model entry before canonical grouping.
struct DevEntry {
    provider_id: String,
    model_id: String,
    model: DevModel,
}

// ── Entry point ─────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), String> {
    let args = Args::parse()?;

    let dev_json = load_source(&args.models_dev).await?;
    let cn_json = load_source(&args.models_cn).await?;

    let dev: BTreeMap<String, DevProvider> = serde_json::from_str(&dev_json)
        .map_err(|e| format!("Failed to parse models.dev JSON: {e}"))?;
    let cn: CnDatabase = serde_json::from_str(&cn_json)
        .map_err(|e| format!("Failed to parse models-cn JSON: {e}"))?;

    let mut catalog = build_catalog(dev);
    merge_duplicate_models(&mut catalog);
    merge_cny(&mut catalog, cn);
    for model in &mut catalog.models {
        sort_offers(model);
    }
    catalog
        .models
        .sort_by_key(|model| model.id.to_ascii_lowercase());

    let source = format!(
        "models.dev: {}; models-cn: {}",
        args.models_dev, args.models_cn
    );
    let output = render(&catalog, &source)?;
    std::fs::write(&args.output, output)
        .map_err(|e| format!("Failed to write {}: {e}", args.output.display()))?;

    let usd_offers: usize = catalog
        .models
        .iter()
        .map(|m| m.offers.iter().filter(|o| o.currency == "USD").count())
        .sum();
    let cny_offers: usize = catalog
        .models
        .iter()
        .map(|m| m.offers.iter().filter(|o| o.currency == "CNY").count())
        .sum();
    let cny_models = catalog
        .models
        .iter()
        .filter(|m| m.offers.iter().any(|o| o.currency == "CNY"))
        .count();

    println!(
        "Wrote {} models ({} USD offers, {} CNY offers across {} models) to {}",
        catalog.models.len(),
        usd_offers,
        cny_offers,
        cny_models,
        args.output.display()
    );
    Ok(())
}

// ── Arguments ───────────────────────────────────────────────────────

struct Args {
    models_dev: String,
    models_cn: String,
    output: PathBuf,
}

impl Args {
    fn parse() -> Result<Self, String> {
        let mut args = std::env::args().skip(1);
        let mut models_dev = MODELS_DEV_URL.to_string();
        let mut models_cn = MODELS_CN_URL.to_string();
        let mut output = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(DEFAULT_OUTPUT);

        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--models-dev" => models_dev = next_value(&mut args, "--models-dev")?,
                "--models-cn" => models_cn = next_value(&mut args, "--models-cn")?,
                "--output" => output = PathBuf::from(next_value(&mut args, "--output")?),
                "-h" | "--help" => {
                    print_usage();
                    std::process::exit(0);
                }
                other => return Err(format!("unknown argument: {other}")),
            }
        }

        Ok(Self {
            models_dev,
            models_cn,
            output,
        })
    }
}

fn next_value(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<String, String> {
    args.next()
        .ok_or_else(|| format!("missing value for {flag}"))
}

fn print_usage() {
    println!(
        "Usage: update_models [--models-dev URL|PATH] [--models-cn URL|PATH] [--output PATH]\n\
         Defaults: models.dev api.json, models-cn v1 api.json, assets/models.json"
    );
}

// ── Loading ─────────────────────────────────────────────────────────

async fn load_source(source: &str) -> Result<String, String> {
    if !source.starts_with("http://") && !source.starts_with("https://") {
        return std::fs::read_to_string(source)
            .map_err(|e| format!("Failed to read {source}: {e}"));
    }

    crabot::setup::install_crypto_provider();
    let client = reqwest::Client::builder()
        .user_agent(crabot::app_title())
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(|e| format!("Failed to build HTTP client: {e}"))?;
    let response = client
        .get(source)
        .send()
        .await
        .map_err(|e| format!("HTTP request failed for {source}: {e}"))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|e| format!("Failed to read response from {source}: {e}"))?;
    if !status.is_success() {
        let detail: String = body.chars().take(200).collect();
        return Err(format!("HTTP {status} for {source}: {detail}"));
    }
    Ok(body)
}

// ── models.dev conversion ───────────────────────────────────────────

fn build_catalog(dev: BTreeMap<String, DevProvider>) -> Catalog {
    let mut groups: BTreeMap<String, Vec<DevEntry>> = BTreeMap::new();
    for (provider_id, provider) in dev {
        for (model_id, model) in provider.models {
            let key = model
                .canonical_model_id
                .clone()
                .unwrap_or_else(|| model.id.clone());
            groups.entry(key).or_default().push(DevEntry {
                provider_id: provider_id.clone(),
                model_id,
                model,
            });
        }
    }

    let mut catalog = Catalog::default();
    for (key, mut entries) in groups {
        if entries.is_empty() || !entries.iter().any(|e| accepts_text_output(&e.model)) {
            continue;
        }

        let (vendor, short) = key
            .split_once('/')
            .map_or(("", key.as_str()), |(vendor, short)| (vendor, short));
        entries.sort_by_key(|entry| rank(entry, vendor, short));
        let primary = &entries[0];

        let context_length = primary
            .model
            .limit
            .context
            .or_else(|| entries.iter().filter_map(|e| e.model.limit.context).max());
        let max_output_tokens = primary
            .model
            .limit
            .output
            .or_else(|| entries.iter().filter_map(|e| e.model.limit.output).max());
        let reasoning = entries.iter().any(|e| e.model.reasoning);
        let thinking_levels = thinking_levels(&entries, reasoning);
        let input_modalities = input_modalities(primary, &entries);
        let aliases = aliases(&entries, &key, &primary.model_id);
        let offers = usd_offers(&entries, vendor);

        catalog.models.push(OutputModel {
            id: primary.model_id.clone(),
            model: if primary.model.name.is_empty() {
                short.to_string()
            } else {
                primary.model.name.clone()
            },
            offers,
            context_length,
            max_output_tokens,
            reasoning: Some(reasoning),
            tool_calling: Some(primary.model.tool_call),
            input_modalities,
            thinking_levels,
            alias_id: aliases,
            vendor: vendor.to_string(),
        });
    }
    catalog
}

/// Prefer the canonical vendor's own entry, then `openrouter`, then the rest.
fn rank(
    entry: &DevEntry,
    vendor: &str,
    short: &str,
) -> (bool, bool, bool, std::cmp::Reverse<usize>, String) {
    (
        entry.provider_id != vendor,
        entry.model_id != short,
        entry.model.cost.is_none_or(|cost| !cost.has_price()),
        std::cmp::Reverse(entry.model.modalities.input.len()),
        entry.provider_id.clone(),
    )
}

fn accepts_text_output(model: &DevModel) -> bool {
    model.modalities.output.is_empty()
        || model
            .modalities
            .output
            .iter()
            .any(|m| m.eq_ignore_ascii_case("text"))
}

fn thinking_levels(entries: &[DevEntry], reasoning: bool) -> Vec<String> {
    if !reasoning {
        return Vec::new();
    }
    let mut levels = Vec::new();
    for entry in entries {
        for option in &entry.model.reasoning_options {
            if option.kind.eq_ignore_ascii_case("effort") {
                for value in option.values.iter().flatten() {
                    if !value.is_empty() && !levels.contains(value) {
                        levels.push(value.clone());
                    }
                }
            }
        }
        if !levels.is_empty() {
            break;
        }
    }
    if levels.is_empty() {
        ["low", "medium", "high", "max"]
            .into_iter()
            .map(String::from)
            .collect()
    } else {
        levels
    }
}

fn input_modalities(primary: &DevEntry, entries: &[DevEntry]) -> Vec<String> {
    if !primary.model.modalities.input.is_empty() {
        return sorted_unique(&primary.model.modalities.input);
    }
    let inputs: Vec<String> = entries
        .iter()
        .flat_map(|entry| entry.model.modalities.input.iter().cloned())
        .collect();
    let inputs = sorted_unique(&inputs);
    if inputs.is_empty() {
        vec!["text".to_string()]
    } else {
        inputs
    }
}

fn sorted_unique(values: &[String]) -> Vec<String> {
    values
        .iter()
        .filter(|value| !value.is_empty())
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn aliases(entries: &[DevEntry], canonical: &str, primary_id: &str) -> Vec<String> {
    let mut aliases: BTreeSet<String> = BTreeSet::new();
    for entry in entries {
        if entry.model_id != primary_id {
            aliases.insert(entry.model_id.clone());
        }
        if !entry.model.id.is_empty() && entry.model.id != primary_id {
            aliases.insert(entry.model.id.clone());
        }
    }
    if canonical != primary_id {
        aliases.insert(canonical.to_string());
    }
    aliases.into_iter().collect()
}

fn usd_offers(entries: &[DevEntry], vendor: &str) -> Vec<OutputOffer> {
    let mut offers: Vec<(String, DevPrice)> = entries
        .iter()
        .filter_map(|entry| {
            let price = entry.model.cost?;
            price
                .has_price()
                .then(|| (entry.provider_id.clone(), price))
        })
        .collect();
    offers.sort_by(|a, b| {
        (a.0 != vendor, a.0.as_str() != "openrouter", a.0.as_str()).cmp(&(
            b.0 != vendor,
            b.0.as_str() != "openrouter",
            b.0.as_str(),
        ))
    });

    let mut seen_sources = BTreeSet::new();
    let mut seen_prices = BTreeSet::new();
    let mut result = Vec::new();
    for (source, price) in offers {
        if !seen_sources.insert(source.clone()) || !seen_prices.insert(price.signature()) {
            continue;
        }
        result.push(OutputOffer::usd(source, price));
        if result.len() == MAX_USD_OFFERS {
            break;
        }
    }
    result
}

// ── models-cn overlay ───────────────────────────────────────────────

fn merge_duplicate_models(catalog: &mut Catalog) {
    let mut by_id: BTreeMap<String, OutputModel> = BTreeMap::new();
    for model in std::mem::take(&mut catalog.models) {
        match by_id.entry(model.id.clone()) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(model);
            }
            std::collections::btree_map::Entry::Occupied(entry) => {
                let merged = entry.remove().merge(model);
                by_id.insert(merged.id.clone(), merged);
            }
        }
    }
    catalog.models = by_id.into_values().collect();
}

impl OutputModel {
    /// Merge one duplicate primary ID into another record, keeping the record
    /// with the most offers/aliases as the metadata source.
    fn merge(mut self, other: OutputModel) -> Self {
        let (base, extra) = if model_score(&other) > model_score(&self) {
            (other, self)
        } else {
            (self, other)
        };
        self = base;

        self.alias_id = self
            .alias_id
            .into_iter()
            .chain(extra.alias_id)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        self.input_modalities = sorted_unique(
            &self
                .input_modalities
                .into_iter()
                .chain(extra.input_modalities)
                .collect::<Vec<_>>(),
        );
        if self.thinking_levels.is_empty() {
            self.thinking_levels = extra.thinking_levels;
        }
        self.reasoning = Some(self.reasoning.unwrap_or(false) || extra.reasoning.unwrap_or(false));
        self.tool_calling =
            Some(self.tool_calling.unwrap_or(false) || extra.tool_calling.unwrap_or(false));
        self.context_length = self.context_length.or(extra.context_length);
        self.max_output_tokens = self.max_output_tokens.or(extra.max_output_tokens);

        let mut seen: BTreeSet<OfferKey> = self.offers.iter().map(OutputOffer::key).collect();
        for offer in extra.offers {
            if seen.insert(offer.key()) {
                self.offers.push(offer);
            }
        }
        self
    }
}

fn model_score(model: &OutputModel) -> (usize, usize) {
    (model.offers.len(), model.alias_id.len())
}

/// Canonical vendor first, then OpenRouter, then other USD offers, then CNY.
fn sort_offers(model: &mut OutputModel) {
    let vendor = model.vendor.clone();
    model
        .offers
        .sort_by_key(|offer| offer_order(offer, &vendor));
    let mut seen = BTreeSet::new();
    model.offers.retain(|offer| seen.insert(offer.signature()));
    let mut usd = 0;
    model.offers.retain(|offer| {
        if offer.currency != "USD" {
            return true;
        }
        usd += 1;
        usd <= MAX_USD_OFFERS
    });
}

fn offer_order(offer: &OutputOffer, vendor: &str) -> (u8, String) {
    let rank = if offer.currency == "USD" && offer.source == vendor {
        0
    } else if offer.currency == "USD" && offer.source == "openrouter" {
        1
    } else if offer.currency == "USD" {
        2
    } else {
        3
    };
    (rank, offer.source.clone())
}

type OfferKey = (String, String);

impl OutputOffer {
    fn key(&self) -> OfferKey {
        (self.source.clone(), self.currency.clone())
    }

    /// Same-price offers collapse to the first (best-ranked) one.
    fn signature(&self) -> (String, Vec<PriceSignature>) {
        (
            self.currency.clone(),
            self.prices.iter().map(OutputPrice::signature).collect(),
        )
    }
}

fn merge_cny(catalog: &mut Catalog, cn: CnDatabase) {
    let mut lookup = Lookup::from_models(&catalog.models);
    let mut new_models: Vec<OutputModel> = Vec::new();

    for provider in cn.providers {
        for model in &provider.models {
            let Some(offer) = cny_offer(&provider, model) else {
                continue;
            };
            if let Some(index) = lookup.find(&model.id, &model.aliases) {
                let target = &mut catalog.models[index];
                if !target.offers.iter().any(|o| o.source == offer.source) {
                    target.offers.push(offer);
                }
                lookup.insert_aliases(index, &model.id, &model.aliases);
            } else {
                let index = catalog.models.len() + new_models.len();
                let record = model_from_cn(&provider, model, offer);
                lookup.insert_primary(index, &record.id);
                lookup.insert_aliases(index, &record.id, &record.alias_id);
                new_models.push(record);
            }
        }
    }
    catalog.models.extend(new_models);
}

/// Primary IDs take precedence over aliases, so a model that is both a real
/// model ID and another model's alias (for example a highspeed variant) still
/// receives its own prices.
#[derive(Default)]
struct Lookup {
    primary: HashMap<String, usize>,
    alias: HashMap<String, usize>,
    primary_normalized: HashMap<String, usize>,
    alias_normalized: HashMap<String, usize>,
}

impl Lookup {
    fn from_models(models: &[OutputModel]) -> Self {
        let mut lookup = Self::default();
        for (index, model) in models.iter().enumerate() {
            lookup.insert_primary(index, &model.id);
        }
        for (index, model) in models.iter().enumerate() {
            lookup.insert_aliases(index, &model.id, &model.alias_id);
        }
        lookup
    }

    fn insert_primary(&mut self, index: usize, id: &str) {
        self.primary.entry(id.to_string()).or_insert(index);
        self.primary_normalized
            .entry(normalize_id(id))
            .or_insert(index);
    }

    fn insert_aliases(&mut self, index: usize, id: &str, aliases: &[String]) {
        self.alias.entry(id.to_string()).or_insert(index);
        self.alias_normalized
            .entry(normalize_id(id))
            .or_insert(index);
        for alias in aliases {
            self.alias.entry(alias.clone()).or_insert(index);
            self.alias_normalized
                .entry(normalize_id(alias))
                .or_insert(index);
        }
    }

    fn find(&self, id: &str, aliases: &[String]) -> Option<usize> {
        for candidate in std::iter::once(id).chain(aliases.iter().map(String::as_str)) {
            if let Some(index) = self.primary.get(candidate) {
                return Some(*index);
            }
            if let Some(index) = self.alias.get(candidate) {
                return Some(*index);
            }
        }
        for candidate in std::iter::once(id).chain(aliases.iter().map(String::as_str)) {
            let normalized = normalize_id(candidate);
            if let Some(index) = self.primary_normalized.get(&normalized) {
                return Some(*index);
            }
            if let Some(index) = self.alias_normalized.get(&normalized) {
                return Some(*index);
            }
        }
        None
    }
}

/// Normalize dots, underscores, slashes and case so `LongCat-2.5-Preview`
/// matches `longcat-2-5-preview`.
fn normalize_id(id: &str) -> String {
    let short = id.rsplit('/').next().unwrap_or(id);
    let mut out = String::with_capacity(short.len());
    for ch in short.chars() {
        let ch = ch.to_ascii_lowercase();
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

fn model_from_cn(provider: &CnProvider, model: &CnModel, offer: OutputOffer) -> OutputModel {
    let thinking = model.capabilities.thinking;
    let mut inputs = model.capabilities.input_modalities.clone();
    if inputs.is_empty() {
        inputs.push("text".to_string());
    }
    OutputModel {
        id: model.id.clone(),
        model: model.name.clone(),
        offers: vec![offer],
        context_length: model.limits.context_tokens,
        max_output_tokens: model.limits.max_output_tokens,
        reasoning: Some(thinking),
        tool_calling: Some(model.capabilities.tool_calls),
        input_modalities: inputs,
        thinking_levels: if thinking {
            ["low", "medium", "high", "max"]
                .into_iter()
                .map(String::from)
                .collect()
        } else {
            Vec::new()
        },
        alias_id: model.aliases.clone(),
        vendor: provider.id.clone(),
    }
}

fn cny_offer(provider: &CnProvider, model: &CnModel) -> Option<OutputOffer> {
    let mut candidates: Vec<&CnPrice> = model
        .prices
        .iter()
        .filter(|price| {
            price.currency == "CNY"
                && price.unit == "1M_tokens"
                && price
                    .input
                    .as_ref()
                    .and_then(|input| input.standard)
                    .is_some()
                && price.output.is_some()
        })
        .collect();
    if candidates.is_empty() {
        return None;
    }

    let standard: Vec<&CnPrice> = candidates
        .iter()
        .copied()
        .filter(|price| price.rate_type == "standard")
        .collect();
    if !standard.is_empty() {
        candidates = standard;
    }

    let (base, double_on_peak_hour) = pick_cny_price(&candidates)?;
    let input = base.input.as_ref()?;
    let cache_read = input.cache_hit.or(input.explicit_cache_hit);
    let cache_write = input.explicit_cache_creation;

    Some(OutputOffer {
        source: format!("models-cn:{}", provider.id),
        currency: "CNY".to_string(),
        prices: vec![OutputPrice::new(
            input.standard?,
            base.output?,
            cache_read.unwrap_or(0.0),
            cache_write.unwrap_or(0.0),
            double_on_peak_hour,
        )],
        url: base.source_url.clone(),
    })
}

fn pick_cny_price<'a>(candidates: &[&'a CnPrice]) -> Option<(&'a CnPrice, bool)> {
    let peak = candidates.iter().copied().find(|price| is_peak(price));
    let off_peak = candidates.iter().copied().find(|price| is_off_peak(price));
    if let (Some(peak), Some(off_peak)) = (peak, off_peak)
        && peak_and_off_peak_are_double(peak, off_peak)
    {
        return Some((off_peak, true));
    }

    if let Some(base) = candidates
        .iter()
        .copied()
        .find(|price| price.daily_time_range.is_none())
    {
        return Some((base, false));
    }

    candidates
        .iter()
        .copied()
        .min_by_key(|price| {
            price
                .input_token_range
                .as_ref()
                .and_then(|range| range.max_inclusive)
                .unwrap_or(0)
        })
        .map(|base| (base, false))
}

fn price_label(price: &CnPrice) -> String {
    price
        .daily_time_range
        .as_ref()
        .map(|range| range.label.to_ascii_lowercase())
        .unwrap_or_default()
}

fn is_peak(price: &CnPrice) -> bool {
    let label = price_label(price);
    (label.contains("高峰") || label.contains("peak"))
        && !(label.contains("空闲") || label.contains('谷') || label.contains("off"))
}

fn is_off_peak(price: &CnPrice) -> bool {
    let label = price_label(price);
    label.contains("空闲") || label.contains('谷') || label.contains("off")
}

fn peak_and_off_peak_are_double(peak: &CnPrice, off_peak: &CnPrice) -> bool {
    let Some((peak_input, peak_output)) = price_amounts(peak) else {
        return false;
    };
    let Some((off_input, off_output)) = price_amounts(off_peak) else {
        return false;
    };
    off_input > 0.0
        && off_output > 0.0
        && (peak_input / off_input - 2.0).abs() < 0.02
        && (peak_output / off_output - 2.0).abs() < 0.02
}

fn price_amounts(price: &CnPrice) -> Option<(f64, f64)> {
    Some((price.input.as_ref()?.standard?, price.output?))
}

// ── Serialization ───────────────────────────────────────────────────

type PriceSignature = (i64, i64, i64, i64, bool);

fn quantize(value: f64) -> i64 {
    (value * 1e9).round() as i64
}

impl DevPrice {
    fn has_price(self) -> bool {
        self.input > 0.0 || self.output > 0.0 || self.cache_read > 0.0 || self.cache_write > 0.0
    }

    /// Quantized tuple used to collapse resellers that quote the same price.
    fn signature(self) -> (i64, i64, i64, i64) {
        (
            quantize(self.input),
            quantize(self.output),
            quantize(self.cache_read),
            quantize(self.cache_write),
        )
    }
}

impl OutputOffer {
    fn usd(source: String, price: DevPrice) -> Self {
        Self {
            source,
            currency: "USD".to_string(),
            prices: vec![OutputPrice::new(
                price.input,
                price.output,
                price.cache_read,
                price.cache_write,
                false,
            )],
            url: None,
        }
    }
}

impl OutputPrice {
    /// Quantized price tuple for dedup.
    fn signature(&self) -> PriceSignature {
        let quantized =
            |amount: &Option<OutputAmount>| amount.as_ref().map_or(0, |a| quantize(a.amount));
        (
            quantized(&self.input),
            quantized(&self.output),
            quantized(&self.cache_read),
            quantized(&self.cache_write),
            self.double_on_peak_hour,
        )
    }

    fn new(
        input: f64,
        output: f64,
        cache_read: f64,
        cache_write: f64,
        double_on_peak_hour: bool,
    ) -> Self {
        Self {
            input: amount(input),
            output: amount(output),
            cache_read: amount(cache_read),
            cache_write: amount(cache_write),
            double_on_peak_hour,
        }
    }
}

fn amount(value: f64) -> Option<OutputAmount> {
    (value > 0.0).then_some(OutputAmount { amount: value })
}

fn render(catalog: &Catalog, source: &str) -> Result<String, String> {
    let generated_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let mut out = String::new();
    out.push_str("{\n");
    out.push_str("  \"schema_version\": 2,\n");
    out.push_str(&format!(
        "  \"generated_at\": {},\n",
        json_string(&generated_at)?
    ));
    out.push_str(&format!("  \"source\": {},\n", json_string(source)?));
    out.push_str(&format!("  \"count\": {},\n", catalog.models.len()));
    out.push_str("  \"models\": [\n");
    for (index, model) in catalog.models.iter().enumerate() {
        let line = serde_json::to_string(model)
            .map_err(|e| format!("Failed to serialize model {}: {e}", model.id))?;
        out.push_str("    ");
        out.push_str(&line);
        if index + 1 < catalog.models.len() {
            out.push(',');
        }
        out.push('\n');
    }
    out.push_str("  ]\n}\n");
    Ok(out)
}

fn json_string(value: &str) -> Result<String, String> {
    serde_json::to_string(value).map_err(|e| format!("Failed to serialize JSON string: {e}"))
}

fn is_false(value: &bool) -> bool {
    !*value
}
