use serde_json::json;
use std::{collections::BTreeMap, path::PathBuf};

#[derive(Debug, Clone)]
pub struct McpServerSpec {
    /// Server name; tools surface to Claude as `mcp__<name>__<tool>`.
    pub name: String,
    pub command: PathBuf,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default)]
pub struct ClaudeConfig {
    /// Path to the `claude` binary; `None` = search PATH.
    pub claude_path: Option<PathBuf>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub mcp_servers: Vec<McpServerSpec>,
    /// Appended to Claude Code's system prompt (the app's own guidance).
    pub append_system_prompt: Option<String>,
    /// Working directory for the child. Use a neutral, empty directory so no project CLAUDE.md or
    /// `.claude/settings.json` is discovered.
    pub cwd: Option<PathBuf>,
    /// Extra environment (empty value removes the variable).
    pub env_overrides: BTreeMap<String, String>,
}

pub fn mcp_config_json(servers: &[McpServerSpec]) -> String {
    let mut m = serde_json::Map::new();
    for s in servers {
        m.insert(
            s.name.clone(),
            json!({ "type": "stdio", "command": s.command.to_string_lossy(), "args": s.args, "env": s.env }),
        );
    }
    json!({ "mcpServers": m }).to_string()
}

/// CLI arguments for one print-mode turn. The prompt itself goes on stdin (no quoting or length
/// limits on Windows). `resume` continues an earlier session id.
pub fn build_args(cfg: &ClaudeConfig, resume: Option<&str>) -> Vec<String> {
    let mut a: Vec<String> = vec![
        "--print".into(),
        "--output-format".into(),
        "stream-json".into(),
        "--verbose".into(),
        "--include-partial-messages".into(),
        // Only our MCP servers: not ~/.claude.json, .mcp.json or claude.ai connectors.
        "--strict-mcp-config".into(),
        "--mcp-config".into(),
        mcp_config_json(&cfg.mcp_servers),
        // No built-in tools (Bash/Edit/...): the app exposes exactly what it chooses over MCP.
        "--tools".into(),
        String::new(),
        // Nothing from user/project/local settings (hooks, apiKeyHelper, env, plugins).
        "--setting-sources".into(),
        String::new(),
    ];
    for s in &cfg.mcp_servers {
        a.push("--allowedTools".into());
        a.push(format!("mcp__{}", s.name));
    }
    if let Some(m) = &cfg.model {
        a.extend(["--model".into(), m.clone()]);
    }
    if let Some(e) = &cfg.effort {
        a.extend(["--effort".into(), e.clone()]);
    }
    if let Some(p) = &cfg.append_system_prompt {
        a.extend(["--append-system-prompt".into(), p.clone()]);
    }
    if let Some(r) = resume {
        a.extend(["--resume".into(), r.to_string()]);
    }
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> ClaudeConfig {
        ClaudeConfig {
            mcp_servers: vec![McpServerSpec { name: "mtd".into(), command: "C:/x/mtd-mcp.exe".into(), args: vec!["--db".into(), "C:/m.db".into()], env: Default::default() }],
            model: Some("claude-sonnet-5-5".into()),
            ..Default::default()
        }
    }

    #[test]
    fn isolation_flags_present() {
        let a = build_args(&cfg(), None);
        let pos = |f: &str| a.iter().position(|x| x == f).unwrap_or_else(|| panic!("missing {f}"));
        assert!(a.contains(&"--strict-mcp-config".to_string()));
        assert_eq!(a[pos("--tools") + 1], "");
        assert_eq!(a[pos("--setting-sources") + 1], "");
        assert_eq!(a[pos("--allowedTools") + 1], "mcp__mtd");
        assert_eq!(a[pos("--model") + 1], "claude-sonnet-5-5");
        assert!(!a.contains(&"--resume".to_string()));
    }

    #[test]
    fn resume_and_config_json() {
        let a = build_args(&cfg(), Some("abc-123"));
        assert!(a.windows(2).any(|w| w == ["--resume", "abc-123"]));
        let j: serde_json::Value = serde_json::from_str(&mcp_config_json(&cfg().mcp_servers)).unwrap();
        assert_eq!(j["mcpServers"]["mtd"]["type"], "stdio");
        assert_eq!(j["mcpServers"]["mtd"]["args"][1], "C:/m.db");
    }
}
