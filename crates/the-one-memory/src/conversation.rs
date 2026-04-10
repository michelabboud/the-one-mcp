use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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
}
