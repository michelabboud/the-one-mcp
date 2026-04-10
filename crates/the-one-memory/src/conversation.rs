use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Hash, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ConversationRole {
    System,
    User,
    Assistant,
    Tool,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConversationFormat {
    #[serde(rename = "openai_messages")]
    OpenAiMessages,
    ClaudeTranscript,
    GenericJsonl,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConversationMessage {
    pub role: ConversationRole,
    pub content: String,
    pub turn_index: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConversationTranscript {
    pub source_id: String,
    pub messages: Vec<ConversationMessage>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AaakPatternCandidate {
    pub pattern_key: String,
    pub role: ConversationRole,
    pub canonical_text: String,
    pub occurrence_count: usize,
    pub confidence_percent: u8,
    pub turn_indexes: Vec<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AaakEnvelope {
    pub source_id: String,
    pub used_verbatim: bool,
    pub motifs: Vec<AaakPatternCandidate>,
    pub sequence: Vec<AaakSequenceItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AaakSequenceItem {
    Verbatim {
        message: ConversationMessage,
    },
    MotifRef {
        pattern_key: String,
        turn_index: usize,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AaakCompressionArtifact {
    pub used_verbatim: bool,
    pub confidence_percent: u8,
    pub patterns: Vec<AaakPatternCandidate>,
    pub envelope: AaakEnvelope,
}

impl ConversationTranscript {
    pub fn from_json_str(
        source_id: &str,
        format: ConversationFormat,
        input: &str,
    ) -> Result<Self, String> {
        match format {
            ConversationFormat::OpenAiMessages => Self::from_message_array_json(source_id, input),
            ConversationFormat::ClaudeTranscript => Self::from_message_array_json(source_id, input),
            ConversationFormat::GenericJsonl => Self::from_jsonl(source_id, input),
        }
    }

    fn from_message_array_json(source_id: &str, input: &str) -> Result<Self, String> {
        #[derive(Deserialize)]
        struct RawMessage {
            role: String,
            content: serde_json::Value,
        }

        let raw_messages: Vec<RawMessage> =
            serde_json::from_str(input).map_err(|error| format!("invalid transcript: {error}"))?;

        let messages = raw_messages
            .into_iter()
            .enumerate()
            .map(|(turn_index, message)| ConversationMessage {
                role: match message.role.as_str() {
                    "system" => ConversationRole::System,
                    "user" => ConversationRole::User,
                    "assistant" => ConversationRole::Assistant,
                    "tool" => ConversationRole::Tool,
                    _ => ConversationRole::Unknown,
                },
                content: Self::normalize_content(message.content),
                turn_index,
            })
            .collect();

        Ok(Self {
            source_id: source_id.to_string(),
            messages,
        })
    }

    fn from_jsonl(source_id: &str, input: &str) -> Result<Self, String> {
        let mut messages = Vec::new();

        for (turn_index, line) in input.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            let value: serde_json::Value = serde_json::from_str(line)
                .map_err(|error| format!("invalid jsonl line: {error}"))?;
            let role = value
                .get("role")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            let content = value
                .get("content")
                .cloned()
                .unwrap_or(serde_json::Value::Null);

            messages.push(ConversationMessage {
                role: match role {
                    "system" => ConversationRole::System,
                    "user" => ConversationRole::User,
                    "assistant" => ConversationRole::Assistant,
                    "tool" => ConversationRole::Tool,
                    _ => ConversationRole::Unknown,
                },
                content: Self::normalize_content(content),
                turn_index,
            });
        }

        Ok(Self {
            source_id: source_id.to_string(),
            messages,
        })
    }

    fn normalize_content(content: serde_json::Value) -> String {
        match content {
            serde_json::Value::Null => String::new(),
            serde_json::Value::String(text) => text,
            serde_json::Value::Number(number) => number.to_string(),
            serde_json::Value::Bool(value) => value.to_string(),
            serde_json::Value::Array(items) => items
                .into_iter()
                .map(Self::normalize_content)
                .filter(|text| !text.is_empty())
                .collect::<Vec<_>>()
                .join(" "),
            serde_json::Value::Object(mut object) => {
                if let Some(text) = object.remove("text") {
                    let normalized = Self::normalize_content(text);
                    if !normalized.is_empty() {
                        return normalized;
                    }
                }

                if let Some(parts) = object.remove("parts") {
                    let normalized = Self::normalize_content(parts);
                    if !normalized.is_empty() {
                        return normalized;
                    }
                }

                object
                    .into_iter()
                    .filter_map(|(_, value)| {
                        let text = Self::normalize_content(value);
                        (!text.is_empty()).then_some(text)
                    })
                    .collect::<Vec<_>>()
                    .join(" ")
            }
        }
    }

    pub fn compress_aaak(
        &self,
        min_occurrences: usize,
        min_confidence: u8,
    ) -> AaakCompressionArtifact {
        let patterns = self
            .derive_aaak_patterns(min_occurrences)
            .into_iter()
            .filter(|pattern| pattern.confidence_percent >= min_confidence)
            .collect::<Vec<_>>();

        if patterns.is_empty() {
            return AaakCompressionArtifact {
                used_verbatim: true,
                confidence_percent: 0,
                patterns: Vec::new(),
                envelope: AaakEnvelope {
                    source_id: self.source_id.clone(),
                    used_verbatim: true,
                    motifs: Vec::new(),
                    sequence: self
                        .messages
                        .iter()
                        .cloned()
                        .map(|message| AaakSequenceItem::Verbatim { message })
                        .collect(),
                },
            };
        }

        let mut turn_to_pattern = HashMap::new();
        for pattern in &patterns {
            for turn_index in &pattern.turn_indexes {
                turn_to_pattern.insert(*turn_index, pattern.pattern_key.clone());
            }
        }

        let sequence = self
            .messages
            .iter()
            .cloned()
            .map(|message| {
                if let Some(pattern_key) = turn_to_pattern.get(&message.turn_index) {
                    AaakSequenceItem::MotifRef {
                        pattern_key: pattern_key.clone(),
                        turn_index: message.turn_index,
                    }
                } else {
                    AaakSequenceItem::Verbatim { message }
                }
            })
            .collect::<Vec<_>>();

        let confidence_percent = patterns
            .iter()
            .map(|pattern| pattern.confidence_percent as usize)
            .sum::<usize>()
            .checked_div(patterns.len())
            .unwrap_or(0) as u8;

        AaakCompressionArtifact {
            used_verbatim: false,
            confidence_percent,
            patterns: patterns.clone(),
            envelope: AaakEnvelope {
                source_id: self.source_id.clone(),
                used_verbatim: false,
                motifs: patterns,
                sequence,
            },
        }
    }

    pub fn derive_aaak_patterns(&self, min_occurrences: usize) -> Vec<AaakPatternCandidate> {
        let mut grouped: HashMap<(ConversationRole, String), Vec<ConversationMessage>> =
            HashMap::new();

        for message in &self.messages {
            grouped
                .entry((message.role.clone(), message.content.clone()))
                .or_default()
                .push(message.clone());
        }

        let mut patterns = grouped
            .into_iter()
            .filter_map(|((role, canonical_text), messages)| {
                if messages.len() < min_occurrences || canonical_text.trim().is_empty() {
                    return None;
                }

                let confidence_percent = aaak_confidence_percent(&canonical_text, messages.len());
                let turn_indexes = messages
                    .iter()
                    .map(|message| message.turn_index)
                    .collect::<Vec<_>>();
                Some(AaakPatternCandidate {
                    pattern_key: aaak_pattern_key(&role, &canonical_text),
                    role,
                    canonical_text,
                    occurrence_count: messages.len(),
                    confidence_percent,
                    turn_indexes,
                })
            })
            .collect::<Vec<_>>();

        patterns.sort_by(|left, right| {
            right
                .occurrence_count
                .cmp(&left.occurrence_count)
                .then_with(|| left.canonical_text.cmp(&right.canonical_text))
                .then_with(|| left.pattern_key.cmp(&right.pattern_key))
        });
        patterns
    }
}

impl AaakEnvelope {
    pub fn to_json_string(&self) -> Result<String, String> {
        serde_json::to_string(self).map_err(|error| format!("invalid aaak envelope: {error}"))
    }

    pub fn from_json_str(input: &str) -> Result<Self, String> {
        serde_json::from_str(input).map_err(|error| format!("invalid aaak envelope: {error}"))
    }

    pub fn expand(&self) -> Result<ConversationTranscript, String> {
        let motifs = self
            .motifs
            .iter()
            .cloned()
            .map(|motif| (motif.pattern_key.clone(), motif))
            .collect::<HashMap<_, _>>();

        let mut messages = Vec::with_capacity(self.sequence.len());
        for item in &self.sequence {
            match item {
                AaakSequenceItem::Verbatim { message } => messages.push(message.clone()),
                AaakSequenceItem::MotifRef {
                    pattern_key,
                    turn_index,
                } => {
                    let motif = motifs
                        .get(pattern_key)
                        .ok_or_else(|| format!("unknown aaak motif reference: {pattern_key}"))?;
                    messages.push(ConversationMessage {
                        role: motif.role.clone(),
                        content: motif.canonical_text.clone(),
                        turn_index: *turn_index,
                    });
                }
            }
        }

        messages.sort_by_key(|message| message.turn_index);
        Ok(ConversationTranscript {
            source_id: self.source_id.clone(),
            messages,
        })
    }
}

fn aaak_pattern_key(role: &ConversationRole, canonical_text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(format!("{role:?}:{canonical_text}").as_bytes());
    let digest = hasher.finalize();
    let hex = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("aaak-{}", &hex[..12])
}

fn aaak_confidence_percent(canonical_text: &str, occurrence_count: usize) -> u8 {
    let length_bonus = match canonical_text.trim().len() {
        0..=9 => 0,
        10..=23 => 5,
        24..=47 => 10,
        _ => 15,
    };
    let occurrence_bonus = match occurrence_count {
        0..=1 => 0,
        2 => 10,
        3 => 20,
        _ => 30,
    };

    (50 + length_bonus + occurrence_bonus).min(95) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_openai_export_into_messages() {
        let input = r#"[
          {"role":"system","content":"You are helpful"},
          {"role":"user","content":"Why did we switch auth vendors?"},
          {"role":"assistant","content":"Because refresh tokens were failing in staging"}
        ]"#;

        let transcript = ConversationTranscript::from_json_str(
            "auth-review",
            ConversationFormat::OpenAiMessages,
            input,
        )
        .expect("transcript should parse");

        assert_eq!(transcript.messages.len(), 3);
        assert_eq!(transcript.messages[1].role, ConversationRole::User);
        assert!(transcript.messages[2]
            .content
            .contains("refresh tokens were failing"));
    }

    #[test]
    fn maps_unknown_roles_to_unknown() {
        let input = r#"[
          {"role":"moderator","content":"Please stay on topic"}
        ]"#;

        let transcript = ConversationTranscript::from_json_str(
            "moderation-log",
            ConversationFormat::OpenAiMessages,
            input,
        )
        .expect("transcript should parse");

        assert_eq!(transcript.messages.len(), 1);
        assert_eq!(transcript.messages[0].role, ConversationRole::Unknown);
        assert_eq!(transcript.messages[0].content, "Please stay on topic");
    }

    #[test]
    fn normalizes_structured_content_into_text() {
        let input = r#"[
          {
            "role":"assistant",
            "content":[
              {"type":"text","text":"Hello"},
              {"type":"text","text":"world"}
            ]
          },
          {
            "role":"assistant",
            "content":{"type":"output_text","text":"Structured reply"}
          }
        ]"#;

        let transcript = ConversationTranscript::from_json_str(
            "structured-export",
            ConversationFormat::OpenAiMessages,
            input,
        )
        .expect("transcript should parse");

        assert_eq!(transcript.messages.len(), 2);
        assert_eq!(transcript.messages[0].content, "Hello world");
        assert_eq!(transcript.messages[1].content, "Structured reply");
    }

    #[test]
    fn aaak_lossless_parse_and_serialize_roundtrip() {
        let transcript = ConversationTranscript {
            source_id: "aaak-roundtrip".to_string(),
            messages: vec![
                ConversationMessage {
                    role: ConversationRole::Assistant,
                    content: "We should rotate the issuer config in staging.".to_string(),
                    turn_index: 0,
                },
                ConversationMessage {
                    role: ConversationRole::Assistant,
                    content: "We should rotate the issuer config in staging.".to_string(),
                    turn_index: 1,
                },
            ],
        };

        let compressed = transcript.compress_aaak(2, 60);
        let json = compressed
            .envelope
            .to_json_string()
            .expect("serialize should succeed");
        let decoded = AaakEnvelope::from_json_str(&json).expect("deserialize should succeed");
        let expanded = decoded.expand().expect("expand should succeed");

        assert_eq!(decoded, compressed.envelope);
        assert_eq!(expanded, transcript);
    }

    #[test]
    fn aaak_compression_is_deterministic_for_repeated_motifs() {
        let transcript = ConversationTranscript {
            source_id: "aaak-deterministic".to_string(),
            messages: vec![
                ConversationMessage {
                    role: ConversationRole::Assistant,
                    content: "Refresh tokens were failing in staging due to issuer drift."
                        .to_string(),
                    turn_index: 0,
                },
                ConversationMessage {
                    role: ConversationRole::User,
                    content: "What was the mitigation?".to_string(),
                    turn_index: 1,
                },
                ConversationMessage {
                    role: ConversationRole::Assistant,
                    content: "Refresh tokens were failing in staging due to issuer drift."
                        .to_string(),
                    turn_index: 2,
                },
            ],
        };

        let first = transcript.compress_aaak(2, 60);
        let second = transcript.compress_aaak(2, 60);

        assert!(!first.used_verbatim);
        assert_eq!(first, second);
        assert_eq!(first.patterns.len(), 1);
        assert_eq!(first.patterns[0].occurrence_count, 2);
    }

    #[test]
    fn aaak_falls_back_to_verbatim_when_confidence_is_low() {
        let transcript = ConversationTranscript {
            source_id: "aaak-low-confidence".to_string(),
            messages: vec![
                ConversationMessage {
                    role: ConversationRole::Assistant,
                    content: "ok".to_string(),
                    turn_index: 0,
                },
                ConversationMessage {
                    role: ConversationRole::Assistant,
                    content: "ok".to_string(),
                    turn_index: 1,
                },
            ],
        };

        let compressed = transcript.compress_aaak(2, 70);
        assert!(compressed.used_verbatim);
        assert!(compressed.patterns.is_empty());
        assert!(matches!(
            compressed.envelope.sequence[0],
            AaakSequenceItem::Verbatim { .. }
        ));
    }

    #[test]
    fn aaak_expand_rejects_unknown_motif_reference() {
        let envelope = AaakEnvelope {
            source_id: "aaak-bad-motif".to_string(),
            used_verbatim: false,
            motifs: vec![],
            sequence: vec![AaakSequenceItem::MotifRef {
                pattern_key: "aaak-missing".to_string(),
                turn_index: 0,
            }],
        };

        let err = envelope
            .expand()
            .expect_err("expand should fail when motif reference is missing");
        assert!(err.contains("unknown aaak motif reference"));
    }

    #[test]
    fn aaak_from_json_rejects_invalid_payload() {
        let err = AaakEnvelope::from_json_str("{not-json}")
            .expect_err("invalid json payload should fail");
        assert!(err.contains("invalid aaak envelope"));
    }
}
