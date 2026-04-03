use std::env;
use std::fs;
use std::path::Path;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use the_one_core::contracts::{Capability, RiskLevel, VisibilityMode};
use the_one_core::error::CoreError;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CapabilitySuggestion {
    pub id: String,
    pub title: String,
    pub reason: String,
}

#[derive(Debug, Default)]
pub struct CapabilityRegistry {
    capabilities: Vec<Capability>,
}

impl CapabilityRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, capability: Capability) {
        self.capabilities.push(capability);
    }

    pub fn all(&self) -> &[Capability] {
        &self.capabilities
    }

    pub fn save_to_path(&self, path: &Path) -> Result<(), CoreError> {
        let parent = path.parent().ok_or_else(|| {
            CoreError::InvalidProjectConfig(format!(
                "registry path has no parent: {}",
                path.display()
            ))
        })?;
        fs::create_dir_all(parent)?;
        let payload = serde_json::to_vec_pretty(&self.capabilities)?;
        fs::write(path, payload)?;
        Ok(())
    }

    pub fn load_from_path(path: &Path) -> Result<Self, CoreError> {
        if !path.exists() {
            return Ok(Self::new());
        }

        let body = fs::read_to_string(path)?;
        let capabilities: Vec<Capability> = serde_json::from_str(&body)?;
        Ok(Self { capabilities })
    }

    pub fn default_catalog_path() -> Result<PathBuf, CoreError> {
        if let Ok(path) = env::var("THE_ONE_HOME") {
            let path = PathBuf::from(path);
            if !path.is_absolute() {
                return Err(CoreError::InvalidProjectConfig(
                    "THE_ONE_HOME must be absolute".to_string(),
                ));
            }
            return Ok(path.join("registry").join("capabilities.json"));
        }

        let home = env::var("HOME").map_err(|_| {
            CoreError::InvalidProjectConfig(
                "HOME is not set and THE_ONE_HOME not provided".to_string(),
            )
        })?;

        Ok(PathBuf::from(home)
            .join(".the-one")
            .join("registry")
            .join("capabilities.json"))
    }

    pub fn load_global_default() -> Result<Self, CoreError> {
        let path = Self::default_catalog_path()?;
        Self::load_from_path(&path)
    }

    pub fn save_global_default(&self) -> Result<(), CoreError> {
        let path = Self::default_catalog_path()?;
        self.save_to_path(&path)
    }

    pub fn visible_capabilities(&self, mode: VisibilityMode) -> Vec<Capability> {
        self.capabilities
            .iter()
            .filter(|cap| cap.visibility_mode == mode)
            .cloned()
            .collect()
    }

    pub fn suggest(
        &self,
        query: &str,
        risk_budget: RiskLevel,
        max: usize,
    ) -> Vec<CapabilitySuggestion> {
        let query_lower = query.to_lowercase();

        let mut matches = self
            .capabilities
            .iter()
            .filter(|cap| within_risk_budget(cap.risk_level.clone(), risk_budget.clone()))
            .filter(|cap| {
                cap.id.to_lowercase().contains(&query_lower)
                    || cap.title.to_lowercase().contains(&query_lower)
                    || cap.description.to_lowercase().contains(&query_lower)
            })
            .take(max)
            .map(|cap| CapabilitySuggestion {
                id: cap.id.clone(),
                title: cap.title.clone(),
                reason: format!("matched query '{}'", query),
            })
            .collect::<Vec<_>>();

        if matches.is_empty() {
            matches = self
                .capabilities
                .iter()
                .filter(|cap| cap.visibility_mode == VisibilityMode::Core)
                .take(max)
                .map(|cap| CapabilitySuggestion {
                    id: cap.id.clone(),
                    title: cap.title.clone(),
                    reason: "fallback core capability".to_string(),
                })
                .collect();
        }

        matches
    }
}

fn within_risk_budget(candidate: RiskLevel, budget: RiskLevel) -> bool {
    risk_rank(candidate) <= risk_rank(budget)
}

fn risk_rank(level: RiskLevel) -> usize {
    match level {
        RiskLevel::Low => 1,
        RiskLevel::Medium => 2,
        RiskLevel::High => 3,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::CapabilityRegistry;
    use the_one_core::contracts::{Capability, CapabilityType, RiskLevel, VisibilityMode};

    #[test]
    fn test_suggest_filters_by_risk_budget() {
        let mut registry = CapabilityRegistry::new();
        registry.add(Capability {
            id: "safe.search".to_string(),
            title: "Safe Search".to_string(),
            capability_type: CapabilityType::McpTool,
            family: "search".to_string(),
            visibility_mode: VisibilityMode::Core,
            risk_level: RiskLevel::Low,
            description: "search docs".to_string(),
        });
        registry.add(Capability {
            id: "danger.deploy".to_string(),
            title: "Danger Deploy".to_string(),
            capability_type: CapabilityType::McpTool,
            family: "deploy".to_string(),
            visibility_mode: VisibilityMode::Project,
            risk_level: RiskLevel::High,
            description: "deploy production".to_string(),
        });

        let suggestions = registry.suggest("deploy", RiskLevel::Medium, 5);
        assert!(suggestions.iter().all(|s| s.id != "danger.deploy"));
    }

    #[test]
    fn test_registry_save_and_load_roundtrip() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let file = temp.path().join("registry/capabilities.json");

        let mut registry = CapabilityRegistry::new();
        registry.add(Capability {
            id: "docs.search".to_string(),
            title: "Docs Search".to_string(),
            capability_type: CapabilityType::McpTool,
            family: "docs".to_string(),
            visibility_mode: VisibilityMode::Core,
            risk_level: RiskLevel::Low,
            description: "search docs".to_string(),
        });

        registry.save_to_path(&file).expect("save should succeed");
        assert!(fs::metadata(&file).is_ok());

        let loaded = CapabilityRegistry::load_from_path(&file).expect("load should succeed");
        assert_eq!(loaded.all().len(), 1);
        assert_eq!(loaded.all()[0].id, "docs.search");
    }

    #[test]
    fn test_default_catalog_path_uses_the_one_home() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        std::env::set_var("THE_ONE_HOME", temp.path().display().to_string());

        let path = CapabilityRegistry::default_catalog_path().expect("path should resolve");
        assert!(path.ends_with("registry/capabilities.json"));
        assert!(path.starts_with(temp.path()));

        std::env::remove_var("THE_ONE_HOME");
    }
}
