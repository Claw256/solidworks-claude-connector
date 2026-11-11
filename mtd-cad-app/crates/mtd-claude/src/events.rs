use serde_json::Value;

/// What the UI cares about from Claude Code's `stream-json` output.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Session(String),
    /// Streaming text delta.
    Text(String),
    ToolUse { name: String, input: String },
    ToolResult { is_error: bool, text: String },
    /// Final result of the turn.
    Done { text: String, is_error: bool, session: Option<String>, cost_usd: Option<f64> },
}

fn tool_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(items) => items.iter().filter_map(|i| i.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join("\n"),
        _ => String::new(),
    }
}

/// Parse one line. Unknown or malformed lines yield no events (the stream format grows over time).
pub fn parse_stream_line(line: &str) -> Vec<Event> {
    let Ok(v) = serde_json::from_str::<Value>(line) else { return vec![] };
    let ty = v.get("type").and_then(Value::as_str).unwrap_or("");
    match ty {
        "system" if v.get("subtype").and_then(Value::as_str) == Some("init") => {
            v.get("session_id").and_then(Value::as_str).map(|s| vec![Event::Session(s.to_string())]).unwrap_or_default()
        }
        "stream_event" => {
            let d = &v["event"]["delta"];
            if d.get("type").and_then(Value::as_str) == Some("text_delta") {
                d.get("text").and_then(Value::as_str).map(|t| vec![Event::Text(t.to_string())]).unwrap_or_default()
            } else {
                vec![]
            }
        }
        "assistant" => v["message"]["content"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|b| b.get("type").and_then(Value::as_str) == Some("tool_use"))
            .map(|b| Event::ToolUse {
                name: b.get("name").and_then(Value::as_str).unwrap_or("?").to_string(),
                input: b.get("input").map(|i| i.to_string()).unwrap_or_default(),
            })
            .collect(),
        "user" => v["message"]["content"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|b| b.get("type").and_then(Value::as_str) == Some("tool_result"))
            .map(|b| Event::ToolResult { is_error: b.get("is_error").and_then(Value::as_bool).unwrap_or(false), text: tool_text(&b["content"]) })
            .collect(),
        "result" => vec![Event::Done {
            text: v.get("result").and_then(Value::as_str).unwrap_or("").to_string(),
            is_error: v.get("is_error").and_then(Value::as_bool).unwrap_or(false) || v.get("subtype").and_then(Value::as_str).is_some_and(|s| s != "success"),
            session: v.get("session_id").and_then(Value::as_str).map(str::to_string),
            cost_usd: v.get("total_cost_usd").and_then(Value::as_f64),
        }],
        _ => vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_stream() {
        assert_eq!(parse_stream_line(r#"{"type":"system","subtype":"init","session_id":"s1"}"#), vec![Event::Session("s1".into())]);
        assert_eq!(
            parse_stream_line(r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"Hi"}}}"#),
            vec![Event::Text("Hi".into())]
        );
        let tu = parse_stream_line(r#"{"type":"assistant","message":{"content":[{"type":"text","text":"x"},{"type":"tool_use","name":"mcp__mtd__get_design","input":{"design":"J1"}}]}}"#);
        assert_eq!(tu, vec![Event::ToolUse { name: "mcp__mtd__get_design".into(), input: r#"{"design":"J1"}"#.into() }]);
        let tr = parse_stream_line(r#"{"type":"user","message":{"content":[{"type":"tool_result","content":[{"type":"text","text":"card"}],"is_error":false}]}}"#);
        assert_eq!(tr, vec![Event::ToolResult { is_error: false, text: "card".into() }]);
        match &parse_stream_line(r#"{"type":"result","subtype":"success","is_error":false,"result":"done","session_id":"s1","total_cost_usd":0.01}"#)[0] {
            Event::Done { text, is_error, session, .. } => assert!(text == "done" && !is_error && session.as_deref() == Some("s1")),
            e => panic!("{e:?}"),
        }
    }

    #[test]
    fn tolerates_noise() {
        assert!(parse_stream_line("not json").is_empty());
        assert!(parse_stream_line(r#"{"type":"rate_limit_event"}"#).is_empty());
        match &parse_stream_line(r#"{"type":"result","subtype":"error_max_turns"}"#)[0] {
            Event::Done { is_error, .. } => assert!(is_error),
            e => panic!("{e:?}"),
        }
    }
}
