//! Aggregate session statistics: count sessions and sum costs per day, grouped by model.
//!
//! Usage:
//!   cargo run --release --example session_stats -- <workspace_path>
//!
//! If no workspace path is given, the current directory is used.

use std::collections::BTreeMap;
use std::env;

use crabot::session::{self, Session};

fn main() {
    let workspace = env::args()
        .nth(1)
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| env::current_dir().expect("cannot determine current directory"));

    let workspace = dunce::canonicalize(&workspace).unwrap_or_else(|_| workspace.clone());

    println!("Workspace: {}", workspace.display());
    println!();

    // 1. List all session file paths.
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

    // 2. Load each session and group by day → model_id.
    let mut by_day: BTreeMap<String, BTreeMap<String, DayStats>> = BTreeMap::new();

    for path in &paths {
        let session = match Session::load(path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Warning: skipping {} — {e}", path.display());
                continue;
            }
        };

        // Extract just the date portion from created_at ("YYYY-MM-DD HH:MM:SS" → "YYYY-MM-DD").
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

        let entry = by_day.entry(day).or_default().entry(model_id).or_default();
        entry.count += 1;
        entry.total_cost += session.cost;
        entry.total_input_tokens += session.tokens.input as u64;
        entry.total_output_tokens += session.tokens.output as u64;
    }

    // 3. Print per-day per-model summary and grand total.
    println!(
        "{:<12} {:<28} {:>6} {:>12} {:>12} {:>12}",
        "Day", "Model", "Count", "Cost", "Input Tok", "Output Tok"
    );
    println!(
        "{:-<12} {:-<28} {:-<6} {:-<12} {:-<12} {:-<12}",
        "", "", "", "", "", ""
    );

    let mut grand_count = 0u64;
    let mut grand_cost = 0.0f64;
    let mut grand_input = 0u64;
    let mut grand_output = 0u64;

    for (day, models) in &by_day {
        let mut day_count = 0u64;
        let mut day_cost = 0.0f64;
        let mut day_input = 0u64;
        let mut day_output = 0u64;

        for (model_id, stats) in models {
            println!(
                "{:<12} {:<28} {:>6} {:>12.4} {:>12} {:>12}",
                day,
                model_id,
                stats.count,
                stats.total_cost,
                stats.total_input_tokens,
                stats.total_output_tokens,
            );
            day_count += stats.count;
            day_cost += stats.total_cost;
            day_input += stats.total_input_tokens;
            day_output += stats.total_output_tokens;
        }
        // Day subtotal row (only if multiple models on this day)
        if models.len() > 1 {
            println!(
                "{:<12} {:<28} {:>6} {:>12.4} {:>12} {:>12}",
                "", "── day total ──", day_count, day_cost, day_input, day_output,
            );
        }
        println!();

        grand_count += day_count;
        grand_cost += day_cost;
        grand_input += day_input;
        grand_output += day_output;
    }

    println!(
        "{:-<12} {:-<28} {:-<6} {:-<12} {:-<12} {:-<12}",
        "", "", "", "", "", ""
    );
    println!(
        "{:<12} {:<28} {:>6} {:>12.4} {:>12} {:>12}",
        "TOTAL", "", grand_count, grand_cost, grand_input, grand_output
    );
}

#[derive(Default)]
struct DayStats {
    count: u64,
    total_cost: f64,
    total_input_tokens: u64,
    total_output_tokens: u64,
}
