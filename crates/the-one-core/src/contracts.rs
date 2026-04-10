use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum MemoryNavigationNodeKind {
    #[serde(rename = "drawer")]
    Drawer,
    #[serde(rename = "closet")]
    Closet,
    #[serde(rename = "room")]
    Room,
}

impl MemoryNavigationNodeKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Drawer => "drawer",
            Self::Closet => "closet",
            Self::Room => "room",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryNavigationNode {
    pub node_id: String,
    pub project_id: String,
    pub kind: MemoryNavigationNodeKind,
    pub label: String,
    pub parent_node_id: Option<String>,
    pub wing: Option<String>,
    pub hall: Option<String>,
    pub room: Option<String>,
    pub updated_at_epoch_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryNavigationTunnel {
    pub tunnel_id: String,
    pub project_id: String,
    pub from_node_id: String,
    pub to_node_id: String,
    pub updated_at_epoch_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AaakPattern {
    pub pattern_key: String,
    pub role: String,
    pub canonical_text: String,
    pub occurrence_count: usize,
    pub confidence_percent: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AaakLesson {
    pub lesson_id: String,
    pub project_id: String,
    pub pattern_key: String,
    pub role: String,
    pub canonical_text: String,
    pub occurrence_count: usize,
    pub confidence_percent: u8,
    pub source_transcript_path: Option<String>,
    pub updated_at_epoch_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AaakCompressionResult {
    pub used_verbatim: bool,
    pub confidence_percent: u8,
    pub original_message_count: usize,
    pub sequence_item_count: usize,
    pub compressed_payload_json: String,
    pub patterns: Vec<AaakPattern>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AaakTeachOutcome {
    pub lessons_written: usize,
    pub lessons: Vec<AaakLesson>,
    pub skipped_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DiaryEntry {
    pub entry_id: String,
    pub project_id: String,
    pub entry_date: String,
    pub mood: Option<String>,
    pub tags: Vec<String>,
    pub content: String,
    pub created_at_epoch_ms: i64,
    pub updated_at_epoch_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DiarySummary {
    pub date_from: Option<String>,
    pub date_to: Option<String>,
    pub entry_count: usize,
    pub summary: String,
    pub highlights: Vec<String>,
}

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
