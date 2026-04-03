pub mod adapter_core;
pub mod api;
pub mod broker;
pub mod swagger;

pub const MCP_SCHEMA_VERSION: &str = "v1beta";

pub fn schema_version() -> &'static str {
    MCP_SCHEMA_VERSION
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::fs;
    use std::path::PathBuf;

    #[test]
    fn test_v1beta_schema_files_exist_and_are_valid_json() {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let schema_dir = manifest_dir.join("../../schemas/mcp/v1beta");

        let expected = [
            "audit.events.request.schema.json",
            "audit.events.response.schema.json",
            "config.export.request.schema.json",
            "config.export.response.schema.json",
            "docs.get.request.schema.json",
            "docs.get.response.schema.json",
            "docs.get_section.request.schema.json",
            "docs.get_section.response.schema.json",
            "docs.list.request.schema.json",
            "docs.list.response.schema.json",
            "memory.fetch_chunk.request.schema.json",
            "memory.fetch_chunk.response.schema.json",
            "memory.search.request.schema.json",
            "memory.search.response.schema.json",
            "metrics.snapshot.request.schema.json",
            "metrics.snapshot.response.schema.json",
            "openapi.swagger.json",
            "project.init.request.schema.json",
            "project.init.response.schema.json",
            "project.profile.get.request.schema.json",
            "project.profile.get.response.schema.json",
            "project.refresh.request.schema.json",
            "project.refresh.response.schema.json",
            "tool.enable.request.schema.json",
            "tool.enable.response.schema.json",
            "tool.run.request.schema.json",
            "tool.run.response.schema.json",
            "tool.search.request.schema.json",
            "tool.search.response.schema.json",
            "tool.suggest.request.schema.json",
            "tool.suggest.response.schema.json",
        ]
        .into_iter()
        .map(str::to_string)
        .collect::<HashSet<_>>();

        let entries = fs::read_dir(&schema_dir).expect("schema dir should exist");
        let mut seen = HashSet::new();
        for entry in entries {
            let entry = entry.expect("entry should be readable");
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            let body = fs::read_to_string(&path).expect("schema file should be readable");
            let _: serde_json::Value =
                serde_json::from_str(&body).expect("schema file should be valid json");
            let file_name = path
                .file_name()
                .and_then(|name| name.to_str())
                .expect("schema file should have name")
                .to_string();
            seen.insert(file_name);
        }

        assert_eq!(seen, expected);
    }

    #[test]
    fn test_v1beta_schema_ids_and_draft_are_consistent() {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let schema_dir = manifest_dir.join("../../schemas/mcp/v1beta");
        let entries = fs::read_dir(&schema_dir).expect("schema dir should exist");

        for entry in entries {
            let entry = entry.expect("entry should be readable");
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }

            let body = fs::read_to_string(&path).expect("schema should be readable");
            let value: serde_json::Value =
                serde_json::from_str(&body).expect("schema should be valid json");

            let draft = value
                .get("$schema")
                .and_then(|item| item.as_str())
                .expect("schema should declare draft");
            assert_eq!(draft, "https://json-schema.org/draft/2020-12/schema");

            let id = value
                .get("$id")
                .and_then(|item| item.as_str())
                .expect("schema should declare id");
            assert!(
                id.starts_with("the-one.mcp.v1beta."),
                "unexpected schema id: {id}"
            );
        }
    }
}
