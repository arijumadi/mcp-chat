use base64::Engine;
use serde_json::Value;

use crate::mcp::types::ToolInfo;
use crate::util::sanitize_tool_name;

#[derive(Debug, Clone)]
pub struct HeaderParam {
    pub path: Vec<String>,
    pub header: String,
}

/// Validate the `x-mcp-header` annotations in a tool's inputSchema.
///
/// Rules from the 2026-07-28 Streamable HTTP transport: only statically
/// reachable object properties, only string/integer/boolean parameters, unique
/// case-insensitive header names, and a valid HTTP field-name token. An invalid
/// annotation invalidates the whole tool, so the caller must hide it.
pub fn header_params(tool: &ToolInfo) -> Result<Vec<HeaderParam>, String> {
    let schema = &tool.input_schema;
    let mut params = Vec::new();
    let mut seen = std::collections::HashSet::new();
    walk_properties(schema, &mut Vec::new(), &mut params, &mut seen)?;
    let total_annotations = count_annotations(schema);
    if total_annotations != params.len() {
        return Err("x-mcp-header is only valid on statically reachable properties".into());
    }
    Ok(params)
}

fn walk_properties(
    node: &Value,
    path: &mut Vec<String>,
    out: &mut Vec<HeaderParam>,
    seen: &mut std::collections::HashSet<String>,
) -> Result<(), String> {
    if let Some(properties) = node.get("properties").and_then(Value::as_object) {
        for (name, property) in properties {
            if let Some(header) = property.get("x-mcp-header") {
                let header = header
                    .as_str()
                    .ok_or_else(|| "x-mcp-header must be a string".to_owned())?;
                if !is_header_token(header) {
                    return Err(format!("invalid header name {header:?}"));
                }
                let key = header.to_ascii_lowercase();
                if !seen.insert(key) {
                    return Err(format!("duplicate x-mcp-header {header:?}"));
                }
                let ty = property.get("type").and_then(Value::as_str).unwrap_or("");
                if !matches!(ty, "string" | "integer" | "boolean") {
                    return Err(format!(
                        "x-mcp-header on {name:?} must be string, integer or boolean"
                    ));
                }
                if ty == "integer" {
                    if let Some(min) = property.get("minimum").and_then(Value::as_i64) {
                        if min < -9007199254740991 {
                            return Err(format!(
                                "integer header {name:?} is outside the safe range"
                            ));
                        }
                    }
                }
                let mut full_path = path.clone();
                full_path.push(name.clone());
                out.push(HeaderParam {
                    path: full_path,
                    header: header.to_owned(),
                });
            }
            path.push(name.clone());
            walk_properties(property, path, out, seen)?;
            path.pop();
        }
    }
    Ok(())
}

fn count_annotations(node: &Value) -> usize {
    match node {
        Value::Object(map) => map
            .iter()
            .map(|(key, value)| usize::from(key == "x-mcp-header") + count_annotations(value))
            .sum(),
        Value::Array(items) => items.iter().map(count_annotations).sum(),
        _ => 0,
    }
}

fn is_header_token(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'!' | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'.'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'|'
                        | b'~'
                )
        })
}

/// Return `Mcp-Param-*` header pairs for a tool call. Values are encoded per the
/// spec: plain visible ASCII unless it needs base64 sentinel encoding.
pub fn extract_header_values(tool: &ToolInfo, arguments: &Value) -> Vec<(String, String)> {
    let Ok(params) = header_params(tool) else {
        return Vec::new();
    };
    let mut values = Vec::new();
    for param in params {
        let mut cursor = arguments;
        let mut found = true;
        for segment in &param.path {
            match cursor.get(segment) {
                Some(next) => cursor = next,
                None => {
                    found = false;
                    break;
                }
            }
        }
        if !found || cursor.is_null() {
            continue;
        }
        let raw = match cursor {
            Value::String(text) => text.clone(),
            Value::Bool(flag) => flag.to_string(),
            Value::Number(number) if number.is_i64() => number.to_string(),
            _ => continue,
        };
        values.push((
            format!("Mcp-Param-{}", param.header),
            encode_header_value(&raw),
        ));
    }
    values
}

pub fn encode_header_value(raw: &str) -> String {
    let safe = !raw.is_empty()
        && raw
            .bytes()
            .all(|byte| (0x21..=0x7e).contains(&byte) || byte == b' ' || byte == b'\t')
        && raw == raw.trim()
        && !(raw.starts_with("=?base64?") && raw.ends_with("?="));
    if safe {
        raw.to_owned()
    } else {
        format!(
            "=?base64?{}?=",
            base64::engine::general_purpose::STANDARD.encode(raw.as_bytes())
        )
    }
}

/// Filter tools whose schema has invalid header annotations.
pub fn filter_valid_tools(tools: Vec<ToolInfo>) -> Vec<ToolInfo> {
    tools
        .into_iter()
        .filter(|tool| match header_params(tool) {
            Ok(_) => true,
            Err(error) => {
                tracing::warn!(tool = %tool.name, %error, "excluding MCP tool with invalid x-mcp-header");
                false
            }
        })
        .collect()
}

#[derive(Debug, Clone)]
pub struct McpTool {
    pub server_id: String,
    pub server_slug: String,
    pub server_name: String,
    pub remote_name: String,
    pub exposed_name: String,
    pub description: String,
    pub input_schema: Value,
}

impl McpTool {
    pub fn to_info(&self) -> ToolInfo {
        ToolInfo {
            name: self.remote_name.clone(),
            title: None,
            description: self.description.clone(),
            input_schema: self.input_schema.clone(),
            output_schema: None,
            annotations: None,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ToolRegistry {
    pub tools: Vec<McpTool>,
}

impl ToolRegistry {
    pub fn from_servers(
        servers: &[(String, String, String, Vec<ToolInfo>)],
    ) -> crate::Result<Self> {
        let mut tools = Vec::new();
        let mut used = std::collections::HashSet::new();
        for (server_id, slug, server_name, infos) in servers {
            for info in infos {
                let base = sanitize_tool_name(&format!("{slug}__{}", info.name));
                let mut candidate = base.clone();
                let mut suffix = 2;
                while used.contains(&candidate) {
                    candidate = format!("{}_{}", base.chars().take(60).collect::<String>(), suffix);
                    suffix += 1;
                }
                used.insert(candidate.clone());
                tools.push(McpTool {
                    server_id: server_id.clone(),
                    server_slug: slug.clone(),
                    server_name: server_name.clone(),
                    remote_name: info.name.clone(),
                    exposed_name: candidate,
                    description: info.description.clone(),
                    input_schema: info.input_schema.clone(),
                });
            }
        }
        Ok(Self { tools })
    }

    pub fn get(&self, exposed_name: &str) -> Option<&McpTool> {
        self.tools
            .iter()
            .find(|tool| tool.exposed_name == exposed_name)
    }

    pub fn by_server(&self, server_id: &str) -> Vec<&McpTool> {
        self.tools
            .iter()
            .filter(|tool| tool.server_id == server_id)
            .collect()
    }
}

pub fn exposed_name(
    server_slug: &str,
    tool_name: &str,
    used: &mut std::collections::HashSet<String>,
) -> String {
    let base = sanitize_tool_name(&format!("{server_slug}__{tool_name}"));
    let mut candidate = base.clone();
    let mut suffix = 2;
    while used.contains(&candidate) {
        candidate = format!("{}_{}", base.chars().take(60).collect::<String>(), suffix);
        suffix += 1;
    }
    used.insert(candidate.clone());
    candidate
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tool(schema: Value) -> ToolInfo {
        ToolInfo {
            name: "execute_sql".into(),
            title: None,
            description: String::new(),
            input_schema: schema,
            output_schema: None,
            annotations: None,
        }
    }

    #[test]
    fn extracts_and_encodes_header_values() {
        let tool = tool(json!({
            "type": "object",
            "properties": {
                "region": {"type": "string", "x-mcp-header": "Region"},
                "nested": {
                    "type": "object",
                    "properties": {"count": {"type": "integer", "x-mcp-header": "Count"}}
                }
            }
        }));
        let values = extract_header_values(
            &tool,
            &json!({"region": "us-west1", "nested": {"count": 3}}),
        );
        assert!(values.contains(&("Mcp-Param-Region".into(), "us-west1".into())));
        assert!(values.contains(&("Mcp-Param-Count".into(), "3".into())));
    }

    #[test]
    fn rejects_annotations_inside_arrays() {
        let tool = tool(json!({
            "type": "object",
            "properties": {
                "items": {
                    "type": "array",
                    "items": {"type": "string", "x-mcp-header": "Bad"}
                }
            }
        }));
        assert!(header_params(&tool).is_err());
    }

    #[test]
    fn rejects_number_and_duplicate_headers() {
        let number = tool(json!({
            "type": "object",
            "properties": {"x": {"type": "number", "x-mcp-header": "X"}}
        }));
        assert!(header_params(&number).is_err());
        let duplicate = tool(json!({
            "type": "object",
            "properties": {
                "a": {"type": "string", "x-mcp-header": "X"},
                "b": {"type": "string", "x-mcp-header": "x"}
            }
        }));
        assert!(header_params(&duplicate).is_err());
    }

    #[test]
    fn base64_encodes_unsafe_values() {
        assert_eq!(encode_header_value("us-west1"), "us-west1");
        assert!(encode_header_value("Hello, 世界").starts_with("=?base64?"));
    }
}
