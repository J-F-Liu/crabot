//! Convert a session `.jsonl` file into a whole-document JSON file.
//!
//! Usage:
//!   cargo run --release --example jsonl_to_json -- <session.jsonl> [output.json]
//!   cargo run --release --example jsonl_to_json -- <workspace_dir>
//!
//! A single `.jsonl` file is printed to stdout unless an output path is given.
//! A workspace directory converts every session found under `.agent/sessions/`
//! and writes `{id}.json` next to each source `.jsonl`.
//!
//! The output is the serialized [`Session`] struct (the legacy `.json`
//! format): meta fields, full `history`, and the usage tally.

use std::env;
use std::path::{Path, PathBuf};

use crabot::session::{self, Session};

/// Load a session file and serialize it as a pretty-printed JSON document.
fn convert_file(path: &Path) -> Result<String, String> {
    let session = Session::load(path)?;
    serde_json::to_string_pretty(&session).map_err(|e| format!("Failed to serialize: {e}"))
}

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        eprintln!("Usage: jsonl_to_json <session.jsonl> [output.json] | <workspace_dir>");
        std::process::exit(1);
    }

    let input = PathBuf::from(&args[1]);

    if input.is_dir() {
        // Workspace mode: convert every session under .agent/sessions/.
        let paths = match session::list_session_paths(&input) {
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
        let mut ok = 0usize;
        let mut failed = 0usize;
        for path in &paths {
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                println!("Skipping legacy json (not a jsonl): {}", path.display());
                continue;
            }
            match convert_file(path) {
                Ok(json) => {
                    let out = path.with_extension("json");
                    match std::fs::write(&out, json) {
                        Ok(()) => {
                            println!("{} → {}", path.display(), out.display());
                            ok += 1;
                        }
                        Err(e) => {
                            eprintln!("Error writing {}: {e}", out.display());
                            failed += 1;
                        }
                    }
                }
                Err(e) => {
                    eprintln!("Error converting {}: {e}", path.display());
                    failed += 1;
                }
            }
        }
        println!("\nConverted {ok} session(s), {failed} failed.");
        return;
    }

    // Single-file mode.
    let json = match convert_file(&input) {
        Ok(j) => j,
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    };

    match args.get(2) {
        Some(out) => {
            let out = PathBuf::from(out);
            if let Err(e) = std::fs::write(&out, json) {
                eprintln!("Error writing {}: {e}", out.display());
                std::process::exit(1);
            }
            println!("{} → {}", input.display(), out.display());
        }
        None => println!("{json}"),
    }
}
