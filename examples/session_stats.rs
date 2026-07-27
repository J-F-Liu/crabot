//! Aggregate session statistics: count sessions and sum costs per day, grouped by model.
//!
//! Usage:
//!   cargo run --release --example session_stats -- <workspace_path>
//!
//! If no workspace path is given, the current directory is used.

use std::collections::BTreeMap;
use std::env;

use crabot::model::{Currency, currency_symbol};
use crabot::session::{self, Session};

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

fn main() {
    let workspace = env::args()
        .nth(1)
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| env::current_dir().expect("cannot determine current directory"));

    let workspace = dunce::canonicalize(&workspace).unwrap_or_else(|_| workspace.clone());

    println!("Workspace: {}", workspace.display());
    println!();

    // 1. List all session file paths (last 3 months).
    let paths = match session::list_session_paths(&workspace) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Error listing sessions: {e}");
            std::process::exit(1);
        }
    };

    if paths.is_empty() {
        println!("No sessions found.");
        return;
    }

    println!("Found {} session(s).\n", paths.len());

    // 2. Load each session and group by year-month → day → model_id.
    let mut by_month: BTreeMap<String, BTreeMap<String, BTreeMap<String, DayStats>>> =
        BTreeMap::new();

    for path in &paths {
        let session = match Session::load(path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Warning: skipping {} — {e}", path.display());
                continue;
            }
        };

        // Extract year-month from session id (e.g. "20260727-120000" → "2026-07").
        let year_month = session::year_month_from_id(&session.id);

        // Extract date portion from created_at ("YYYY-MM-DD HH:MM:SS" → "YYYY-MM-DD").
        let day = session
            .created_at
            .split_whitespace()
            .next()
            .unwrap_or("unknown")
            .to_string();

        let model_id = session
            .model
            .as_ref()
            .map(|m| m.model_id.as_str())
            .unwrap_or("unknown")
            .to_string();

        let entry = by_month
            .entry(year_month)
            .or_default()
            .entry(day)
            .or_default()
            .entry(model_id)
            .or_default();
        entry.count += 1;
        entry.requests += session.requests as u64;
        // Treat sessions with no currency as CNY (e.g. free models, legacy sessions).
        let currency = if session.currency.is_empty() {
            Currency::from("CNY").unwrap()
        } else {
            session.currency
        };
        *entry.costs.entry(currency).or_insert(0.0) += session.cost;
    }

    // 3. Print per-month → per-day → per-model summary.
    let header = format!(
        "{:<12} {:<30} {:>6} {:>7} {:>10}",
        "Day", "Model", "Sess", "Reqs", "Cost",
    );
    let separator = format!("{:-<12} {:-<30} {:-<6} {:-<7} {:-<10}", "", "", "", "", "");

    let mut grand_count = 0u64;
    let mut grand_requests = 0u64;
    let mut grand_costs: BTreeMap<Currency, f64> = BTreeMap::new();

    for (year_month, days) in &by_month {
        println!("══ {} ══\n", year_month);
        println!("{header}");
        println!("{separator}");

        let mut month_count = 0u64;
        let mut month_requests = 0u64;
        let mut month_costs: BTreeMap<Currency, f64> = BTreeMap::new();

        for (day, models) in days {
            let mut day_count = 0u64;
            let mut day_requests = 0u64;
            let mut day_costs: BTreeMap<Currency, f64> = BTreeMap::new();

            for (model_id, stats) in models {
                let cost_str = format_costs(&stats.costs);
                println!(
                    "{:<12} {:<30} {:>6} {:>7} {:>10}",
                    day, model_id, stats.count, stats.requests, cost_str,
                );
                day_count += stats.count;
                day_requests += stats.requests;
                merge_costs(&mut day_costs, &stats.costs);
            }
            if models.len() > 1 {
                let cost_str = format_costs(&day_costs);
                println!(
                    "{:<12} {:<30} {:>6} {:>7} {:>10}",
                    "", "── day total ──", day_count, day_requests, cost_str,
                );
            }
            println!();

            month_count += day_count;
            month_requests += day_requests;
            merge_costs(&mut month_costs, &day_costs);
        }

        println!("{separator}");
        let month_cost_str = format_costs(&month_costs);
        println!(
            "{:<12} {:<30} {:>6} {:>7} {:>10}",
            "", "── month total ──", month_count, month_requests, month_cost_str,
        );
        println!();
        println!();

        grand_count += month_count;
        grand_requests += month_requests;
        merge_costs(&mut grand_costs, &month_costs);
    }

    println!("{separator}");
    let grand_cost_str = format_costs(&grand_costs);
    println!(
        "{:<12} {:<30} {:>6} {:>7} {:>10}",
        "GRAND TOTAL", "", grand_count, grand_requests, grand_cost_str,
    );
}

#[derive(Default)]
struct DayStats {
    count: u64,
    requests: u64,
    costs: BTreeMap<Currency, f64>,
}
