use iced::{
    Element, Fill, Length,
    widget::{column, container, rule, scrollable, text_editor},
};

use super::model_config::ProviderEntry;
use super::model_config::model_config_view;
use super::session_view::session_view;
use super::styles::{label, pane_side};
use super::system_prompt::{
    date_field_view, files_field_view, preamble_field_view, rules_field_view, tools_field_view,
    workspace_field_view,
};
use super::tool_list::tools_section;
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
    rules_expanded: bool,
    tools_expanded: bool,
    files_expanded: bool,
    selected_preamble: &'a str,
    preamble_options: &'a [FilepathEntry],
    workspace_options: &'a [FilepathEntry],
    rules_content: &'a TextArea,
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
    let col = column![
        model_config_view(provided_models, provider_entries, selected_model)
            .map(Message::ModelConfigEvent),
        rule::horizontal(0),
        label("System Prompt", 140.0),
        preamble_field_view(&system_prompt.preamble, preamble_options, selected_preamble,),
        rules_field_view(rules_expanded, &system_prompt.rules, rules_content,),
        tools_field_view(tools_expanded, &system_prompt.tools, tools_content,),
        workspace_field_view(&system_prompt.workspace, workspace_options),
        files_field_view(files_expanded, &system_prompt.files, files_content,),
        date_field_view(&system_prompt.date),
        session_view(streaming, session_options, current_session_id),
        label("User Prompt", 140.0),
        user_prompt_view(user_prompt, workmode),
        tools_section("Builtin Tools", enabled_tools, builtin_tool_names),
        tools_section("Custom Tools", enabled_tools, custom_tool_names),
    ]
    .spacing(8);

    container(scrollable(col.padding([4, 12])))
        .width(Length::Fixed(left_w))
        .height(Fill)
        .style(pane_side)
        .into()
}
