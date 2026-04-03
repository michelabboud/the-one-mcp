use crate::contracts::RiskLevel;
use crate::error::CoreError;
use crate::limits::ConfigurableLimits;

#[derive(Debug, Clone)]
pub struct PolicyEngine {
    limits: ConfigurableLimits,
}

impl Default for PolicyEngine {
    fn default() -> Self {
        Self {
            limits: ConfigurableLimits::default().validated(),
        }
    }
}

impl PolicyEngine {
    pub fn new(limits: ConfigurableLimits) -> Self {
        Self {
            limits: limits.validated(),
        }
    }

    pub fn limits(&self) -> &ConfigurableLimits {
        &self.limits
    }

    pub fn clamp_suggestions(&self, requested: usize) -> usize {
        requested.min(self.limits.max_tool_suggestions)
    }

    pub fn clamp_search_hits(&self, requested: usize) -> usize {
        requested.min(self.limits.max_search_hits)
    }

    pub fn clamp_doc_bytes(&self, requested: usize) -> usize {
        requested.min(self.limits.max_raw_section_bytes)
    }

    pub fn validate_enabled_families_count(&self, count: usize) -> Result<(), CoreError> {
        if count <= self.limits.max_enabled_families {
            return Ok(());
        }

        Err(CoreError::PolicyDenied(format!(
            "enabled families exceed policy limit: {} > {}",
            count, self.limits.max_enabled_families
        )))
    }

    pub fn requires_approval(&self, risk_level: RiskLevel) -> bool {
        matches!(risk_level, RiskLevel::High)
    }
}

#[cfg(test)]
mod tests {
    use super::PolicyEngine;
    use crate::limits::ConfigurableLimits;

    #[test]
    fn test_policy_clamps_values_to_limits() {
        let engine = PolicyEngine::new(ConfigurableLimits {
            max_tool_suggestions: 3,
            max_search_hits: 2,
            max_raw_section_bytes: 1024,
            max_enabled_families: 1,
            ..ConfigurableLimits::default()
        });

        assert_eq!(engine.clamp_suggestions(10), 3);
        assert_eq!(engine.clamp_search_hits(10), 2);
        assert_eq!(engine.clamp_doc_bytes(9999), 1024);
        assert!(engine.validate_enabled_families_count(2).is_err());
    }
}
