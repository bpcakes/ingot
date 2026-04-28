use ingot_agent_protocol::response::{
    AgentOutputChannel, AgentOutputKind, AgentOutputSegmentDraft, AgentOutputStatus,
};

#[derive(Clone, Copy)]
pub(crate) struct MessageTextConfig<'a> {
    pub(crate) array_part_fields: &'a [&'a str],
    pub(crate) object_text_field: Option<&'a str>,
    pub(crate) object_content_field: Option<&'a str>,
}

pub(crate) fn message_text(
    value: &serde_json::Value,
    config: MessageTextConfig<'_>,
) -> Option<String> {
    match value {
        serde_json::Value::String(text) => Some(text.to_owned()),
        serde_json::Value::Array(parts) => {
            let joined = parts
                .iter()
                .filter_map(|part| {
                    config
                        .array_part_fields
                        .iter()
                        .find_map(|field| part.get(*field).and_then(|value| value.as_str()))
                })
                .collect::<Vec<_>>()
                .join("\n");
            if joined.is_empty() {
                None
            } else {
                Some(joined)
            }
        }
        serde_json::Value::Object(_) => {
            if let Some(field) = config.object_text_field {
                if let Some(text) = value.get(field).and_then(|value| value.as_str()) {
                    return Some(text.to_owned());
                }
            }

            config
                .object_content_field
                .and_then(|field| value.get(field))
                .and_then(|value| message_text(value, config))
        }
        _ => None,
    }
}

pub(crate) fn lifecycle_segment(
    title: impl Into<String>,
    text: Option<String>,
    status: Option<AgentOutputStatus>,
    data: serde_json::Value,
) -> AgentOutputSegmentDraft {
    AgentOutputSegmentDraft {
        channel: AgentOutputChannel::Diagnostic,
        kind: AgentOutputKind::Lifecycle,
        status,
        title: Some(title.into()),
        text,
        data: Some(data),
    }
}

pub(crate) fn provider_raw_fallback(
    provider: &str,
    event_type: &str,
    raw: serde_json::Value,
    text: Option<String>,
) -> AgentOutputSegmentDraft {
    AgentOutputSegmentDraft {
        channel: AgentOutputChannel::Diagnostic,
        kind: AgentOutputKind::RawFallback,
        status: Some(AgentOutputStatus::Unknown),
        title: Some("Provider event".into()),
        text,
        data: Some(serde_json::json!({
            "provider": provider,
            "provider_event_type": event_type,
            "raw": raw
        })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_text_extracts_string_values() {
        let text = message_text(
            &serde_json::json!("hello"),
            MessageTextConfig {
                array_part_fields: &["text"],
                object_text_field: None,
                object_content_field: None,
            },
        );

        assert_eq!(text.as_deref(), Some("hello"));
    }

    #[test]
    fn message_text_joins_configured_array_fields() {
        let text = message_text(
            &serde_json::json!([
                {"type": "text", "text": "first"},
                {"type": "other", "content": "second"},
                {"type": "image", "url": "ignored"}
            ]),
            MessageTextConfig {
                array_part_fields: &["text", "content"],
                object_text_field: None,
                object_content_field: None,
            },
        );

        assert_eq!(text.as_deref(), Some("first\nsecond"));
    }

    #[test]
    fn message_text_recurses_through_configured_content_field() {
        let text = message_text(
            &serde_json::json!({
                "role": "assistant",
                "content": [{"type": "text", "text": "nested"}]
            }),
            MessageTextConfig {
                array_part_fields: &["text"],
                object_text_field: Some("text"),
                object_content_field: Some("content"),
            },
        );

        assert_eq!(text.as_deref(), Some("nested"));
    }

    #[test]
    fn provider_raw_fallback_records_provider_event_and_raw_payload() {
        let segment = provider_raw_fallback(
            "codex",
            "mystery",
            serde_json::json!({"type": "mystery"}),
            Some("unknown".into()),
        );

        assert_eq!(segment.kind, AgentOutputKind::RawFallback);
        assert_eq!(segment.status, Some(AgentOutputStatus::Unknown));
        let data = segment.data.as_ref().expect("raw fallback data");
        assert_eq!(data["provider"], "codex");
        assert_eq!(data["provider_event_type"], "mystery");
    }
}
