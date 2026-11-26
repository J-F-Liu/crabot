//! Assembles the system prompt from the enabled left-pane sections.

use crate::app::prompt;
use crate::app::{PromptOptions, PromptWorkspaceState};

/// Minimal prompt used when no section contributes any text.
const DEFAULT_PROMPT: &str = "You are a helpful software engineer assistant.";

/// Build the prompt for `options`, reading the selected preamble and skills.
pub(crate) fn build(state: &PromptWorkspaceState, options: &PromptOptions) -> String {
    let preamble = options
        .preamble_enabled
        .then(|| prompt::load_prompt_file(&state.preamble_options, &options.selected_preamble));
    compose(state, options, preamble.as_deref())
}

/// Like [`build`], but with a caller-provided preamble (`None` omits it).
///
/// Every section can be disabled or empty, so the result always carries at
/// least [`DEFAULT_PROMPT`].
pub(crate) fn compose(
    state: &PromptWorkspaceState,
    options: &PromptOptions,
    preamble: Option<&str>,
) -> String {
    let mut prompt = String::new();
    if let Some(content) = preamble {
        push_section(&mut prompt, content);
    }
    if options.workmode_enabled
        && let Some(contents) = crabot::setup::ASSETS
            .get_file("workmode.md")
            .and_then(|file| file.contents_utf8())
    {
        push_section(&mut prompt, contents);
    }
    if options.skills_enabled {
        push_section(
            &mut prompt,
            &prompt::load_prompt_files(&state.skills_options, &options.selected_skills),
        );
    }
    if options.workspace_enabled && options.workspace.is_dir() {
        let path = crabot::tools::convert_path_to_unix_style(&options.workspace);
        push_section(&mut prompt, &format!("Current Workspace: {path}"));
    }
    if options.agents_md_enabled {
        push_section(&mut prompt, &options.agents_md);
    }
    if options.date_enabled && !state.date.1.is_empty() {
        push_section(&mut prompt, &format!("Current Date: {}", state.date.1));
    }
    with_default(prompt)
}

/// Replace a blank composition with [`DEFAULT_PROMPT`], so no request carries
/// an empty system prompt.
fn with_default(prompt: String) -> String {
    if prompt.trim().is_empty() {
        DEFAULT_PROMPT.to_string()
    } else {
        prompt
    }
}

/// Append `content` plus a newline unless it is empty.
fn push_section(prompt: &mut String, content: &str) {
    if !content.is_empty() {
        prompt.push_str(content);
        prompt.push('\n');
    }
}
