use crate::contracts::RiskLevel;
use crate::error::CoreError;

#[derive(Debug, Clone)]
pub struct PolicyLimits {
    pub max_tool_suggestions: usize,
    pub max_search_hits: usize,
    pub max_raw_section_bytes: usize,
    pub max_enabled_families: usize,
}

impl Default for PolicyLimits {
    fn default() -> Self {
        Self {
            max_tool_suggestions: 5,
            max_search_hits: 5,
            max_raw_section_bytes: 24 * 1024,
            max_enabled_families: 12,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct PolicyEngine {
    limits: PolicyLimits,
}

impl PolicyEngine {
    pub fn new(limits: PolicyLimits) -> Self {
        Self { limits }
    }

    pub fn limits(&self) -> &PolicyLimits {
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
    use super::{PolicyEngine, PolicyLimits};

    #[test]
    fn test_policy_clamps_values_to_limits() {
        let engine = PolicyEngine::new(PolicyLimits {
            max_tool_suggestions: 3,
            max_search_hits: 2,
            max_raw_section_bytes: 100,
            max_enabled_families: 1,
        });

        assert_eq!(engine.clamp_suggestions(10), 3);
        assert_eq!(engine.clamp_search_hits(10), 2);
        assert_eq!(engine.clamp_doc_bytes(9999), 100);
        assert!(engine.validate_enabled_families_count(2).is_err());
    }
}
