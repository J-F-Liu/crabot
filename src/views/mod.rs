pub mod builtin_tools;
pub mod center_pane;
pub mod left_pane;
pub mod model_config;
pub mod right_pane;
pub mod session_view;
pub mod styles;
pub mod system_prompt;
pub mod theme;
pub mod tool_message;
pub mod user_prompt;

// Re-export the style functions for external callers.
pub(crate) use styles::{primary_button, primary_checkbox, primary_toggler};

// Re-export the pane constructors and helpers used by `App::view` / `App::update`.
pub(crate) use center_pane::{center_pane, scroll_to_end};
pub(crate) use left_pane::left_pane;
pub(crate) use right_pane::right_pane;
pub(crate) use styles::divider;
