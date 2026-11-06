use serde::Serialize;
use serde_json::{Value, json};

#[derive(Debug, Clone, Serialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

pub fn specs() -> Vec<ToolSpec> {
    vec![
        // TODO(wltu): implement the truncation this description promises
        ToolSpec {
            name: "read_file".into(),
            description: "Read a UTF-8 text file from the project and return its contents. \
                          Paths are relative to the project root; large files are truncated."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Path relative to the project root" }
                },
                "required": ["path"]
            }),
        },
        // TODO(wltu): the approval prompt is the safety gate; no command runs without it
        ToolSpec {
            name: "bash".into(),
            description: "Runs a shell command in the project root; the user approves each command; it returns stdout, stderr and the exit code".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "Command to run in the project root" }
                },
                "required": ["command"]
            }),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn specs_are_valid_object_schemas() {
        for spec in specs() {
            let p = &spec.parameters;
            assert_eq!(p["type"], "object", "{}", spec.name);
            for key in p["required"].as_array().expect("required list") {
                let key = key.as_str().unwrap();
                assert!(
                    p["properties"].get(key).is_some(),
                    "{}: required `{key}` missing from properties",
                    spec.name
                );
            }
        }
    }
}
