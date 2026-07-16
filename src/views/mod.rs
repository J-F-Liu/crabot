pub mod center_pane;
pub mod left_pane;
pub mod modal;
pub mod model_config;
pub mod right_pane;
pub mod search_bar;
pub mod session_list;
pub mod session_state;
pub mod styles;
pub mod system_prompt;
pub mod theme;
pub mod tool_list;
pub mod tool_message;
pub mod user_prompt;

// Re-export the style functions for external callers.
pub(crate) use styles::{
    disabled_dropdown_style, primary_button, primary_checkbox, primary_toggler, secondary_button,
};

// Re-export the pane constructors and helpers used by `App::view` / `App::update`.
pub(crate) use center_pane::{
    SEARCH_INPUT, center_pane, measure_turn_offsets, scroll_to_end, scroll_to_turn_at,
};
pub(crate) use left_pane::left_pane;
pub(crate) use modal::workspace_modal;
pub(crate) use right_pane::right_pane;
pub(crate) use search_bar::SearchEvent;
pub(crate) use session_state::{SessionEvent, SessionState};
pub(crate) use styles::DividerState;
pub(crate) use styles::divider;
pub(crate) use system_prompt::{build_workspace_options, load_prompt_options};
