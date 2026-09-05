use iced::{
    Element, Fill, Length,
    widget::{column, container, scrollable},
};

use super::model_config::model_config_view;
use super::session_list::session_view;
use super::styles::{label, pane_side};
use super::system_prompt::{
    agents_md_field_view, date_field_view, preamble_picker_view, skills_picker_view,
    tools_field_view, workspace_field_view,
};
use super::theme::thin_vertical;
use super::tool_list::{
    BUILTIN_TOOLS, CUSTOM_TOOLS, ToolListState, mcp_tools_section, tools_section,
};
use super::user_prompt::user_prompt_view;
use crate::FilepathEntry;
use crate::app::{ConversationState, PromptWorkspaceState, ToolState};
use crate::llm::DialogPhase;
use crate::views::session_list::SessionEntry;
use crate::widgets::textarea::TextArea;
use crate::{LeftPaneEvent, PromptEvent, ToolEvent};
use crabot::user::WorkMode;
use std::collections::HashSet;

pub(crate) fn left_pane<'a>(
    settings: &'a crabot::settings::Settings,
    prompt: &'a PromptWorkspaceState,
    conversation: &'a ConversationState,
    tools: &'a ToolState,
    models: &'a crabot::model::ModelList,
) -> Element<'a, LeftPaneEvent> {
    let lang = settings.language;
    let left_w: f32 = settings.left_pane_width;
    let selected_model: &String = &conversation.viewing().selected_model;
    let selected_preamble: &str = &conversation.viewing().selected_preamble;
    let agents_md_exists: bool = prompt.agents_md_exists;
    let tool_list_state: &ToolListState = &tools.tool_list_state;
    let preamble_options: &[FilepathEntry] = &prompt.preamble_options;
    let selected_skills: &[String] = &settings.selected_skills;
    let skills_options: &[FilepathEntry] = &prompt.skills_options;
    let workspace_options: &[FilepathEntry] = &prompt.workspace_options;
    let files: &crate::app::FileTreePane = &prompt.files;
    let workspace_set = !prompt.workspace.1.as_os_str().is_empty();
    let enabled_tools: &HashSet<String> = &tools.enabled_tools;
    let tool_registry: &crabot::tools::ToolRegistry = &tools.tool_registry;
    let user_prompt: &TextArea = &prompt.user_prompt;
    let workmode: WorkMode = prompt.workmode;
    let workmode_enabled: bool = prompt.workmode_enabled;
    let prompt_recipes: &[String] = {
        let key = prompt.workmode.name.to_lowercase();
        settings
            .prompt_recipes
            .get(&key)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    };
    let recipe_dropdown_expanded: bool = prompt.recipe_dropdown_expanded;
    let streaming: DialogPhase = conversation.viewing().session_state.phase;
    let session_options: &[SessionEntry] = &conversation.session_list;
    let current_session_id: &str = &conversation.viewing().session.id;
    let enabled_mcp_servers: &HashSet<String> = &tools.enabled_mcp_servers;
    container(
        column![
            container(
                model_config_view(models, selected_model, lang).map(LeftPaneEvent::ModelConfig),
            )
            .padding([2, 10]),
            scrollable(
                column![
                    label(lang.tr("System Prompt"), 140.0),
                    preamble_picker_view(
                        prompt.preamble_enabled,
                        preamble_options,
                        selected_preamble,
                        lang,
                    )
                    .map(LeftPaneEvent::Prompt),
                    skills_picker_view(
                        prompt.skills_enabled,
                        prompt.skills_menu_expanded,
                        skills_options,
                        selected_skills,
                        lang,
                    )
                    .map(LeftPaneEvent::Prompt),
                    tools_field_view(&prompt.tools, lang).map(LeftPaneEvent::Prompt),
                    workspace_field_view(&prompt.workspace, workspace_options, lang)
                        .map(LeftPaneEvent::Prompt),
                    if agents_md_exists {
                        agents_md_field_view(&prompt.agents_md).map(LeftPaneEvent::Prompt)
                    } else {
                        column![].into()
                    },
                    date_field_view(&prompt.date, lang).map(LeftPaneEvent::Prompt),
                    session_view(
                        streaming,
                        session_options,
                        current_session_id,
                        conversation.session_list_loading,
                        lang,
                    )
                    .map(LeftPaneEvent::Conversation),
                    label(lang.tr("User Prompt"), 140.0),
                    user_prompt_view(
                        user_prompt,
                        workmode,
                        workmode_enabled,
                        prompt_recipes,
                        recipe_dropdown_expanded,
                        files,
                        workspace_set,
                        lang,
                    )
                    .map(LeftPaneEvent::Prompt),
                    tools_section(
                        BUILTIN_TOOLS,
                        tool_list_state.builtin_expanded,
                        enabled_tools,
                        &tool_registry.builtin_names,
                        lang,
                    )
                    .map(map_tool_list_event),
                    tools_section(
                        CUSTOM_TOOLS,
                        tool_list_state.custom_expanded,
                        enabled_tools,
                        &tool_registry.custom_names,
                        lang,
                    )
                    .map(map_tool_list_event),
                    mcp_tools_section(
                        tool_list_state.mcp_expanded,
                        enabled_tools,
                        &tool_registry.mcp,
                        enabled_mcp_servers,
                        lang,
                    )
                    .map(map_tool_list_event),
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

/// Map a [`super::tool_list::ToolListEvent`] to the pane [`LeftPaneEvent`].
fn map_tool_list_event(event: super::tool_list::ToolListEvent) -> LeftPaneEvent {
    use super::tool_list::ToolListEvent;
    match event {
        ToolListEvent::ExpandSection(name) => {
            LeftPaneEvent::Prompt(PromptEvent::ToggleExpanded(name))
        }
        ToolListEvent::ToggleMcpServer(server, enabled) => {
            LeftPaneEvent::Tools(ToolEvent::ToggleMcpServer(server, enabled))
        }
        ToolListEvent::ToggleAgentTool(name, enabled) => {
            LeftPaneEvent::Tools(ToolEvent::ToggleAgentTool(name, enabled))
        }
    }
}
