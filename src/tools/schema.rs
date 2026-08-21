//! Strict-mode schema post-processing and stringified-argument decoding.

use serde_json::Value;
use std::collections::HashSet;

/// Adjust the schema in-place for strict tool-calling mode:
/// every property becomes required, and optional properties get `"type": ["T", "null"]` union types.
/// "additionalProperties: false" is automatically added by `genai`.
pub fn make_strict_schema(schema: &mut Value) {
    process_strict(schema);
}

fn process_strict(value: &mut Value) {
    let Value::Object(obj) = value else {
        // Recurse into array items
        if let Value::Array(arr) = value {
            for item in arr.iter_mut() {
                process_strict(item);
            }
        }
        return;
    };

    // If this is an object-typed schema node with properties, enforce that
    // *every* declared property appears in `required`.
    if obj.get("type").and_then(Value::as_str) == Some("object") {
        // Collect property keys and identify optional ones without holding
        // a borrow on `obj` across the mutable `insert` below.
        let (all_keys, optional_keys) =
            if let Some(properties) = obj.get("properties").and_then(|v| v.as_object()) {
                let required_set: HashSet<&str> = obj
                    .get("required")
                    .and_then(Value::as_array)
                    .map(|arr| arr.iter().filter_map(Value::as_str).collect())
                    .unwrap_or_default();

                let all: Vec<String> = properties.keys().cloned().collect();
                let optional: Vec<String> = all
                    .iter()
                    .filter(|k| !required_set.contains(k.as_str()))
                    .cloned()
                    .collect();
                (all, optional)
            } else {
                (Vec::new(), Vec::new())
            };

        if !all_keys.is_empty() {
            obj.insert(
                "required".to_string(),
                Value::Array(all_keys.iter().map(|k| Value::String(k.clone())).collect()),
            );
        }

        // Make optional properties nullable.
        if let Some(props) = obj.get_mut("properties").and_then(|v| v.as_object_mut()) {
            for key in &optional_keys {
                if let Some(prop) = props.get_mut(key) {
                    make_nullable(prop);
                }
            }
        }

        // Change to string type to accept arbitrary key-value data in strict
        // mode, dropping the object-only keyword a strict provider rejects on
        // a string schema.
        if !obj.contains_key("properties") {
            obj.remove("additionalProperties");
            if let Some(type_val) = obj.get_mut("type") {
                *type_val = Value::String("string".into());
            }
        }
    }

    // Recurse into every child value
    for (_k, v) in obj.iter_mut() {
        process_strict(v);
    }
}

/// Modify a property schema in-place so that it accepts `null`.
///
/// Handles both `"type": "T"` → `"type": ["T", "null"]` and
/// `anyOf` → appends `{"type": "null"}`.
fn make_nullable(value: &mut Value) {
    let Value::Object(obj) = value else { return };

    if let Some(type_val) = obj.get_mut("type") {
        match type_val {
            Value::String(s)
                if ["string", "number", "integer", "boolean"].contains(&s.as_str()) =>
            {
                *type_val =
                    Value::Array(vec![Value::String(s.clone()), Value::String("null".into())]);
            }
            Value::Array(arr) if !arr.iter().any(|v| v.as_str() == Some("null")) => {
                arr.push(Value::String("null".into()));
            }
            _ => {}
        }
    }

    // If the property uses `anyOf` (union type from custom tools), add a null variant.
    if let Some(any_of) = obj.get_mut("anyOf").and_then(|v| v.as_array_mut())
        && !any_of.iter().any(|v| {
            v.as_object()
                .and_then(|o| o.get("type"))
                .and_then(Value::as_str)
                == Some("null")
        })
    {
        any_of.push(serde_json::json!({"type": "null"}));
    }
}

/// Undo strict-mode string coercion in tool-call `args`: where `schema`
/// declares an object or array but the argument arrived as a JSON string
/// (strict schemas coerce free-form objects to strings), decode it back.
/// Recurses into declared properties and array items. Undecodable or
/// wrong-kind strings are left as-is so the callee reports the mismatch.
pub fn decode_stringified_args(schema: &Value, value: &mut Value) {
    if let Value::String(s) = &*value
        && let Some(ty) = schema.get("type").and_then(Value::as_str)
        && matches!(ty, "object" | "array")
        && let Ok(decoded) = serde_json::from_str::<Value>(s)
        && ((ty == "object" && decoded.is_object()) || (ty == "array" && decoded.is_array()))
    {
        *value = decoded;
    }

    match value {
        Value::Object(map) => {
            let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
                return;
            };
            for (key, val) in map.iter_mut() {
                if let Some(prop_schema) = properties.get(key) {
                    decode_stringified_args(prop_schema, val);
                }
            }
        }
        Value::Array(items) => {
            let Some(item_schema) = schema.get("items") else {
                return;
            };
            for val in items.iter_mut() {
                decode_stringified_args(item_schema, val);
            }
        }
        _ => {}
    }
}
