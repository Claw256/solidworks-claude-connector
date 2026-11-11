use crate::{ClaudeConfig, Event, build_args, child_env, parse_stream_line};
use anyhow::{Context, Result, bail};
use std::{collections::BTreeMap, path::PathBuf, process::Stdio};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::Command,
};

#[derive(Debug, Clone, Default)]
pub struct TurnResult {
    pub text: String,
    pub session: Option<String>,
    pub is_error: bool,
    pub cost_usd: Option<f64>,
    pub tools_used: Vec<String>,
}

/// Locate the `claude` binary: explicit path, else PATH (`claude.exe`, `claude.cmd`, `claude`).
pub fn find_claude(explicit: Option<&PathBuf>) -> Option<PathBuf> {
    if let Some(p) = explicit {
        return p.exists().then(|| p.clone());
    }
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        for name in ["claude.exe", "claude.cmd", "claude"] {
            let c = dir.join(name);
            if c.is_file() {
                return Some(c);
            }
        }
    }
    None
}

/// Run one turn. `on_event` is called for every parsed event as it streams in.
pub async fn run_turn(cfg: &ClaudeConfig, prompt: &str, resume: Option<&str>, mut on_event: impl FnMut(&Event)) -> Result<TurnResult> {
    let exe = find_claude(cfg.claude_path.as_ref()).context("`claude` not found: install Claude Code and run `claude auth login`")?;
    let env: BTreeMap<String, String> = child_env(std::env::vars(), &cfg.env_overrides);

    let mut cmd = Command::new(&exe);
    cmd.args(build_args(cfg, resume))
        .env_clear()
        .envs(&env)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(cwd) = &cfg.cwd {
        std::fs::create_dir_all(cwd).ok();
        cmd.current_dir(cwd);
    }
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW: no console flash from a GUI app

    let mut child = cmd.spawn().with_context(|| format!("spawn {}", exe.display()))?;
    let mut stdin = child.stdin.take().context("stdin")?;
    stdin.write_all(prompt.as_bytes()).await?;
    drop(stdin); // EOF = end of prompt

    let mut stderr = child.stderr.take().context("stderr")?;
    let err_task = tokio::spawn(async move {
        let mut s = String::new();
        let _ = stderr.read_to_string(&mut s).await;
        s
    });

    let mut res = TurnResult::default();
    let mut streamed = String::new();
    let mut lines = BufReader::new(child.stdout.take().context("stdout")?).lines();
    while let Some(line) = lines.next_line().await? {
        for ev in parse_stream_line(&line) {
            match &ev {
                Event::Session(s) => res.session = Some(s.clone()),
                Event::Text(t) => streamed.push_str(t),
                Event::ToolUse { name, .. } => res.tools_used.push(name.clone()),
                Event::Done { text, is_error, session, cost_usd } => {
                    res.text = text.clone();
                    res.is_error = *is_error;
                    res.cost_usd = *cost_usd;
                    if session.is_some() {
                        res.session = session.clone();
                    }
                }
                Event::ToolResult { .. } => {}
            }
            on_event(&ev);
        }
    }
    let status = child.wait().await?;
    let stderr = err_task.await.unwrap_or_default();
    if res.text.is_empty() {
        res.text = streamed;
    }
    if !status.success() && res.text.is_empty() {
        bail!("claude exited with {status}: {}", stderr.trim());
    }
    Ok(res)
}
