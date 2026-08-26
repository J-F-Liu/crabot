use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use tokio_util::sync::CancellationToken;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use shell_words::split;
use tinytemplate::TinyTemplate;

use super::Tool;

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

// ── CustomTool ──────────────────────────────────────────────────────

/// A user-defined command-line tool.
///
/// Serialized directly to `~/.crabot/tools.ron`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomTool {
    pub name: String,
    pub description: String,
    pub instruction: String,
    /// Tool parameters definition.
    pub parameters: Vec<ToolParameter>,
    /// Command template using [TinyTemplate syntax](https://docs.rs/tinytemplate/1.2.1/tinytemplate/syntax/index.html).
    /// The first whitespace-separated token is the executable; the remainder are arguments.
    /// `{param}` inserts an argument value, and `{{ if param }}...{{ endif }}` enables conditional logic.
    ///
    /// Values are substituted after shell-style splitting (see [`PLACEHOLDER_PREFIX`]),
    /// so they can never inject extra argv elements.
    pub command: String,
}

impl Tool for CustomTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn instruction(&self) -> &str {
        &self.instruction
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
        let parts = self.build_argv(args)?;
        let (exe, args) = parts
            .split_first()
            .ok_or_else(|| "Empty command template".to_string())?;

        // Create unnamed pipe pairs for stdout and stderr.
        let (stdout_tx, stdout_rx) = super::create_pipe_pair("stdout")?;
        let (stderr_tx, stderr_rx) = super::create_pipe_pair("stderr")?;

        let mut cmd = Command::new(exe);
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

        let timeout = std::time::Duration::from_millis(super::tool_limits().command_timeout_ms);
        let output = super::wait_with_timeout(
            child,
            Some(stdout_rx),
            Some(stderr_rx),
            timeout,
            timeout,
            false, // custom tools don't run in their own process group
            cancel,
            None, // custom tools don't stream output
        )
        .map_err(|e| format!("Custom tool '{}': {}", self.name, e.into_message()))?;

        // Native executable — no MSYS signal-decode semantics.
        Ok(super::format_command_output(&output, false))
    }
}

// ── Command construction ──────────────────────────────────────────

/// Reserved prefix of internal placeholders (`@@CRABOT_ARG_<n>@@`). Values are
/// rendered as unique placeholders, split with shell-quoting rules, then
/// substituted back — so values can never inject extra argv elements. The
/// prefix must not appear in templates or values, or a crafted input could
/// forge another parameter's placeholder.
const PLACEHOLDER_PREFIX: &str = "@@CRABOT_ARG_";

fn placeholder(idx: usize) -> String {
    format!("{PLACEHOLDER_PREFIX}{idx}@@")
}

impl CustomTool {
    /// Build the argv for this tool call. See [`PLACEHOLDER_PREFIX`] for the
    /// placeholder workflow. Unknown args are ignored; `{{ if }}` conditionals
    /// still work since present params hold truthy placeholders.
    fn build_argv(&self, args: &Value) -> Result<Vec<String>, String> {
        let mut tt = TinyTemplate::new();
        tt.add_template("cmd", &self.command)
            .map_err(|e| format!("Template error: {e}"))?;

        // See PLACEHOLDER_PREFIX: forbid collisions that could forge a placeholder.
        if self.command.contains(PLACEHOLDER_PREFIX) {
            return Err(format!(
                "Command template must not contain the reserved marker '{PLACEHOLDER_PREFIX}'"
            ));
        }

        let mut ph_ctx = serde_json::Map::new();
        let mut substitutions: Vec<(String, String)> = Vec::new();
        for param in &self.parameters {
            match args.get(&param.name) {
                Some(v) if !v.is_null() => {
                    // Non-string values render as compact JSON; strings stay raw.
                    let rendered = match v {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    if rendered.contains(PLACEHOLDER_PREFIX) {
                        return Err(format!(
                            "Value of parameter '{}' must not contain the reserved marker '{PLACEHOLDER_PREFIX}'",
                            param.name
                        ));
                    }
                    let ph = placeholder(substitutions.len());
                    ph_ctx.insert(param.name.clone(), Value::String(ph.clone()));
                    substitutions.push((ph, rendered));
                }
                _ => {
                    ph_ctx.insert(param.name.clone(), Value::Null);
                }
            }
        }

        let rendered = tt
            .render("cmd", &Value::Object(ph_ctx))
            .map_err(|e| format!("Template render error: {e}"))?;

        // Split first (shell quoting), substitute second — see PLACEHOLDER_PREFIX.
        Ok(split(&rendered)
            .map_err(|e| format!("Failed to parse command: {e}"))?
            .into_iter()
            .map(|token| {
                substitutions
                    .iter()
                    .fold(token, |t, (ph, value)| t.replace(ph, value))
            })
            .collect())
    }
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

    /// Load custom tools from disk, returning empty list if missing or malformed.
    pub fn load() -> Self {
        let path = Self::path();
        if !path.exists() {
            return Self::default();
        }
        match std::fs::read_to_string(&path) {
            Ok(text) => match ron::from_str::<ToolList>(&text) {
                Ok(list) => list,
                Err(e) => {
                    tracing::warn!(path = %path.display(), "failed to parse tools.ron, using empty list: {e}");
                    Self::default()
                }
            },
            Err(e) => {
                tracing::warn!(path = %path.display(), "failed to read tools.ron: {e}");
                Self::default()
            }
        }
    }

    /// Save custom tools to disk as RON text.
    pub fn save(&self) {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let config = ron::ser::PrettyConfig::default().new_line("\n");
        match ron::ser::to_string_pretty(self, config) {
            Ok(text) => {
                if let Err(e) = std::fs::write(&path, text) {
                    tracing::error!(path = %path.display(), "failed to save tools.ron: {e}");
                }
            }
            Err(e) => tracing::error!("failed to serialize custom tools: {e}"),
        }
    }

    /// Return the names of every custom tool.
    pub fn names(&self) -> Vec<String> {
        self.custom_tools.iter().map(|ct| ct.name.clone()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::Path;

    #[test]
    fn create_custom_tool() {
        let crate_source = CustomTool {
            name: "crate_source".to_string(),
            description:
                "Find the local source path for a Rust crate from cargo cache. Returns the cached extraction directory containing the full crate source code. Useful for inspecting a crate's API, reading its implementation, or debugging dependencies." .to_string(),
            instruction:
                "Look up Rust crate version and source locations. Before inspecting a Rust dependency's source code, use crate_source to find its local path." .to_string(),
            parameters: vec![
                ToolParameter {
                    name: "crate".to_string(),
                    kind: ParameterType::String,
                    description: "Name of the Rust crate to find (e.g., 'bevy', 'serde', 'nalgebra')".to_string(),
                    required: true,
                },
            ],
            command: "bash -c \"registry=$(ls -1dt ~/.cargo/registry/src/* | head -n1);crate=$(cargo tree -i {crate} | sed -n '1s/ v/-/p');echo \\$registry/\\$crate\"".to_string(),
        };

        let args = json!({"crate": "iced"});
        let result = crate_source
            .execute(&args, Path::new("."), &CancellationToken::new())
            .unwrap();
        println!("{:?}", result);

        let schema = crate_source.schema();
        println!("{}", schema);

        let tools = ToolList {
            custom_tools: vec![crate_source],
        };
        let assets = Path::new("assets");
        let text =
            ron::ser::to_string_pretty(&tools, ron::ser::PrettyConfig::default().new_line("\n"))
                .unwrap();
        std::fs::write(assets.join("tools.ron"), text).unwrap();
        println!("Saved tools to {}", assets.display());
    }
}
