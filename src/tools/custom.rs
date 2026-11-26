use std::collections::BTreeSet;
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};
use tokio_util::sync::CancellationToken;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use shell_words::split;
use tinytemplate::error::Error as TinyTemplateError;
use tinytemplate::{TinyTemplate, format_unescaped};

use super::Tool;

/// Reserved variable holding environment variables for HTTP templates.
const ENV_KEY: &str = "env";
/// Textual prefix of an environment reference; keep in sync with `ENV_KEY`.
const ENV_REF: &str = "env.";
/// Stand-in value used when validating templates without reading the environment.
const EXAMPLE_ENV_VALUE: &str = "https://example.com";
/// Maximum body/detail characters included in an error message.
const ERROR_PREVIEW_CHARS: usize = 1000;

// ── Parameter types ─────────────────────────────────────────────────

/// Kind of a tool parameter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ParameterType {
    Null,
    String,
    Integer,
    Number,
    Boolean,
    Array(Box<ParameterType>),
    Object(Vec<ToolParameter>),
    Union(Vec<ParameterType>),
}

impl ParameterType {
    /// Convert this type into its JSON Schema representation.
    fn to_schema_value(&self) -> Value {
        match self {
            ParameterType::Null => json!({ "type": "null" }),
            ParameterType::String => json!({ "type": "string" }),
            ParameterType::Integer => json!({ "type": "integer" }),
            ParameterType::Number => json!({ "type": "number" }),
            ParameterType::Boolean => json!({ "type": "boolean" }),
            ParameterType::Array(inner) => {
                json!({
                    "type": "array",
                    "items": inner.to_schema_value()
                })
            }
            ParameterType::Object(params) => build_schema(params),
            ParameterType::Union(variants) => {
                let schemas: Vec<Value> = variants.iter().map(|v| v.to_schema_value()).collect();
                json!({ "anyOf": schemas })
            }
        }
    }
}

/// Description of a single tool parameter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolParameter {
    pub name: String,
    pub kind: ParameterType,
    pub description: String,
    pub required: bool,
}

/// Build a JSON Schema `{"type":"object", "properties":..., "required":...}`
/// from a list of parameter definitions.
fn build_schema(params: &[ToolParameter]) -> Value {
    let mut properties = serde_json::Map::new();
    let mut required: Vec<&str> = Vec::new();

    for p in params {
        let mut prop_schema = p.kind.to_schema_value();
        if let Some(obj) = prop_schema.as_object_mut() {
            obj.insert(
                "description".to_string(),
                Value::String(p.description.clone()),
            );
        }
        properties.insert(p.name.clone(), prop_schema);
        if p.required {
            required.push(&p.name);
        }
    }

    let mut schema = json!({
        "type": "object",
        "properties": properties,
    });
    if !required.is_empty() {
        schema["required"] = Value::Array(
            required
                .iter()
                .map(|&r| Value::String(r.to_string()))
                .collect(),
        );
    }
    schema
}

// ── HTTP request types ──────────────────────────────────────────────

/// How a custom tool is invoked.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ToolKind {
    /// CLI template; split before substitution to block argv injection.
    Command(String),
    /// HTTP API request.
    Http(HttpRequest),
}

impl Default for ToolKind {
    fn default() -> Self {
        Self::Command(String::new())
    }
}

impl ToolKind {
    /// Type discriminant for the settings UI.
    pub fn mode(&self) -> ToolMode {
        match self {
            Self::Command(_) => ToolMode::Command,
            Self::Http(_) => ToolMode::Http,
        }
    }
}

/// Tool type selected in the settings UI; carries no payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolMode {
    Command,
    Http,
}

/// HTTP method used by an HTTP custom tool.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum HttpMethod {
    Get,
    #[default]
    Post,
    Put,
    Patch,
    Delete,
}

impl HttpMethod {
    pub const ALL: [Self; 5] = [Self::Get, Self::Post, Self::Put, Self::Patch, Self::Delete];

    fn to_reqwest(self) -> reqwest::Method {
        match self {
            Self::Get => reqwest::Method::GET,
            Self::Post => reqwest::Method::POST,
            Self::Put => reqwest::Method::PUT,
            Self::Patch => reqwest::Method::PATCH,
            Self::Delete => reqwest::Method::DELETE,
        }
    }
}

impl fmt::Display for HttpMethod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.to_reqwest().as_str())
    }
}

/// One request header. Values support `{param}` and `{env.VAR}` templates.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HttpHeader {
    pub name: String,
    pub value: String,
}

/// HTTP request definition for a custom tool.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HttpRequest {
    pub method: HttpMethod,
    /// Request URL; `{param}` values are percent-encoded.
    pub url: String,
    pub headers: Vec<HttpHeader>,
    /// Leading `{`/`[` selects JSON, preserving parameter value types.
    pub body: String,
    /// Optional TinyTemplate applied to the JSON response.
    pub response_template: String,
}

// ── CustomTool ──────────────────────────────────────────────────────

/// A user-defined command-line or HTTP API tool, persisted to `~/.crabot/tools.ron`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CustomTool {
    pub name: String,
    pub description: String,
    /// Tool parameters definition.
    pub parameters: Vec<ToolParameter>,
    /// Tool type and its configuration.
    pub kind: ToolKind,
}

impl Tool for CustomTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn schema(&self) -> Value {
        build_schema(&self.parameters)
    }

    fn execute_inner(
        &self,
        args: &Value,
        workspace: &Path,
        cancel: &CancellationToken,
    ) -> Result<String, String> {
        match &self.kind {
            ToolKind::Command(command) => self.execute_command(command, args, workspace, cancel),
            ToolKind::Http(request) => self.execute_http(request, args, cancel),
        }
    }
}

// ── Command execution ──────────────────────────────────────────────

impl CustomTool {
    fn execute_command(
        &self,
        command: &str,
        args: &Value,
        workspace: &Path,
        cancel: &CancellationToken,
    ) -> Result<String, String> {
        let parts = self.build_argv(command, args)?;
        let (exe, args) = parts
            .split_first()
            .ok_or_else(|| "Empty command template".to_string())?;
        let encoding = super::argv_encoding(exe, args);
        let fallback = super::console_fallback(encoding);

        // Resolve through `PATH`: `npx`-style shims (`.cmd`) need a shell lookup.
        let paths = super::host_path_lists(None);

        // Create unnamed pipe pairs for stdout and stderr.
        let (stdout_tx, stdout_rx) = super::create_pipe_pair("stdout")?;
        let (stderr_tx, stderr_rx) = super::create_pipe_pair("stderr")?;

        let mut cmd = Command::new(super::resolve_command(exe, &paths, workspace));
        cmd.args(args)
            .current_dir(workspace)
            .stdin(Stdio::null())
            .stdout(super::pipe_to_stdio(stdout_tx))
            .stderr(super::pipe_to_stdio(stderr_tx));
        // Drop secrets and rustup's recursion counter, like other child paths.
        super::sanitize_child_env(&mut cmd);
        // Prevent a visible console window from flashing on Windows.
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
        }
        let child = cmd
            .spawn()
            .map_err(|e| format!("Failed to execute custom tool '{}': {e}", self.name))?;

        let timeout = Duration::from_millis(super::tool_limits().command_timeout_ms);
        let output = super::wait_with_timeout(
            child,
            Some(stdout_rx),
            Some(stderr_rx),
            timeout,
            timeout,
            false, // custom tools don't run in their own process group
            encoding,
            fallback,
            cancel,
            None, // custom tools don't stream output
        )
        .map_err(|e| format!("Custom tool '{}': {}", self.name, e.into_message()))?;

        // Native executable — no MSYS signal-decode semantics.
        Ok(super::format_command_output(
            &output, false, encoding, fallback,
        ))
    }
}

// ── HTTP execution ─────────────────────────────────────────────────

impl CustomTool {
    fn execute_http(
        &self,
        request: &HttpRequest,
        args: &Value,
        cancel: &CancellationToken,
    ) -> Result<String, String> {
        check_http_templates(request)?;

        let values = self.template_values(args, true)?;
        let (url, body, headers) = request.prepare(&values)?;
        let method = request.method;
        let response_template = &request.response_template;
        let limits = super::tool_limits();
        let timeout = Duration::from_millis(limits.fetch_timeout_ms);
        let max_body_bytes = limits.fetch_max_body_bytes;

        tokio::runtime::Handle::current().block_on(async {
            let mut builder = super::http::client()?
                .request(method.to_reqwest(), url)
                .headers(headers)
                .timeout(timeout);
            if let Some(body) = body {
                builder = builder.body(body);
            }

            let start = Instant::now();
            // Race the HTTP request against user cancellation.
            let resp = tokio::select! {
                biased;
                _ = cancel.cancelled() => return Err(super::CANCEL_REASON.into()),
                r = builder.send() => {
                    r.map_err(|e| format!("HTTP request failed: {}", e.without_url()))?
                }
            };
            let status = resp.status();
            let body_bytes = super::http::read_body(resp, max_body_bytes, cancel).await?;
            let body_text = super::decode_bytes(&body_bytes);
            // Do not log the URL: it may carry an API key from a parameter or env var.
            tracing::debug!(
                tool = self.name,
                method = %method,
                status = %status,
                elapsed_ms = start.elapsed().as_millis() as u64,
                "custom HTTP tool finished"
            );

            if !status.is_success() {
                return Err(http_error_message(status, &body_text));
            }

            let output = if response_template.trim().is_empty() {
                body_text
            } else {
                render_response(response_template, status, &body_text)?
            };
            let output = super::truncate_output(output);
            Ok(if output.trim().is_empty() {
                "(empty response)".to_string()
            } else {
                output
            })
        })
    }

    /// Build the template context, resolving only referenced environment variables.
    fn template_values(&self, args: &Value, with_env: bool) -> Result<TemplateValues, String> {
        let env_names = if with_env {
            self.referenced_env_names()
        } else {
            Vec::new()
        };
        self.template_values_with(args, with_env, &env_names, |name| {
            std::env::var_os(name).and_then(|value| value.into_string().ok())
        })
    }

    /// Shared builder; `lookup` lets validation render without real secrets.
    fn template_values_with(
        &self,
        args: &Value,
        with_env: bool,
        env_names: &[String],
        mut lookup: impl FnMut(&str) -> Option<String>,
    ) -> Result<TemplateValues, String> {
        let mut context = serde_json::Map::new();
        let mut substitutions = Vec::new();

        for param in &self.parameters {
            if with_env && param.name == ENV_KEY {
                return Err(format!(
                    "Parameter name '{ENV_KEY}' is reserved for environment variables in HTTP tools"
                ));
            }
            match args.get(&param.name) {
                Some(value) if !value.is_null() => {
                    let rendered = text_form(value);
                    reject_marker(&format!("Value of parameter '{}'", param.name), &rendered)?;
                    let ph = placeholder(substitutions.len());
                    context.insert(param.name.clone(), Value::String(ph.clone()));
                    substitutions.push(Substitution {
                        placeholder: ph,
                        value: value.clone(),
                        source: ValueSource::Parameter,
                    });
                }
                _ => {
                    context.insert(param.name.clone(), Value::Null);
                }
            }
        }

        if with_env {
            let mut env = serde_json::Map::new();
            for name in env_names {
                let Some(value) = lookup(name) else {
                    continue;
                };
                // A marker-bearing value could forge another substitution.
                if value.contains(PLACEHOLDER_PREFIX) {
                    continue;
                }
                let ph = placeholder(substitutions.len());
                env.insert(name.clone(), Value::String(ph.clone()));
                substitutions.push(Substitution {
                    placeholder: ph,
                    value: Value::String(value),
                    source: ValueSource::Environment,
                });
            }
            context.insert(ENV_KEY.to_string(), Value::Object(env));
        }

        Ok(TemplateValues {
            context: Value::Object(context),
            substitutions,
        })
    }

    /// Build argv with placeholders split before substitution (see [`PLACEHOLDER_PREFIX`]).
    fn build_argv(&self, command: &str, args: &Value) -> Result<Vec<String>, String> {
        reject_marker("Command template", command)?;

        let values = self.template_values(args, false)?;
        let mut tt = TinyTemplate::new();
        tt.add_template("cmd", command)
            .map_err(|e| format!("Template error: {e}"))?;
        let rendered = tt
            .render("cmd", &values.context)
            .map_err(|e| format!("Template render error: {e}"))?;

        // Split first (shell quoting), substitute second — see PLACEHOLDER_PREFIX.
        Ok(split(&rendered)
            .map_err(|e| format!("Failed to parse command: {e}"))?
            .into_iter()
            .map(|token| values.substitute_text(&token))
            .collect())
    }
}

// ── Validation ─────────────────────────────────────────────────────

impl CustomTool {
    /// Validate with example values, without I/O or real environment variables.
    pub fn validate(&self) -> Result<(), String> {
        self.validate_kind()
            .map_err(|e| format!("Tool '{}': {e}", self.name))
    }

    fn validate_kind(&self) -> Result<(), String> {
        if self.name.trim().is_empty() {
            return Err("name is empty".to_string());
        }
        let mut names = BTreeSet::new();
        for param in &self.parameters {
            if param.name.trim().is_empty() {
                return Err("a parameter has an empty name".to_string());
            }
            if !names.insert(param.name.as_str()) {
                return Err(format!("duplicate parameter '{}'", param.name));
            }
        }

        let args = example_args(&self.parameters);
        match &self.kind {
            ToolKind::Command(command) => {
                if command.trim().is_empty() {
                    return Err("command template is empty".to_string());
                }
                self.build_argv(command, &args)?;
            }
            ToolKind::Http(request) => {
                check_http_templates(request)?;
                let env_names = self.referenced_env_names();
                let values = self.template_values_with(&args, true, &env_names, |_| {
                    Some(EXAMPLE_ENV_VALUE.to_string())
                })?;
                let _ = request.prepare(&values)?;
                if !request.response_template.trim().is_empty() {
                    check_template_syntax(&request.response_template, "response")?;
                }
            }
        }
        Ok(())
    }

    /// Environment variables referenced by this tool's request templates.
    fn referenced_env_names(&self) -> Vec<String> {
        let ToolKind::Http(request) = &self.kind else {
            return Vec::new();
        };
        let mut names = BTreeSet::new();
        collect_env_names(&request.url, &mut names);
        collect_env_names(&request.body, &mut names);
        for header in &request.headers {
            collect_env_names(&header.value, &mut names);
        }
        names.into_iter().collect()
    }
}

/// Collect `{env.NAME}` references, including ones inside `{{ blocks }}`.
fn collect_env_names(template: &str, names: &mut BTreeSet<String>) {
    let mut offset = 0;
    while let Some(pos) = template[offset..].find(ENV_REF) {
        let start = offset + pos;
        let preceded_by_ident = template[..start]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_');
        let name_start = start + ENV_REF.len();
        let name: String = template[name_start..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '(' | ')' | '-'))
            .collect();
        if !preceded_by_ident && !name.is_empty() {
            names.insert(name.clone());
        }
        offset = name_start + name.len();
    }
}

/// Example parameter values for validation, chosen to render valid URLs/JSON.
fn example_args(parameters: &[ToolParameter]) -> Value {
    let mut args = serde_json::Map::new();
    for param in parameters {
        let value = match &param.kind {
            // Null params need a stand-in to render a complete validation request.
            ParameterType::Null | ParameterType::String | ParameterType::Union(_) => {
                json!("example")
            }
            ParameterType::Integer => json!(1),
            ParameterType::Number => json!(1.0),
            ParameterType::Boolean => json!(true),
            ParameterType::Array(_) => json!([]),
            ParameterType::Object(_) => json!({}),
        };
        args.insert(param.name.clone(), value);
    }
    Value::Object(args)
}

// ── HTTP request construction ──────────────────────────────────────

impl HttpRequest {
    fn prepare(
        &self,
        values: &TemplateValues,
    ) -> Result<(reqwest::Url, Option<String>, HeaderMap), String> {
        let url = self.build_url(values)?;
        let (body, json_body) = self.build_body(values)?;
        let headers = self.build_headers(values, json_body)?;
        Ok((url, body, headers))
    }

    fn build_url(&self, values: &TemplateValues) -> Result<reqwest::Url, String> {
        if self.url.trim().is_empty() {
            return Err("HTTP tool URL is empty".to_string());
        }
        let rendered = render_http_template(&self.url, values, "URL")?;
        let encoded = values.substitute_url(&rendered);
        // Omit the rendered URL: it may carry an API key from a parameter or env var.
        let url = reqwest::Url::parse(&encoded)
            .map_err(|e| format!("Invalid URL after template rendering: {e}"))?;
        super::http::ensure_http_url(&url)?;
        Ok(url)
    }

    fn build_headers(&self, values: &TemplateValues, json_body: bool) -> Result<HeaderMap, String> {
        let mut headers = HeaderMap::new();
        for header in &self.headers {
            let name = header.name.trim();
            if name.is_empty() {
                if header.value.trim().is_empty() {
                    continue;
                }
                return Err("HTTP header has a value but no name".to_string());
            }
            let name = HeaderName::from_bytes(name.as_bytes())
                .map_err(|e| format!("Invalid header name '{}': {e}", header.name))?;
            let rendered = render_http_template(&header.value, values, "header value")?;
            let value = values.substitute_text(&rendered);
            let value = HeaderValue::from_bytes(value.as_bytes())
                .map_err(|e| format!("Invalid value for header '{}': {e}", header.name))?;
            headers.append(name, value);
        }
        if json_body && !headers.contains_key(CONTENT_TYPE) {
            headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        }
        Ok(headers)
    }

    fn build_body(&self, values: &TemplateValues) -> Result<(Option<String>, bool), String> {
        if self.body.is_empty() {
            return Ok((None, false));
        }
        let rendered = render_http_template(&self.body, values, "request body")?;
        let trimmed = rendered.trim_start();
        if trimmed.starts_with('{') || trimmed.starts_with('[') {
            // Substitute bare placeholders before parsing; quoted ones remain for later.
            let substituted = values.substitute_bare_json(&rendered)?;
            let mut value: Value = serde_json::from_str(&substituted).map_err(|e| {
                format!(
                    "Request body is not valid JSON after template rendering: {e}\nBody:\n{}",
                    crate::truncate_chars(&rendered, ERROR_PREVIEW_CHARS)
                )
            })?;
            values.substitute_json(&mut value);
            let body = serde_json::to_string(&value)
                .map_err(|e| format!("Failed to serialize JSON request body: {e}"))?;
            Ok((Some(body), true))
        } else {
            Ok((Some(values.substitute_text(&rendered)), false))
        }
    }
}

// ── Template values and substitution ──────────────────────────────

/// Where a value came from; URL parameters are percent-encoded, env vars stay raw.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ValueSource {
    Parameter,
    Environment,
}

#[derive(Debug, Clone)]
struct Substitution {
    placeholder: String,
    value: Value,
    source: ValueSource,
}

#[derive(Debug, Clone)]
struct TemplateValues {
    context: Value,
    substitutions: Vec<Substitution>,
}

impl TemplateValues {
    /// Replace placeholders in a URL; parameter values are percent-encoded.
    fn substitute_url(&self, rendered: &str) -> String {
        self.substitute(rendered, true)
    }

    /// Replace placeholders in headers or non-JSON bodies with raw values.
    fn substitute_text(&self, rendered: &str) -> String {
        self.substitute(rendered, false)
    }

    /// Replace bare placeholders with typed JSON literals before parsing.
    /// Quoted placeholders remain for [`Self::substitute_json`].
    fn substitute_bare_json(&self, rendered: &str) -> Result<String, String> {
        let mut out = String::with_capacity(rendered.len() + 32);
        let mut rest = rendered;
        let mut in_string = false;
        let mut escaped = false;
        while let Some(ch) = rest.chars().next() {
            if !in_string
                && rest.starts_with(PLACEHOLDER_PREFIX)
                && let Some(sub) = self
                    .substitutions
                    .iter()
                    .find(|sub| rest.starts_with(&sub.placeholder))
            {
                let literal = serde_json::to_string(&sub.value)
                    .map_err(|e| format!("Failed to serialize JSON request body: {e}"))?;
                out.push_str(&literal);
                rest = &rest[sub.placeholder.len()..];
                continue;
            }
            match ch {
                '"' => {
                    if in_string && escaped {
                        escaped = false;
                    } else {
                        in_string = !in_string;
                    }
                }
                '\\' if in_string => escaped = !escaped,
                _ => escaped = false,
            }
            out.push(ch);
            rest = &rest[ch.len_utf8()..];
        }
        Ok(out)
    }

    fn substitute(&self, rendered: &str, encode_params: bool) -> String {
        self.substitutions
            .iter()
            .fold(rendered.to_string(), |text, sub| {
                let mut value = text_form(&sub.value);
                if encode_params && sub.source == ValueSource::Parameter {
                    value = url_encode(&value);
                }
                text.replace(&sub.placeholder, &value)
            })
    }

    /// Replace placeholders inside a parsed JSON body, preserving parameter types.
    fn substitute_json(&self, value: &mut Value) {
        match value {
            Value::String(s) => {
                if let Some(sub) = self.substitutions.iter().find(|sub| s == &sub.placeholder) {
                    *value = sub.value.clone();
                    return;
                }
                let replaced = self.substitute_text(s);
                if replaced != *s {
                    *value = Value::String(replaced);
                }
            }
            Value::Array(items) => {
                for item in items {
                    self.substitute_json(item);
                }
            }
            Value::Object(map) => {
                for item in map.values_mut() {
                    self.substitute_json(item);
                }
            }
            _ => {}
        }
    }
}

/// Render one HTTP template (`url`, header value, or body).
fn render_http_template(
    template: &str,
    values: &TemplateValues,
    what: &str,
) -> Result<String, String> {
    render_template(&escape_literal_braces(template), &values.context, what)
}

/// TinyTemplate engine for HTTP request and response templates.
fn template_engine() -> TinyTemplate<'static> {
    let mut tt = TinyTemplate::new();
    tt.set_default_formatter(&format_unescaped);
    tt.add_formatter("json", format_json);
    tt
}

fn parse_template<'a>(template: &'a str, what: &str) -> Result<TinyTemplate<'a>, String> {
    let mut tt = template_engine();
    tt.add_template("template", template)
        .map_err(|e| format!("Invalid {what} template: {e}"))?;
    Ok(tt)
}

/// Render one TinyTemplate with unescaped output.
fn render_template(template: &str, context: &Value, what: &str) -> Result<String, String> {
    parse_template(template, what)?
        .render("template", context)
        .map_err(|e| format!("Failed to render {what} template: {e}"))
}

/// Parse only, using the same brace escaping as rendering.
fn check_template_syntax(template: &str, what: &str) -> Result<(), String> {
    parse_template(&escape_literal_braces(template), what).map(|_| ())
}

/// Format a value as compact JSON, escaping quotes and control characters.
fn format_json(value: &Value, output: &mut String) -> Result<(), TinyTemplateError> {
    output.push_str(&serde_json::to_string(value)?);
    Ok(())
}

/// Escape literal `{` while preserving `{{ blocks }}` and `{values}`.
fn escape_literal_braces(template: &str) -> String {
    let mut out = String::with_capacity(template.len() + 8);
    let mut rest = template;
    while let Some(pos) = rest.find('{') {
        let (before, from) = rest.split_at(pos);
        out.push_str(before);
        if let Some(after) = from.strip_prefix("{{") {
            out.push_str("{{");
            rest = after;
        } else if out.ends_with('\\') {
            out.push('{');
            rest = &from[1..];
        } else if let Some(end) = from.find('}')
            && is_value_expr(&from[1..end])
        {
            out.push_str(&from[..=end]);
            rest = &from[end + 1..];
        } else {
            out.push_str("\\{");
            rest = &from[1..];
        }
    }
    out.push_str(rest);
    out
}

/// Whether `inner` looks like TinyTemplate value syntax (`path` or `path | formatter`).
fn is_value_expr(inner: &str) -> bool {
    let inner = inner.trim();
    let (path, formatter) = match inner.split_once('|') {
        Some((path, formatter)) => (path.trim(), Some(formatter.trim())),
        None => (inner, None),
    };
    if let Some(formatter) = formatter
        && !is_identifier(formatter)
    {
        return false;
    }
    path.split('.').all(is_path_segment)
}

fn is_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Environment names may contain punctuation such as `ProgramFiles(x86)`.
fn is_path_segment(s: &str) -> bool {
    if matches!(s, "@root" | "@index" | "@first" | "@last") {
        return true;
    }
    if s.parse::<usize>().is_ok() {
        return true;
    }
    let mut chars = s.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '(' | ')' | '-'))
}

/// Text form of a parameter value: strings stay raw, other JSON renders compactly.
fn text_form(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn url_encode(value: &str) -> String {
    // Use `%20`, not `+`, for servers that treat `+` literally.
    url::form_urlencoded::byte_serialize(value.as_bytes())
        .collect::<String>()
        .replace('+', "%20")
}

fn reject_marker(what: &str, text: &str) -> Result<(), String> {
    if text.contains(PLACEHOLDER_PREFIX) {
        Err(format!(
            "{what} must not contain the reserved marker '{PLACEHOLDER_PREFIX}'"
        ))
    } else {
        Ok(())
    }
}

fn check_http_templates(request: &HttpRequest) -> Result<(), String> {
    reject_marker("URL template", &request.url)?;
    reject_marker("Request body", &request.body)?;
    request
        .headers
        .iter()
        .try_for_each(|header| reject_marker("Header value", &header.value))
}

// ── Responses ──────────────────────────────────────────────────────

fn http_error_message(status: reqwest::StatusCode, body: &str) -> String {
    let detail = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| {
            value
                .pointer("/error/message")
                .or_else(|| value.get("message"))
                .and_then(Value::as_str)
                .filter(|detail| !detail.trim().is_empty())
                .map(|detail| crate::truncate_chars(detail, ERROR_PREVIEW_CHARS))
        })
        .unwrap_or_else(|| crate::truncate_chars(body, ERROR_PREVIEW_CHARS));
    if detail.trim().is_empty() {
        format!("HTTP {status}")
    } else {
        format!("HTTP {status}: {detail}")
    }
}

/// Render an optional response template against the parsed JSON response.
fn render_response(
    template: &str,
    status: reqwest::StatusCode,
    body: &str,
) -> Result<String, String> {
    let parsed: Value = serde_json::from_str(body).map_err(|e| {
        format!(
            "Response is not valid JSON, cannot apply response_template: {e}\nBody:\n{}",
            crate::truncate_chars(body, ERROR_PREVIEW_CHARS)
        )
    })?;
    let context = json!({
        "response": parsed,
        "body": body,
        "status": status.as_u16(),
    });
    // A response template may itself contain literal JSON braces.
    render_template(&escape_literal_braces(template), &context, "response")
}

// ── Command construction ──────────────────────────────────────────

/// Reserved placeholder prefix; splitting before substitution blocks argv injection.
const PLACEHOLDER_PREFIX: &str = "@@CRABOT_ARG_";

fn placeholder(idx: usize) -> String {
    format!("{PLACEHOLDER_PREFIX}{idx}@@")
}

// ── ToolList ────────────────────────────────────────────────────────

/// Persistable list of user-defined custom tools.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolList {
    pub custom_tools: Vec<CustomTool>,
}

impl ToolList {
    /// Path to `~/.crabot/tools.ron`.
    pub fn path() -> PathBuf {
        crate::setup::config_dir().join("tools.ron")
    }

    /// Load from disk; an unparsable file is moved aside and recovered from its
    /// `.bak` when possible.
    pub fn load() -> Self {
        crate::atomic::load_ron(&Self::path()).unwrap_or_default()
    }

    /// Save custom tools to disk as RON text.
    pub fn save(&self) {
        let config = ron::ser::PrettyConfig::default().new_line("\n");
        let path = Self::path();
        if let Err(e) = crate::atomic::save_ron(&path, self, config) {
            tracing::error!(path = %path.display(), "failed to save tools.ron: {e}");
        }
    }

    /// Return the names of every custom tool.
    pub fn names(&self) -> Vec<String> {
        self.custom_tools.iter().map(|ct| ct.name.clone()).collect()
    }
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn bundled_tools() -> ToolList {
        ron::from_str(include_str!("../../assets/tools.ron")).unwrap()
    }

    fn parameter(name: &str, kind: ParameterType, required: bool) -> ToolParameter {
        ToolParameter {
            name: name.into(),
            kind,
            description: String::new(),
            required,
        }
    }

    #[test]
    fn current_ron_round_trips() {
        let list = ToolList {
            custom_tools: vec![
                CustomTool {
                    name: "cmd".into(),
                    kind: ToolKind::Command("echo {text}".into()),
                    ..CustomTool::default()
                },
                CustomTool {
                    name: "http".into(),
                    kind: ToolKind::Http(HttpRequest {
                        method: HttpMethod::Get,
                        url: "https://example.com".into(),
                        ..HttpRequest::default()
                    }),
                    ..CustomTool::default()
                },
            ],
        };
        let text = ron::to_string(&list).unwrap();
        let back: ToolList = ron::from_str(&text).unwrap();
        assert_eq!(back.custom_tools[0].kind, list.custom_tools[0].kind);
        assert_eq!(back.custom_tools[1].kind, list.custom_tools[1].kind);
    }

    #[test]
    fn bundled_tools_parse_and_validate() {
        let list = bundled_tools();
        let names: Vec<_> = list
            .custom_tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect();
        assert_eq!(names, ["web_search", "calculator"]);
        for tool in &list.custom_tools {
            tool.validate().unwrap();
        }
    }

    #[test]
    fn bundled_search_tool_builds_typed_body_and_json_output() {
        let list = bundled_tools();
        let search_tool = list
            .custom_tools
            .iter()
            .find(|tool| tool.name == "web_search")
            .unwrap();
        let ToolKind::Http(search) = &search_tool.kind else {
            panic!("web_search must be an HTTP tool");
        };

        let values = search_tool
            .template_values(&json!({"query": "DeepSeek V4", "count": 5}), true)
            .unwrap();
        let (body, is_json) = search.build_body(&values).unwrap();
        assert!(is_json);
        assert_eq!(
            serde_json::from_str::<Value>(body.as_deref().unwrap()).unwrap(),
            json!({"query": "DeepSeek V4", "count": 5})
        );

        let results = r#"{"results":[
            {"title":"say \"hi\"","url":"https://e","summary":"S","content":"C","rerank_score":1.5,"authority":3},
            {"title":"U","url":"https://f","summary":"","content":"","rerank_score":0.5,"authority":-1}
        ]}"#;
        let output =
            render_response(&search.response_template, reqwest::StatusCode::OK, results).unwrap();
        let parsed: Value = serde_json::from_str(&output).expect("search output must be JSON");
        assert_eq!(parsed[0]["title"], json!("say \"hi\""));
        assert_eq!(parsed[0]["rerank_score"], json!(1.5));
        assert!(parsed[1].get("summary").is_none());
        assert!(parsed[1].get("content").is_none());
    }

    #[test]
    fn json_body_accepts_bare_and_quoted_placeholders() {
        let tool = CustomTool {
            name: "search".into(),
            parameters: vec![
                parameter("query", ParameterType::String, true),
                parameter("count", ParameterType::Integer, false),
                parameter("flag", ParameterType::Boolean, false),
            ],
            kind: ToolKind::Http(HttpRequest {
                method: HttpMethod::Post,
                url: "https://example.com/search".into(),
                body: r#"{"query": {query}, "count": {count}, "flag": {flag}, "quoted": "{count}", "label": "hi {query}"}"#.into(),
                ..HttpRequest::default()
            }),
            ..CustomTool::default()
        };
        tool.validate().unwrap();

        let args = json!({"query": "say \"hi\"\nnext", "count": 5, "flag": false});
        let values = tool.template_values(&args, true).unwrap();
        let ToolKind::Http(request) = &tool.kind else {
            panic!("expected an HTTP tool");
        };
        let (body, is_json) = request.build_body(&values).unwrap();
        assert!(is_json);
        assert_eq!(
            serde_json::from_str::<Value>(body.as_deref().unwrap()).unwrap(),
            json!({
                "query": "say \"hi\"\nnext",
                "count": 5,
                "flag": false,
                "quoted": 5,
                "label": "hi say \"hi\"\nnext",
            })
        );
    }

    #[test]
    fn json_error_detail_is_truncated() {
        let body = json!({"error": {"message": "x".repeat(ERROR_PREVIEW_CHARS + 200)}}).to_string();
        let error = http_error_message(reqwest::StatusCode::BAD_REQUEST, &body);
        let detail = error.strip_prefix("HTTP 400 Bad Request: ").unwrap();
        assert_eq!(detail.chars().count(), ERROR_PREVIEW_CHARS + 1);
        assert!(detail.ends_with('…'));
    }

    #[test]
    fn command_values_stay_one_argument() {
        let list = bundled_tools();
        let calculator = list
            .custom_tools
            .iter()
            .find(|tool| tool.name == "calculator")
            .unwrap();
        let ToolKind::Command(command) = &calculator.kind else {
            panic!("calculator must be a command tool");
        };

        let argv = calculator
            .build_argv(command, &json!({"expr": "sqrt(2) + 1"}))
            .unwrap();
        assert_eq!(argv, ["kalker", "sqrt(2) + 1"]);
    }

    fn templated_http_tool(url: &str, header_value: &str, body: &str) -> CustomTool {
        CustomTool {
            name: "search".into(),
            parameters: vec![parameter("query", ParameterType::String, true)],
            kind: ToolKind::Http(HttpRequest {
                url: url.into(),
                headers: vec![HttpHeader {
                    name: "Authorization".into(),
                    value: header_value.into(),
                }],
                body: body.into(),
                ..HttpRequest::default()
            }),
            ..CustomTool::default()
        }
    }

    #[test]
    fn only_referenced_env_variables_are_resolved() {
        let tool = templated_http_tool(
            "https://example.com?q={query}&key={env.API_KEY}",
            "Bearer {env.API_TOKEN}",
            r#"{"token": "{env.API_TOKEN}"}"#,
        );
        let env_names = tool.referenced_env_names();
        assert_eq!(env_names, vec!["API_KEY", "API_TOKEN"]);

        let mut looked_up = Vec::new();
        tool.template_values_with(&json!({"query": "a b"}), true, &env_names, |name| {
            looked_up.push(name.to_string());
            Some(format!("value-{name}"))
        })
        .unwrap();
        assert_eq!(looked_up, vec!["API_KEY", "API_TOKEN"]);
    }

    #[test]
    fn validation_uses_examples_without_reading_the_environment() {
        const KEY: &str = "CRABOT_UNSET_KEY_FOR_VALIDATION";
        assert!(std::env::var_os(KEY).is_none());
        let tool = templated_http_tool(
            "https://example.com?q={query}",
            &format!("Bearer {{env.{KEY}}}"),
            r#"{"query": "{query}"}"#,
        );
        tool.validate().unwrap();

        let tool = templated_http_tool("", "Bearer x", "");
        assert!(tool.validate().unwrap_err().contains("URL is empty"));

        let tool = templated_http_tool("https://example.com", "Bearer x", r#"{"broken": }"#);
        assert!(tool.validate().unwrap_err().contains("not valid JSON"));

        let command = CustomTool {
            name: "cmd".into(),
            kind: ToolKind::Command("   ".into()),
            ..CustomTool::default()
        };
        assert!(command.validate().unwrap_err().contains("empty"));
    }
}
