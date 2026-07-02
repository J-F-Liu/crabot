use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use shell_words::split;
use tinytemplate::TinyTemplate;

use super::{Tool, ToolRef};

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

    fn execute(&self, args: &Value, workspace: &Path) -> Result<String, String> {
        // Build context: all defined params default to null,
        // then overlay with actual args.
        let mut ctx = serde_json::Map::new();
        for param in &self.parameters {
            ctx.insert(param.name.clone(), Value::Null);
        }
        if let Some(obj) = args.as_object() {
            for (key, val) in obj {
                ctx.insert(key.clone(), val.clone());
            }
        }

        let mut tt = TinyTemplate::new();
        tt.add_template("cmd", &self.command)
            .map_err(|e| format!("Template error: {e}"))?;
        let rendered = tt
            .render("cmd", &Value::Object(ctx))
            .map_err(|e| format!("Template render error: {e}"))?;

        // Split into executable and arguments (honouring shell quoting).
        let parts = split(&rendered).map_err(|e| format!("Failed to parse command: {e}"))?;
        let (exe, args) = parts
            .split_first()
            .ok_or_else(|| "Empty command template".to_string())?;

        let child = Command::new(exe)
            .args(args)
            .current_dir(workspace)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| format!("Failed to execute custom tool '{}': {e}", self.name))?;

        let output = child
            .wait_with_output()
            .map_err(|e| format!("Failed to wait on custom tool '{}': {e}", self.name))?;

        Ok(super::format_command_output(&output))
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
        home::home_dir()
            .unwrap_or_default()
            .join(".crabot")
            .join("tools.ron")
    }

    /// Load custom tools from disk, returning empty list if missing or malformed.
    pub fn load() -> Self {
        let path = Self::path();
        if !path.exists() {
            return Self::default();
        }
        match std::fs::read_to_string(&path) {
            Ok(text) => ron::from_str::<ToolList>(&text).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    /// Save custom tools to disk as RON text.
    #[allow(dead_code)]
    pub fn save(&self) {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(text) = ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default()) {
            let _ = std::fs::write(&path, text);
        }
    }

    /// Convert configs into runtime `ToolRef` instances.
    pub fn build_tools(&self) -> Vec<ToolRef> {
        self.custom_tools
            .iter()
            .map(|t| Arc::new(t.clone()) as ToolRef)
            .collect()
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
                ToolParameter {
                    name: "version".to_string(),
                    kind: ParameterType::String,
                    description: "Exact semver of the crate (e.g., '0.14.0')".to_string(),
                    required: true,
                },
            ],
            command: "bash -c \"echo ~/.cargo/registry/src/rsproxy.cn-e3de039b2554c837/{crate}-{version}\"".to_string(),
        };

        let args = json!({"crate": "iced", "version": "0.14.0"});
        let result = crate_source.execute(&args, Path::new(".")).unwrap();
        println!("{}", result);

        let schema = crate_source.schema();
        println!("{}", schema);

        let tools = ToolList {
            custom_tools: vec![crate_source],
        };
        let tmp = Path::new("tmp");
        if !tmp.is_dir() {
            std::fs::create_dir(tmp).unwrap()
        };
        let text = ron::ser::to_string_pretty(&tools, ron::ser::PrettyConfig::default()).unwrap();
        std::fs::write(tmp.join("tools.ron"), text).unwrap();
        println!("Saved tools to {}", tmp.display());
    }
}
