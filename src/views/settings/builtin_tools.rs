use iced::{
    Alignment, Element, Length,
    widget::{column, container, pick_list, row, text, text_input, toggler},
};

use super::{BOLD, SettingsEvent, SettingsState, SettingsTab, form_card_style, section_header};
use crate::views::model_config::ProviderEntry;
use crate::views::theme::color_muted;
use crabot::model::{Model, ModelConfig};
use crabot::tools::ToolLimits;

// ── Events ──────────────────────────────────────────────────────────

/// Events for the Builtin Tools tab: agent limits, tool limits, task models.
#[derive(Debug, Clone)]
pub(crate) enum BuiltinToolsEvent {
    /// Edit the max agent-loop iterations field (raw text).
    EditMaxIterations(String),
    /// Edit the context-fill renew threshold field (raw text).
    EditFillRatioThreshold(String),
    /// Edit the stream stall timeout field (s, raw text; 0 disables).
    EditStreamStallTimeout(String),
    /// Edit one built-in tool limit field (raw text).
    EditToolLimit(ToolLimitField, String),
    /// Pick a provider for a task sub-agent difficulty tier.
    TaskModelSelectProvider(&'static str, String),
    /// Pick a model for a task sub-agent difficulty tier.
    TaskModelSelectModel(&'static str, String),
    /// Toggle "inherit the parent session's model" for a tier.
    TaskModelInherit(&'static str, bool),
    /// Persist agent limits, tool limits, and task models.
    SaveBuiltinTools,
}

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

/// Parse a numeric field: invalid (or 0 unless `allow_zero`) falls back to
/// `default`; `max` clamps the value when given.
pub(super) fn parse_num<T>(s: &str, default: T, max: Option<T>, allow_zero: bool) -> T
where
    T: std::str::FromStr + PartialOrd + From<u8> + Copy,
{
    s.trim()
        .parse()
        .ok()
        .filter(|&v| allow_zero || v > T::from(0))
        .map(|v| match max {
            Some(m) if v > m => m,
            _ => v,
        })
        .unwrap_or(default)
}

/// Parse a positive integer, falling back to `default` when blank or invalid.
fn parse<T>(s: &str, default: T) -> T
where
    T: std::str::FromStr + PartialOrd + From<u8> + Copy,
{
    parse_num(s, default, None, false)
}

// ── Page ───────────────────────────────────────────────────────────

const TIERS: [(&str, &str); 3] = [("easy", "Easy"), ("medium", "Medium"), ("hard", "Hard")];

pub(super) fn builtin_tools_page<'a>(state: &'a SettingsState) -> Element<'a, SettingsEvent> {
    let header = section_header("Builtin Tools");

    column![
        header,
        agent_card(state),
        tool_limits_card(state),
        task_models_card(state),
        super::save_action_row(
            state,
            SettingsTab::BuiltinTools,
            SettingsEvent::BuiltinTools(BuiltinToolsEvent::SaveBuiltinTools),
        ),
    ]
    .spacing(12)
    .into()
}

// ── Agent card ─────────────────────────────────────────────────────

/// Labeled numeric-setting row: label + input + muted hint.
fn setting_row<'a>(
    label: &'static str,
    input: Element<'a, SettingsEvent>,
    hint: &'static str,
) -> Element<'a, SettingsEvent> {
    row![
        text(label).size(13).width(180.0),
        input,
        text(hint).size(11).color(color_muted()),
    ]
    .spacing(8)
    .align_y(Alignment::Center)
    .into()
}

/// Numeric text input bound to a settings field.
fn num_input<'a>(
    placeholder: &'static str,
    value: &'a str,
    on_input: impl Fn(String) -> SettingsEvent + 'a,
) -> Element<'a, SettingsEvent> {
    text_input(placeholder, value)
        .on_input(on_input)
        .width(Length::Fixed(110.0))
        .padding(4)
        .size(13)
        .into()
}

fn agent_card(state: &SettingsState) -> Element<'_, SettingsEvent> {
    container(
        column![
            section_title("Agent"),
            setting_row(
                "Max iterations",
                num_input("100", &state.working_max_iterations, move |v| {
                    SettingsEvent::BuiltinTools(BuiltinToolsEvent::EditMaxIterations(v))
                },),
                "Tool-calling rounds before the agent gives up.",
            ),
            setting_row(
                "Renew threshold (%)",
                num_input("25", &state.working_fill_ratio_threshold, move |v| {
                    SettingsEvent::BuiltinTools(BuiltinToolsEvent::EditFillRatioThreshold(v))
                },),
                "Context fill ratio at which the agent is reminded to consider renewing.",
            ),
            setting_row(
                "Stream stall timeout (s)",
                num_input("120", &state.working_stream_stall_timeout, move |v| {
                    SettingsEvent::BuiltinTools(BuiltinToolsEvent::EditStreamStallTimeout(v))
                },),
                "Seconds with no stream data before giving up. 0 = off.",
            ),
        ]
        .spacing(8),
    )
    .padding([10, 12])
    .style(form_card_style)
    .width(Length::Fill)
    .into()
}

// ── Tool limits card ───────────────────────────────────────────────

fn limit_row(state: &SettingsState, field: ToolLimitField) -> Element<'_, SettingsEvent> {
    let value = state.working_tool_limits.get(field);
    let input = num_input("", value, move |v| {
        SettingsEvent::BuiltinTools(BuiltinToolsEvent::EditToolLimit(field, v))
    });
    row![text(field.label()).size(13).width(180.0), input,]
        .spacing(8)
        .align_y(Alignment::Center)
        .into()
}

fn tool_limits_card(state: &SettingsState) -> Element<'_, SettingsEvent> {
    // Two columns, each holding one related pair group stacked vertically:
    // left = bash / truncation / read, right = find+search / fetch / mcp.
    let (left_fields, right_fields) = ToolLimitField::ALL.split_at(6);
    let column_rows = |fields: &[ToolLimitField]| {
        column(fields.iter().copied().map(|f| limit_row(state, f)))
            .spacing(6)
            .width(Length::FillPortion(1))
    };

    container(
        column![
            section_title("Tool Limits"),
            row![column_rows(left_fields), column_rows(right_fields)].spacing(12),
        ]
        .spacing(8),
    )
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
                SettingsEvent::BuiltinTools(BuiltinToolsEvent::TaskModelSelectProvider(tier, e.id))
            })
            .width(Length::Fill),
            pick_list(models, selected_model, move |m: Model| {
                SettingsEvent::BuiltinTools(BuiltinToolsEvent::TaskModelSelectModel(tier, m.id))
            })
            .width(Length::Fill),
        ]
        .spacing(4)
        .align_y(Alignment::Center)
        .into()
    };

    let toggle: Element<'a, SettingsEvent> = if no_providers {
        // Nothing to pick without a provider — lock the toggle ON.
        toggler(true)
            .label("Inherit")
            .text_size(12)
            .style(crate::views::primary_toggler)
            .into()
    } else {
        toggler(inherit)
            .label("Inherit")
            .text_size(12)
            .on_toggle(move |v| {
                SettingsEvent::BuiltinTools(BuiltinToolsEvent::TaskModelInherit(tier, v))
            })
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
    text(title).size(12).font(BOLD).color(color_muted()).into()
}

// ── Update ─────────────────────────────────────────────────────────

/// Handle a Builtin Tools tab event, mutating the working copies.
pub(super) fn update(state: &mut SettingsState, event: BuiltinToolsEvent) {
    match event {
        BuiltinToolsEvent::EditMaxIterations(v) => state.working_max_iterations = v,
        BuiltinToolsEvent::EditFillRatioThreshold(v) => state.working_fill_ratio_threshold = v,
        BuiltinToolsEvent::EditStreamStallTimeout(v) => state.working_stream_stall_timeout = v,
        BuiltinToolsEvent::EditToolLimit(field, v) => {
            *state.working_tool_limits.get_mut(field) = v;
        }
        BuiltinToolsEvent::TaskModelSelectProvider(tier, id) => {
            if let Some(model) = state
                .working_models
                .providers
                .get(&id)
                .and_then(|p| p.models.first())
            {
                let mut cfg = state.working_task_models.get_config(tier).clone();
                cfg.provider_id = id;
                cfg.apply_model(model);
                state.working_task_models.set_config(tier, cfg);
            }
        }
        BuiltinToolsEvent::TaskModelSelectModel(tier, id) => {
            let cfg = state.working_task_models.get_config(tier).clone();
            if let Some(model) = state
                .working_models
                .providers
                .get(&cfg.provider_id)
                .and_then(|p| p.models.iter().find(|m| m.id == id))
            {
                let mut cfg = cfg;
                cfg.apply_model(model);
                state.working_task_models.set_config(tier, cfg);
            }
        }
        BuiltinToolsEvent::TaskModelInherit(tier, inherit) => {
            if inherit {
                state
                    .working_task_models
                    .set_config(tier, ModelConfig::default());
            } else if state.working_task_models.get_config(tier).is_empty() {
                // Pre-fill with the first provider/model so the pickers
                // have a concrete starting point.
                if let Some(provider_id) = state.working_models.providers.keys().next().cloned() {
                    state.update(SettingsEvent::BuiltinTools(
                        BuiltinToolsEvent::TaskModelSelectProvider(tier, provider_id),
                    ));
                }
            }
        }
        BuiltinToolsEvent::SaveBuiltinTools => {
            // Normalize fields to their parsed values, falling back to the defaults.
            let max_iters = state.parsed_max_iterations();
            state.working_max_iterations = max_iters.to_string();
            let fill_ratio = state.parsed_fill_ratio_threshold();
            state.working_fill_ratio_threshold = fill_ratio.to_string();
            let stall = state.parsed_stream_stall_timeout();
            state.working_stream_stall_timeout = stall.to_string();
            let limits = state.parsed_tool_limits();
            state.working_tool_limits = ToolLimitStrings::from_limits(&limits);
            state.save_feedback = Some(SettingsTab::BuiltinTools);
        }
    }
}
