use iced::{
    Element, Fill, Length,
    widget::{column, container, scrollable, text_editor},
};

use super::model_config::ProviderEntry;
use super::model_config::model_config_view;
use super::session_list::session_view;
use super::styles::{label, pane_side};
use super::system_prompt::{
    PromptSectionState, agents_md_field_view, date_field_view, file_picker_field_view,
    files_field_view, tools_field_view, workspace_field_view,
};
use super::theme::thin_vertical;
use super::tool_list::{
    BUILTIN_TOOLS, CUSTOM_TOOLS, ToolListState, mcp_tools_section, tools_section,
};
use super::user_prompt::user_prompt_view;
use crate::Message;
use crate::llm::DialogPhase;
use crate::tools;
use crate::views::session_list::SessionEntry;
use crate::widgets::textarea::TextArea;
use crabot::model::ModelList;
use crabot::system::{FilepathEntry, SystemPrompt};
use crabot::user::WorkMode;
use std::collections::HashSet;

#[allow(clippy::too_many_arguments)]
pub(crate) fn left_pane<'a>(
    left_w: f32,
    provided_models: &'a ModelList,
    provider_entries: &'a [ProviderEntry],
    selected_model: &'a String,
    system_prompt: &'a SystemPrompt,
    agents_md_exists: bool,
    prompt_section_state: &'a PromptSectionState,
    tool_list_state: &'a ToolListState,
    selected_preamble: &'a str,
    preamble_options: &'a [FilepathEntry],
    selected_rules: &'a str,
    rules_options: &'a [FilepathEntry],
    workspace_options: &'a [FilepathEntry],
    files_content: &'a text_editor::Content,
    tools_content: &'a text_editor::Content,
    enabled_tools: &'a HashSet<String>,
    tool_registry: &'a tools::ToolRegistry,
    user_prompt: &'a TextArea,
    workmode: WorkMode,
    workmode_enabled: bool,
    prompt_recipes: &'a [String],
    recipe_dropdown_expanded: bool,
    streaming: DialogPhase,
    session_options: &'a [SessionEntry],
    current_session_id: &'a str,
    enabled_mcp_servers: &'a HashSet<String>,
) -> Element<'a, Message> {
    container(
        column![
            container(
                model_config_view(provided_models, provider_entries, selected_model)
                    .map(Message::ModelConfigEvent),
            )
            .padding([2, 10]),
            scrollable(
                column![
                    label("System Prompt", 140.0),
                    file_picker_field_view(
                        crabot::system::PREAMBLE,
                        &system_prompt.preamble,
                        preamble_options,
                        selected_preamble,
                        Message::SelectPreamble,
                    ),
                    file_picker_field_view(
                        crabot::system::RULES,
                        &system_prompt.rules,
                        rules_options,
                        selected_rules,
                        Message::SelectRules,
                    ),
                    tools_field_view(
                        prompt_section_state.tools_expanded,
                        &system_prompt.tools,
                        tools_content,
                    ),
                    workspace_field_view(&system_prompt.workspace, workspace_options),
                    if agents_md_exists {
                        agents_md_field_view(&system_prompt.agents_md)
                    } else {
                        column![].into()
                    },
                    files_field_view(
                        prompt_section_state.files_expanded,
                        &system_prompt.files,
                        files_content,
                    ),
                    date_field_view(&system_prompt.date),
                    session_view(streaming, session_options, current_session_id),
                    label("User Prompt", 140.0),
                    user_prompt_view(
                        user_prompt,
                        workmode,
                        workmode_enabled,
                        prompt_recipes,
                        recipe_dropdown_expanded,
                    ),
                    tools_section(
                        BUILTIN_TOOLS,
                        tool_list_state.builtin_expanded,
                        enabled_tools,
                        &tool_registry.builtin_names,
                    ),
                    tools_section(
                        CUSTOM_TOOLS,
                        tool_list_state.custom_expanded,
                        enabled_tools,
                        &tool_registry.custom_names,
                    ),
                    mcp_tools_section(
                        tool_list_state.mcp_expanded,
                        enabled_tools,
                        &tool_registry.mcp,
                        enabled_mcp_servers,
                    ),
                ]
                .spacing(8)
                .padding([4, 12]),
            )
            .direction(thin_vertical())
            .height(Fill),
        ]
        .spacing(4),
    )
    .width(Length::Fixed(left_w))
    .height(Fill)
    .style(pane_side)
    .into()
}
