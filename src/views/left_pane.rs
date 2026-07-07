use iced::{
    Element, Fill, Length,
    widget::{column, container, rule, scrollable, text_editor},
};

use super::model_config::ProviderEntry;
use super::model_config::model_config_view;
use super::session_view::session_view;
use super::styles::{label, pane_side};
use super::system_prompt::{
    PromptSectionState, agents_md_field_view, date_field_view, file_picker_field_view,
    files_field_view, tools_field_view, workspace_field_view,
};
use super::theme::thin_vertical;
use super::tool_list::{BUILTIN_TOOLS, CUSTOM_TOOLS, ToolListState, tools_section};
use super::user_prompt::user_prompt_view;
use crate::Message;
use crate::llm::StreamState;
use crate::model::ModelList;
use crate::system::{FilepathEntry, SystemPrompt};
use crate::user::WorkMode;
use crate::views::session_view::SessionEntry;
use crate::widgets::textarea::TextArea;
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
    builtin_tool_names: &'a [String],
    custom_tool_names: &'a [String],
    user_prompt: &'a TextArea,
    workmode: WorkMode,
    streaming: StreamState,
    session_options: &'a [SessionEntry],
    current_session_id: &'a str,
) -> Element<'a, Message> {
    let agents_md: Element<'a, Message> = if agents_md_exists {
        agents_md_field_view(&system_prompt.agents_md)
    } else {
        container(column![]).into()
    };

    let children: Vec<Element<'a, Message>> = vec![
        model_config_view(provided_models, provider_entries, selected_model)
            .map(Message::ModelConfigEvent),
        rule::horizontal(0).into(),
        label("System Prompt", 140.0),
        file_picker_field_view(
            crate::system::PREAMBLE,
            &system_prompt.preamble,
            preamble_options,
            selected_preamble,
            Message::SelectPreamble,
        ),
        file_picker_field_view(
            crate::system::RULES,
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
        agents_md,
        files_field_view(
            prompt_section_state.files_expanded,
            &system_prompt.files,
            files_content,
        ),
        date_field_view(&system_prompt.date),
        session_view(streaming, session_options, current_session_id),
        label("User Prompt", 140.0),
        user_prompt_view(user_prompt, workmode),
        tools_section(
            BUILTIN_TOOLS,
            tool_list_state.builtin_expanded,
            enabled_tools,
            builtin_tool_names,
        ),
        tools_section(
            CUSTOM_TOOLS,
            tool_list_state.custom_expanded,
            enabled_tools,
            custom_tool_names,
        ),
    ];

    let col = column(children).spacing(8);

    container(scrollable(col.padding([4, 12])).direction(thin_vertical()))
        .width(Length::Fixed(left_w))
        .height(Fill)
        .style(pane_side)
        .into()
}
