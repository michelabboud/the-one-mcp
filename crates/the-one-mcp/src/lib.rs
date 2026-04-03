pub mod adapter_core;
pub mod api;
pub mod broker;
pub mod swagger;
pub mod transport;

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
            "config.update.request.schema.json",
            "config.update.response.schema.json",
            "docs.create.request.schema.json",
            "docs.create.response.schema.json",
            "docs.delete.request.schema.json",
            "docs.delete.response.schema.json",
            "docs.get.request.schema.json",
            "docs.get.response.schema.json",
            "docs.get_section.request.schema.json",
            "docs.get_section.response.schema.json",
            "docs.list.request.schema.json",
            "docs.list.response.schema.json",
            "docs.move.request.schema.json",
            "docs.move.response.schema.json",
            "docs.reindex.request.schema.json",
            "docs.reindex.response.schema.json",
            "docs.trash.empty.request.schema.json",
            "docs.trash.empty.response.schema.json",
            "docs.trash.list.request.schema.json",
            "docs.trash.list.response.schema.json",
            "docs.trash.restore.request.schema.json",
            "docs.trash.restore.response.schema.json",
            "docs.update.request.schema.json",
            "docs.update.response.schema.json",
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
            "tool.add.request.schema.json",
            "tool.add.response.schema.json",
            "tool.disable.request.schema.json",
            "tool.disable.response.schema.json",
            "tool.enable.request.schema.json",
            "tool.enable.response.schema.json",
            "tool.info.request.schema.json",
            "tool.info.response.schema.json",
            "tool.install.request.schema.json",
            "tool.install.response.schema.json",
            "tool.list.request.schema.json",
            "tool.list.response.schema.json",
            "tool.remove.request.schema.json",
            "tool.remove.response.schema.json",
            "tool.run.request.schema.json",
            "tool.run.response.schema.json",
            "tool.search.request.schema.json",
            "tool.search.response.schema.json",
            "tool.suggest.request.schema.json",
            "tool.suggest.response.schema.json",
            "tool.update.request.schema.json",
            "tool.update.response.schema.json",
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
