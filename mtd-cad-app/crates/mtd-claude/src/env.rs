use std::collections::BTreeMap;

/// Guards applied to every child (pi-claude-bridge `CC_CHILD_ENV`). Set last, so they always hold.
const GUARDS: &[(&str, &str)] = &[
    // The user's claude.ai-connected MCP servers (Gmail, Drive...) stay out of the app.
    ("ENABLE_CLAUDEAI_MCP_SERVERS", "0"),
    // The app owns conversation state; Claude Code must not compact its own copy.
    ("DISABLE_AUTO_COMPACT", "1"),
    // No update checks / telemetry from the child.
    ("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1"),
];

/// Variables that describe a *parent* Claude Code session. Present when the app is launched from
/// inside Claude Code (e.g. during development); they would mislabel or hijack the child.
const PARENT_SESSION: &[&str] = &[
    "CLAUDECODE", "CLAUDE_CODE_CHILD_SESSION", "CLAUDE_CODE_SESSION_ID", "CLAUDE_CODE_SESSION_ATTENDED", "CLAUDE_PID",
    "CLAUDE_EFFORT", "TRACEPARENT", "CLAUDE_CODE_INVOKED_SKILLS", "CLAUDE_ENV_FILE", "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_AGENT_SDK_VERSION", "CLAUDE_CODE_EXECPATH", "CLAUDE_CODE_SSE_PORT", "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
];

/// Exported gateway/key variables redirect the CLI away from the user's login and every turn then
/// fails with that gateway's auth error (pi-claude-bridge issue #107). Removed unless the caller
/// explicitly sets them in `overrides`.
const AUTH_OVERRIDES: &[&str] = &["ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_BASE_URL"];

/// Build the child's environment. Pure, so it is unit-tested.
pub fn child_env(parent: impl IntoIterator<Item = (String, String)>, overrides: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    let mut env: BTreeMap<String, String> = parent.into_iter().collect();
    // Windows env names are case-insensitive; compare upper-cased.
    let has = |env: &BTreeMap<String, String>, k: &str| env.keys().any(|e| e.eq_ignore_ascii_case(k));
    if has(&env, "CLAUDECODE") {
        env.retain(|k, _| !PARENT_SESSION.iter().any(|p| p.eq_ignore_ascii_case(k)));
    }
    env.retain(|k, _| !AUTH_OVERRIDES.iter().any(|p| p.eq_ignore_ascii_case(k)));
    for (k, v) in overrides {
        if v.is_empty() {
            env.retain(|e, _| !e.eq_ignore_ascii_case(k));
        } else {
            env.insert(k.clone(), v.clone());
        }
    }
    for (k, v) in GUARDS {
        env.insert((*k).into(), (*v).into());
    }
    env
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parent(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn strips_gateway_overrides_so_login_is_used() {
        let e = child_env(parent(&[("ANTHROPIC_API_KEY", "sk"), ("anthropic_base_url", "http://x"), ("PATH", "p")]), &BTreeMap::new());
        assert!(!e.keys().any(|k| k.to_uppercase().starts_with("ANTHROPIC_")));
        assert_eq!(e["PATH"], "p");
    }

    #[test]
    fn explicit_override_wins_and_empty_removes() {
        let mut o = BTreeMap::new();
        o.insert("ANTHROPIC_BASE_URL".to_string(), "http://gw".to_string());
        o.insert("HTTPS_PROXY".to_string(), String::new());
        let e = child_env(parent(&[("HTTPS_PROXY", "p")]), &o);
        assert_eq!(e["ANTHROPIC_BASE_URL"], "http://gw");
        assert!(!e.contains_key("HTTPS_PROXY"));
    }

    #[test]
    fn drops_parent_session_markers_only_inside_claude_code() {
        let inside = child_env(parent(&[("CLAUDECODE", "1"), ("CLAUDE_CODE_SESSION_ID", "abc"), ("CLAUDE_CONFIG_DIR", "c")]), &BTreeMap::new());
        assert!(!inside.contains_key("CLAUDECODE") && !inside.contains_key("CLAUDE_CODE_SESSION_ID"));
        assert_eq!(inside["CLAUDE_CONFIG_DIR"], "c"); // config passes through
        let outside = child_env(parent(&[("CLAUDE_CODE_SESSION_ID", "abc")]), &BTreeMap::new());
        assert!(outside.contains_key("CLAUDE_CODE_SESSION_ID"));
    }

    #[test]
    fn guards_always_win() {
        let mut o = BTreeMap::new();
        o.insert("DISABLE_AUTO_COMPACT".to_string(), "0".to_string());
        assert_eq!(child_env(parent(&[]), &o)["DISABLE_AUTO_COMPACT"], "1");
    }
}
