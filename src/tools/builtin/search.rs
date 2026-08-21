use std::fmt::Write as _;
use std::io;
use std::path::Path;
use tokio_util::sync::CancellationToken;

use serde_json::{Value, json};

// Why grep-searcher over a naive read_to_string + regex::is_match loop:
//
// - Matcher: same regex-automata engine as `regex`, so pattern semantics are
//   unchanged.
// - Streaming scan: memory-mapped / incremental buffered reads; no
//   whole-file String allocation per file.
// - Early exit: stops scanning the moment the line cap or cancel token fires,
//   without having read the rest of the file.
// - Reused buffer: one search buffer across all files instead of an
//   allocation per file.
// - Single continuous pass: one matcher run per file vs per-line is_match
//   calls, keeping SIMD prefilters hot.
// - Binary detection: quits on the first NUL byte in the same pass, like rg,
//   instead of silently failing read_to_string.
// - Lossy UTF-8: files with invalid UTF-8 are searched rather than skipped,
//   so matches are never silently dropped (no false negatives).
// - Line handling: CRLF stripping, line numbers, and BOM sniffing done by
//   fuzz-tested ripgrep code.

use grep_regex::RegexMatcher;
use grep_searcher::sinks::Lossy;
use grep_searcher::{BinaryDetection, Searcher, SearcherBuilder};

use crate::tools::{
    Tool, arg_str, make_workspace_relative, resolve_path, tool_limits, truncate_output,
};

pub struct SearchTool;

impl Tool for SearchTool {
    fn name(&self) -> &str {
        "search"
    }

    fn description(&self) -> &str {
        "Search for a regex pattern in file contents. Returns file:line:content matches. Respects .gitignore."
    }

    fn instruction(&self) -> &str {
        "Search file contents using a regular expression. Returns matches in file:line:content format. Respects .gitignore rules. Use this tool to locate definitions, references, usages, or other patterns across the codebase before reading or editing specific files."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Regular expression (RE2 syntax) to match against each line of file contents"
                },
                "path": {
                    "type": "string",
                    "description": "File or directory to search within, defaults to '.', i.e, search inside workspace. If path is directory, the search is recursive and depth-first, respects .gitignore rules."
                }
            },
            "required": ["pattern"]
        })
    }

    fn execute_inner(
        &self,
        args: &Value,
        workspace: &Path,
        cancel: &CancellationToken,
    ) -> Result<String, String> {
        execute_search(args, workspace, cancel)
    }
}

/// Accumulates output text and search status across searched files.
#[derive(Default)]
struct SearchOutput {
    out: String,
    matched: usize,
    found: bool,
    /// A match beyond the line limit was actually encountered.
    truncated: bool,
    /// Exactly `max_lines` matches were printed when the walk stopped; more may exist.
    limit_reached: bool,
    cancelled: bool,
}

impl SearchOutput {
    /// Search one file, appending matches as `rel:line:text` lines.
    fn search_file(
        &mut self,
        searcher: &mut Searcher,
        matcher: &RegexMatcher,
        path: &Path,
        rel: &str,
        limit: usize,
        cancel: &CancellationToken,
    ) -> io::Result<()> {
        let mut sink = Lossy(|line_no, text| {
            self.found = true;
            if cancel.is_cancelled() {
                self.cancelled = true;
                return Ok(false);
            }
            if self.matched >= limit {
                self.truncated = true;
                return Ok(false);
            }
            self.matched += 1;
            let _ = writeln!(
                self.out,
                "{rel}:{line_no}:{}",
                text.trim_end_matches(['\r', '\n'])
            );
            Ok(true)
        });
        searcher.search_path(matcher, path, &mut sink)
    }
}

pub(super) fn execute_search(
    args: &Value,
    workspace: &Path,
    cancel: &CancellationToken,
) -> Result<String, String> {
    let max_lines = tool_limits().search_max_lines;

    let pattern = arg_str(args, "pattern").ok_or("Missing 'pattern' argument")?;
    let search_path = arg_str(args, "path")
        .map(|p| resolve_path(p, workspace))
        .transpose()
        .map_err(|e| format!("Failed to resolve path: {e}"))?
        .unwrap_or_else(|| workspace.to_path_buf());

    let matcher = RegexMatcher::new(pattern).map_err(|e| format!("Invalid regex: {e}"))?;
    let mut searcher = SearcherBuilder::new()
        .line_number(true)
        .binary_detection(BinaryDetection::quit(b'\0'))
        .build();
    let mut output = SearchOutput::default();

    if search_path.is_file() {
        let rel = make_workspace_relative(&search_path, workspace);
        output
            .search_file(
                &mut searcher,
                &matcher,
                &search_path,
                &rel,
                max_lines,
                cancel,
            )
            .map_err(|e| format!("Failed to read {rel}: {e}"))?;
    } else if search_path.is_dir() {
        for entry in ignore::WalkBuilder::new(&search_path)
            .standard_filters(true)
            .build()
        {
            if cancel.is_cancelled() {
                output.cancelled = true;
                break;
            }
            if output.matched >= max_lines {
                output.limit_reached = true;
                break;
            }
            let Ok(entry) = entry else { continue };
            if !entry.file_type().is_some_and(|ft| ft.is_file()) {
                continue;
            }
            let rel = make_workspace_relative(entry.path(), workspace);
            // Ignore unreadable files; search continues with the rest.
            let _ = output.search_file(
                &mut searcher,
                &matcher,
                entry.path(),
                &rel,
                max_lines - output.matched,
                cancel,
            );
        }
    } else {
        return Err(format!(
            "Path does not exist or is not searchable: {}",
            make_workspace_relative(&search_path, workspace)
        ));
    }

    if cancel.is_cancelled() || output.cancelled {
        let mut out = output.out;
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str("... [search was cancelled]\n");
        Ok(truncate_output(out))
    } else if !output.found {
        Ok("No matches found.".into())
    } else if output.truncated {
        Ok(truncate_output(format!(
            "{}\n... [output truncated at {max_lines} lines; more matches exist but were omitted] ...\n",
            output.out
        )))
    } else if output.limit_reached {
        Ok(truncate_output(format!(
            "{}\n... [output truncated at {max_lines} lines; more matches may exist] ...\n",
            output.out
        )))
    } else {
        Ok(truncate_output(output.out))
    }
}
