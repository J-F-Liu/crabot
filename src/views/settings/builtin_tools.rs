use iced::{
    Alignment, Element, Length,
    widget::{column, container, pick_list, row, text, text_input, toggler},
};

use super::{SettingsEvent, SettingsState, SettingsTab, form_card_style};
use crate::views::model_config::ProviderEntry;
use crate::views::theme::{CRABOT_PRIMARY, color_muted};
use crabot::model::Model;
use crabot::tools::ToolLimits;

// ── Tool-limit fields ──────────────────────────────────────────────

/// Identifies one numeric field of [`ToolLimits`] being edited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolLimitField {
    CommandTimeoutMs,
    MaxCommandTimeoutMs,
    HeadTailBytes,
    MaxOutputBytes,
    ReadMaxLines,
    ReadMaxBytes,
    FindMaxLines,
    SearchMaxLines,
    FetchMaxBodyBytes,
    FetchTimeoutMs,
    McpConnectTimeoutMs,
    McpCallTimeoutMs,
}

impl ToolLimitField {
    pub(crate) const ALL: [ToolLimitField; 12] = [
        ToolLimitField::CommandTimeoutMs,
        ToolLimitField::MaxCommandTimeoutMs,
        ToolLimitField::HeadTailBytes,
        ToolLimitField::MaxOutputBytes,
        ToolLimitField::ReadMaxLines,
        ToolLimitField::ReadMaxBytes,
        ToolLimitField::FindMaxLines,
        ToolLimitField::SearchMaxLines,
        ToolLimitField::FetchMaxBodyBytes,
        ToolLimitField::FetchTimeoutMs,
        ToolLimitField::McpConnectTimeoutMs,
        ToolLimitField::McpCallTimeoutMs,
    ];

    fn label(self) -> &'static str {
        match self {
            ToolLimitField::CommandTimeoutMs => "bash timeout (ms)",
            ToolLimitField::MaxCommandTimeoutMs => "bash max timeout (ms)",
            ToolLimitField::HeadTailBytes => "truncation head+tail bytes (each)",
            ToolLimitField::MaxOutputBytes => "max output bytes",
            ToolLimitField::ReadMaxLines => "read max lines",
            ToolLimitField::ReadMaxBytes => "read max bytes",
            ToolLimitField::FindMaxLines => "find max lines",
            ToolLimitField::SearchMaxLines => "search max lines",
            ToolLimitField::FetchMaxBodyBytes => "fetch max body bytes",
            ToolLimitField::FetchTimeoutMs => "fetch timeout (ms)",
            ToolLimitField::McpConnectTimeoutMs => "mcp connect timeout (ms)",
            ToolLimitField::McpCallTimeoutMs => "mcp call timeout (ms)",
        }
    }
}

/// Raw-text working copies of [`ToolLimits`] — parsed on Save so in-progress
/// typing is never clobbered by re-formatting.
#[derive(Debug, Clone, Default)]
pub(crate) struct ToolLimitStrings {
    pub command_timeout_ms: String,
    pub max_command_timeout_ms: String,
    pub head_tail_bytes: String,
    pub max_output_bytes: String,
    pub read_max_lines: String,
    pub read_max_bytes: String,
    pub find_max_lines: String,
    pub search_max_lines: String,
    pub fetch_max_body_bytes: String,
    pub fetch_timeout_ms: String,
    pub mcp_connect_timeout_ms: String,
    pub mcp_call_timeout_ms: String,
}

impl ToolLimitStrings {
    pub(crate) fn from_limits(limits: &ToolLimits) -> Self {
        Self {
            command_timeout_ms: limits.command_timeout_ms.to_string(),
            max_command_timeout_ms: limits.max_command_timeout_ms.to_string(),
            head_tail_bytes: limits.head_tail_bytes.to_string(),
            max_output_bytes: limits.max_output_bytes.to_string(),
            read_max_lines: limits.read_max_lines.to_string(),
            read_max_bytes: limits.read_max_bytes.to_string(),
            find_max_lines: limits.find_max_lines.to_string(),
            search_max_lines: limits.search_max_lines.to_string(),
            fetch_max_body_bytes: limits.fetch_max_body_bytes.to_string(),
            fetch_timeout_ms: limits.fetch_timeout_ms.to_string(),
            mcp_connect_timeout_ms: limits.mcp_connect_timeout_ms.to_string(),
            mcp_call_timeout_ms: limits.mcp_call_timeout_ms.to_string(),
        }
    }

    /// Parse all fields, falling back per-field to the defaults when blank
    /// or unparseable.
    pub(crate) fn to_limits(&self) -> ToolLimits {
        let defaults = ToolLimits::default();
        ToolLimits {
            command_timeout_ms: parse(&self.command_timeout_ms, defaults.command_timeout_ms),
            max_command_timeout_ms: parse(
                &self.max_command_timeout_ms,
                defaults.max_command_timeout_ms,
            ),
            head_tail_bytes: parse(&self.head_tail_bytes, defaults.head_tail_bytes),
            max_output_bytes: parse(&self.max_output_bytes, defaults.max_output_bytes),
            read_max_lines: parse(&self.read_max_lines, defaults.read_max_lines),
            read_max_bytes: parse(&self.read_max_bytes, defaults.read_max_bytes),
            find_max_lines: parse(&self.find_max_lines, defaults.find_max_lines),
            search_max_lines: parse(&self.search_max_lines, defaults.search_max_lines),
            fetch_max_body_bytes: parse(&self.fetch_max_body_bytes, defaults.fetch_max_body_bytes),
            fetch_timeout_ms: parse(&self.fetch_timeout_ms, defaults.fetch_timeout_ms),
            mcp_connect_timeout_ms: parse(
                &self.mcp_connect_timeout_ms,
                defaults.mcp_connect_timeout_ms,
            ),
            mcp_call_timeout_ms: parse(&self.mcp_call_timeout_ms, defaults.mcp_call_timeout_ms),
        }
    }

    pub(crate) fn get(&self, field: ToolLimitField) -> &str {
        match field {
            ToolLimitField::CommandTimeoutMs => &self.command_timeout_ms,
            ToolLimitField::MaxCommandTimeoutMs => &self.max_command_timeout_ms,
            ToolLimitField::HeadTailBytes => &self.head_tail_bytes,
            ToolLimitField::MaxOutputBytes => &self.max_output_bytes,
            ToolLimitField::ReadMaxLines => &self.read_max_lines,
            ToolLimitField::ReadMaxBytes => &self.read_max_bytes,
            ToolLimitField::FindMaxLines => &self.find_max_lines,
            ToolLimitField::SearchMaxLines => &self.search_max_lines,
            ToolLimitField::FetchMaxBodyBytes => &self.fetch_max_body_bytes,
            ToolLimitField::FetchTimeoutMs => &self.fetch_timeout_ms,
            ToolLimitField::McpConnectTimeoutMs => &self.mcp_connect_timeout_ms,
            ToolLimitField::McpCallTimeoutMs => &self.mcp_call_timeout_ms,
        }
    }

    pub(crate) fn get_mut(&mut self, field: ToolLimitField) -> &mut String {
        match field {
            ToolLimitField::CommandTimeoutMs => &mut self.command_timeout_ms,
            ToolLimitField::MaxCommandTimeoutMs => &mut self.max_command_timeout_ms,
            ToolLimitField::HeadTailBytes => &mut self.head_tail_bytes,
            ToolLimitField::MaxOutputBytes => &mut self.max_output_bytes,
            ToolLimitField::ReadMaxLines => &mut self.read_max_lines,
            ToolLimitField::ReadMaxBytes => &mut self.read_max_bytes,
            ToolLimitField::FindMaxLines => &mut self.find_max_lines,
            ToolLimitField::SearchMaxLines => &mut self.search_max_lines,
            ToolLimitField::FetchMaxBodyBytes => &mut self.fetch_max_body_bytes,
            ToolLimitField::FetchTimeoutMs => &mut self.fetch_timeout_ms,
            ToolLimitField::McpConnectTimeoutMs => &mut self.mcp_connect_timeout_ms,
            ToolLimitField::McpCallTimeoutMs => &mut self.mcp_call_timeout_ms,
        }
    }
}

/// Parse a positive integer, falling back to `default` when blank or invalid.
fn parse<T>(s: &str, default: T) -> T
where
    T: std::str::FromStr + PartialOrd + From<u8>,
{
    s.trim()
        .parse()
        .ok()
        .filter(|v| *v > T::from(0))
        .unwrap_or(default)
}

// ── Page ───────────────────────────────────────────────────────────

const TIERS: [(&str, &str); 3] = [("easy", "Easy"), ("medium", "Medium"), ("hard", "Hard")];

pub(super) fn builtin_tools_page<'a>(state: &'a SettingsState) -> Element<'a, SettingsEvent> {
    let header = text("Builtin Tools")
        .size(13)
        .font(iced::Font {
            weight: iced::font::Weight::Bold,
            ..iced::Font::DEFAULT
        })
        .color(CRABOT_PRIMARY);

    column![
        header,
        agent_card(state),
        tool_limits_card(state),
        task_models_card(state),
        super::save_action_row(
            state,
            SettingsTab::BuiltinTools,
            SettingsEvent::SaveBuiltinTools,
        ),
    ]
    .spacing(12)
    .into()
}

// ── Agent card ─────────────────────────────────────────────────────

fn agent_card(state: &SettingsState) -> Element<'_, SettingsEvent> {
    let input = text_input("100", &state.working_max_iterations)
        .on_input(SettingsEvent::EditMaxIterations)
        .width(Length::Fixed(110.0))
        .padding(4)
        .size(13);

    container(
        column![
            section_title("Agent"),
            row![
                text("Max iterations").size(13).width(180.0),
                input,
                text("Tool-calling rounds before the agent gives up.")
                    .size(11)
                    .color(color_muted()),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        ]
        .spacing(8),
    )
    .padding([10, 12])
    .style(form_card_style)
    .width(Length::Fill)
    .into()
}

// ── Tool limits card ───────────────────────────────────────────────

fn tool_limits_card(state: &SettingsState) -> Element<'_, SettingsEvent> {
    let rows: Vec<Element<'_, SettingsEvent>> = ToolLimitField::ALL
        .iter()
        .map(|&field| {
            let value = state.working_tool_limits.get(field);
            let input = text_input("", value)
                .on_input(move |v| SettingsEvent::EditToolLimit(field, v))
                .width(Length::Fixed(110.0))
                .padding(4)
                .size(13);
            row![text(field.label()).size(13).width(180.0), input,]
                .spacing(8)
                .align_y(Alignment::Center)
                .into()
        })
        .collect();

    container(column![section_title("Tool Limits"), column(rows).spacing(6)].spacing(8))
        .padding([10, 12])
        .style(form_card_style)
        .width(Length::Fill)
        .into()
}

// ── Sub-agent task models card ─────────────────────────────────────

fn task_models_card<'a>(state: &'a SettingsState) -> Element<'a, SettingsEvent> {
    let providers: Vec<ProviderEntry> = state
        .working_models
        .providers
        .iter()
        .map(|(id, p)| ProviderEntry {
            id: id.clone(),
            name: p.name.clone(),
        })
        .collect();

    let rows: Vec<Element<'a, SettingsEvent>> = TIERS
        .iter()
        .map(|&(tier, label)| tier_row(state, tier, label, providers.clone()))
        .collect();

    container(
        column![
            section_title("Sub-agent Models"),
            text("Models used by the task tool per difficulty tier. Inherit uses the parent session's model.")
                .size(11)
                .color(color_muted()),
            column(rows).spacing(6),
        ]
        .spacing(8),
    )
    .padding([10, 12])
    .style(form_card_style)
    .width(Length::Fill)
    .into()
}

fn tier_row<'a>(
    state: &'a SettingsState,
    tier: &'static str,
    label: &'static str,
    providers: Vec<ProviderEntry>,
) -> Element<'a, SettingsEvent> {
    let cfg = state.working_task_models.get_config(tier);
    let inherit = cfg.is_empty();
    let no_providers = providers.is_empty();

    let pickers: Element<'a, SettingsEvent> = if inherit {
        text("Inherit parent model")
            .size(12)
            .color(color_muted())
            .into()
    } else {
        let selected_provider = providers.iter().find(|e| e.id == cfg.provider_id).cloned();
        let models: Vec<Model> = state
            .working_models
            .providers
            .get(&cfg.provider_id)
            .map(|p| p.models.clone())
            .unwrap_or_default();
        let selected_model = state.working_models.get_model(cfg).cloned();
        row![
            pick_list(providers, selected_provider, move |e| {
                SettingsEvent::TaskModelSelectProvider(tier, e.id)
            })
            .width(Length::Fill),
            pick_list(models, selected_model, move |m: Model| {
                SettingsEvent::TaskModelSelectModel(tier, m.id)
            })
            .width(Length::Fill),
        ]
        .spacing(4)
        .align_y(Alignment::Center)
        .into()
    };

    let toggle: Element<'a, SettingsEvent> = if no_providers {
        // Without at least one provider there is nothing to pick — lock
        // the toggle ON so the user can't flip into a broken state.
        toggler(true)
            .label("Inherit")
            .text_size(12)
            .style(crate::views::primary_toggler)
            .into()
    } else {
        toggler(inherit)
            .label("Inherit")
            .text_size(12)
            .on_toggle(move |v| SettingsEvent::TaskModelInherit(tier, v))
            .style(crate::views::primary_toggler)
            .into()
    };

    row![
        text(label).size(13).width(60.0),
        toggle,
        container(pickers).width(Length::Fill),
    ]
    .spacing(8)
    .align_y(Alignment::Center)
    .into()
}

// ── Shared ─────────────────────────────────────────────────────────

fn section_title(title: &'static str) -> Element<'static, SettingsEvent> {
    text(title)
        .size(12)
        .font(iced::Font {
            weight: iced::font::Weight::Bold,
            ..iced::Font::DEFAULT
        })
        .color(color_muted())
        .into()
}
