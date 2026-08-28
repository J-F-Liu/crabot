//! Aggregate session statistics: sessions and costs per day, grouped by model.
//!
//! Costs are attributed per model from each session's `Meta`/`Tally` records:
//! tallies are cumulative, so each delta (current − previous) is charged to the
//! most recent `Meta` model. Sessions without tallies (legacy `.json`, or no
//! completed requests) fall back to the session-level model and totals.
//!
//! By default only the current month is counted; `--month YYYY-MM` counts a
//! specific month instead.
//!
//! Usage:
//!   cargo run --release --example session_stats -- [workspace_path] [--month YYYY-MM]
//!
//! Without a workspace path, the current directory is used.

use std::collections::BTreeMap;
use std::env;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use crabot::model::{Currency, currency_symbol};
use crabot::session::{self, Session, SessionRecord};

/// Usage accrued while one model was active.
#[derive(Debug, Default)]
struct PerModelStats {
    requests: u64,
    cost: f64,
    currency: Currency,
}

/// Scan a session file's `Meta`/`Tally` records, charging each tally delta
/// (cumulative snapshot − previous snapshot) to the most recent `Meta` model.
fn per_model_stats(path: &Path) -> Result<BTreeMap<String, PerModelStats>, String> {
    let file =
        std::fs::File::open(path).map_err(|e| format!("Failed to open session file: {e}"))?;
    let reader = BufReader::new(file);

    let mut active_model: Option<String> = None;
    let mut prev_tally: Option<(u64, f64)> = None;
    let mut out: BTreeMap<String, PerModelStats> = BTreeMap::new();

    for (line_no, line) in reader.lines().enumerate() {
        let line =
            line.map_err(|e| format!("Failed to read session file at line {}: {e}", line_no + 1))?;
        let Ok(record) = serde_json::from_str::<SessionRecord>(line.trim()) else {
            continue;
        };
        match record {
            SessionRecord::Meta { model, .. } => active_model = model.map(|m| m.model_id),
            SessionRecord::Tally {
                requests,
                cost,
                currency,
                ..
            } => {
                let (prev_requests, prev_cost) = prev_tally.unwrap_or((0, 0.0));
                let entry = out
                    .entry(active_model.clone().unwrap_or_else(|| "unknown".into()))
                    .or_default();
                entry.requests += u64::from(requests) - prev_requests;
                entry.cost += cost - prev_cost;
                entry.currency = currency;
                prev_tally = Some((u64::from(requests), cost));
            }
            SessionRecord::Message { .. } => {}
        }
    }
    Ok(out)
}

/// Format a per-currency cost map into a display string (e.g. "$0.05 ¥3.50").
fn format_costs(costs: &BTreeMap<Currency, f64>) -> String {
    if costs.is_empty() {
        return String::new();
    }
    costs
        .iter()
        .map(|(cur, cost)| {
            let sym = currency_symbol(cur);
            format!("{sym}{:.2}", cost)
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Add all entries from `source` into `target` (mutating in place).
fn merge_costs(target: &mut BTreeMap<Currency, f64>, source: &BTreeMap<Currency, f64>) {
    for (cur, cost) in source {
        *target.entry(*cur).or_insert(0.0) += cost;
    }
}

/// Parse command line arguments: optional workspace path and optional `--month YYYY-MM`.
fn parse_args() -> (Option<PathBuf>, Option<String>) {
    let mut workspace = None;
    let mut month = None;
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--month" | "-m" => {
                let Some(value) = args.next() else {
                    eprintln!("Error: --month requires a value in YYYY-MM format.");
                    std::process::exit(1);
                };
                month = Some(validate_month(&value));
            }
            _ if arg.starts_with("--month=") => {
                month = Some(validate_month(&arg["--month=".len()..]));
            }
            _ if arg.starts_with('-') => {
                eprintln!("Unknown argument: {arg}");
                std::process::exit(1);
            }
            _ if workspace.is_none() => workspace = Some(PathBuf::from(arg)),
            _ => {
                eprintln!("Unexpected extra argument: {arg}");
                std::process::exit(1);
            }
        }
    }
    (workspace, month)
}

/// Normalize a `YYYY-MM` month (e.g. `2026-7` → `2026-07`); exit if malformed.
fn validate_month(value: &str) -> String {
    let Ok(date) = chrono::NaiveDate::parse_from_str(&format!("{value}-01"), "%Y-%m-%d") else {
        eprintln!("Error: invalid month '{value}', expected YYYY-MM (e.g. 2026-07).");
        std::process::exit(1);
    };
    date.format("%Y-%m").to_string()
}

/// Attribute a session's usage per model, falling back to the session-level
/// model/totals when the file has no `Tally` records.
fn session_model_stats(session: &Session, path: &Path) -> BTreeMap<String, PerModelStats> {
    let mut stats = match per_model_stats(path) {
        Ok(stats) => stats,
        Err(e) => {
            eprintln!("Warning: cannot scan {} — {e}", path.display());
            return BTreeMap::new();
        }
    };
    if stats.is_empty() {
        let model_id = session
            .model
            .as_ref()
            .map(|m| m.model_id.as_str())
            .unwrap_or("unknown");
        let currency = if session.currency.is_empty() {
            Currency::from("CNY").unwrap()
        } else {
            session.currency
        };
        stats.insert(
            model_id.to_string(),
            PerModelStats {
                requests: u64::from(session.requests),
                cost: session.cost,
                currency,
            },
        );
    }
    stats
}

fn main() {
    let (workspace_arg, month_arg) = parse_args();

    let workspace = workspace_arg
        .unwrap_or_else(|| env::current_dir().expect("cannot determine current directory"));
    let workspace = dunce::canonicalize(&workspace).unwrap_or_else(|_| workspace.clone());

    let target_month =
        month_arg.unwrap_or_else(|| chrono::Local::now().format("%Y-%m").to_string());

    println!("Workspace: {}", workspace.display());
    println!("Counting sessions in: {target_month}");
    println!();

    // 1. List session file paths for the target month.
    let paths = match session::list_session_paths(&workspace, Some(&target_month)) {
        Ok(paths) => paths,
        Err(e) => {
            eprintln!("Error listing sessions: {e}");
            std::process::exit(1);
        }
    };

    if paths.is_empty() {
        println!("No sessions found for {target_month}.");
        return;
    }

    println!("Found {} session(s).\n", paths.len());

    // 2. Load each session and attribute usage per model (tally deltas).
    let mut by_day: BTreeMap<String, BTreeMap<String, DayStats>> = BTreeMap::new();
    // Sessions per day — a multi-model session counts once per day.
    let mut day_sessions: BTreeMap<String, u64> = BTreeMap::new();

    for path in &paths {
        let session = match Session::load(path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Warning: skipping {} — {e}", path.display());
                continue;
            }
        };

        // Skip legacy flat-dir files from other months.
        if session::year_month_from_id(&session.id) != target_month {
            continue;
        }

        // Date portion of created_at ("YYYY-MM-DD HH:MM:SS" → "YYYY-MM-DD").
        let day = session
            .created_at
            .split_whitespace()
            .next()
            .unwrap_or("unknown")
            .to_string();

        *day_sessions.entry(day.clone()).or_default() += 1;
        for (model_id, stats) in session_model_stats(&session, path) {
            let entry = by_day
                .entry(day.clone())
                .or_default()
                .entry(model_id)
                .or_default();
            entry.count += 1; // sessions that used this model
            entry.requests += stats.requests;
            *entry.costs.entry(stats.currency).or_insert(0.0) += stats.cost;
        }
    }

    // 3. Print per-day → per-model summary.
    let header = format!(
        "{:<12} {:<30} {:>6} {:>7} {:>10}",
        "Day", "Model", "Sess", "Reqs", "Cost",
    );
    let separator = format!("{:-<12} {:-<30} {:-<6} {:-<7} {:-<10}", "", "", "", "", "");

    let mut grand_count = 0u64;
    let mut grand_requests = 0u64;
    let mut grand_costs: BTreeMap<Currency, f64> = BTreeMap::new();

    println!("══ {target_month} ══\n");
    println!("{header}");
    println!("{separator}");

    for (day, models) in &by_day {
        let mut day_requests = 0u64;
        let mut day_costs: BTreeMap<Currency, f64> = BTreeMap::new();

        for (model_id, stats) in models {
            let cost_str = format_costs(&stats.costs);
            println!(
                "{:<12} {:<30} {:>6} {:>7} {:>10}",
                day, model_id, stats.count, stats.requests, cost_str,
            );
            day_requests += stats.requests;
            merge_costs(&mut day_costs, &stats.costs);
        }
        if models.len() > 1 {
            println!(
                "{:<12} {:<30} {:>6} {:>7} {:>10}",
                "",
                "── day total ──",
                day_sessions[day],
                day_requests,
                format_costs(&day_costs),
            );
        }
        println!();

        grand_count += day_sessions[day];
        grand_requests += day_requests;
        merge_costs(&mut grand_costs, &day_costs);
    }

    println!("{separator}");
    println!(
        "{:<12} {:<30} {:>6} {:>7} {:>10}",
        "TOTAL",
        "",
        grand_count,
        grand_requests,
        format_costs(&grand_costs),
    );
}

#[derive(Default)]
struct DayStats {
    count: u64,
    requests: u64,
    costs: BTreeMap<Currency, f64>,
}
