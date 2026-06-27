use indexmap::IndexMap;

use crate::tools::DevTool;

/// Generate an XML-formatted summary of enabled dev tools.
pub fn tools_summary(selected: &IndexMap<DevTool, bool>) -> String {
    let mut result = String::new();
    result.push_str("<available-tools>\n");

    for (tool, &enabled) in selected {
        if enabled {
            result.push_str(&format!(
                "<tool name=\"{}\">{}</tool>\n",
                tool.name(),
                tool.instruction()
            ));
        }
    }

    result.push_str("</available-tools>\n");
    result.push_str("Tools can be enabled or disabled at any time. A tool used earlier in the conversation may no longer be available. Before using a tool, verify that it is currently available. You may also have access to additional tools not listed here.\n");
    result
}
