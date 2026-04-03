use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectProfile {
    pub project_id: String,
    pub project_root: String,
    pub languages: Vec<String>,
    pub frameworks: Vec<String>,
    pub risk_profile: RiskProfile,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Capability {
    pub id: String,
    pub title: String,
    pub capability_type: CapabilityType,
    pub family: String,
    pub visibility_mode: VisibilityMode,
    pub risk_level: RiskLevel,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RouteDecision {
    pub route: RouteType,
    pub requires_memory_search: bool,
    pub requires_approval: bool,
    pub rationale: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PolicyDecision {
    pub allowed: bool,
    pub reason: String,
    pub approval_scope: Option<ApprovalScope>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum CapabilityType {
    Skill,
    Agent,
    McpTool,
    PluginAction,
    Cli,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum VisibilityMode {
    Core,
    Project,
    Dormant,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum RiskLevel {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum RiskProfile {
    Safe,
    Caution,
    HighRisk,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ApprovalScope {
    Once,
    Session,
    Forever,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum RouteType {
    RuleOnly,
    RuleWithNano,
    Retrieval,
    ToolExecution,
}
