use anyhow::{Result, bail};
use clap::{Parser, Subcommand};
use mtd_index::{SearchOpts, Store, paths::default_db_path, scan};
use mtd_sw::{ModelAnalysis, SolidWorksBackend};
use std::path::{Path, PathBuf};

/// MTD CAD memory: index a project library and query it the way Claude will.
#[derive(Parser)]
#[command(name = "mtd", version)]
struct Cli {
    /// SQLite store (default: %LOCALAPPDATA%\MTD CAD\memory.db, or $MTD_DB)
    #[arg(long, global = true)]
    db: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Scan a folder (library or one family such as `Trolleys`) into the index.
    Index { root: PathBuf, #[arg(long)] family: Option<String> },
    /// Full-text search.
    Search { query: String, #[arg(long)] job: Option<String>, #[arg(long)] kind: Option<String>, #[arg(long)] no_attic: bool, #[arg(long, default_value_t = 10)] limit: usize },
    /// Print a design card (e.g. J007097-00C-00) or a job card (e.g. J007097).
    Card { key: String },
    /// Numeric fact lookup across all designs: `mtd facts "sheet.%weight%" --min 100`.
    Facts { key_like: String, #[arg(long)] min: Option<f64>, #[arg(long)] max: Option<f64>, #[arg(long, default_value_t = 50)] limit: i64 },
    /// List jobs, or designs of one job.
    Ls { job: Option<String> },
    Stats,
    /// Open a .sldprt/.sldasm in SolidWorks, analyze it and write `<dir>/.solidpilot/<file>.analysis.json`
    /// (picked up by the next `mtd index`).
    Analyze {
        file: PathBuf,
        /// Use canned data instead of SolidWorks (testing).
        #[arg(long)]
        mock: bool,
    },
    /// Ask Claude (via your `claude` login) a question; it answers from the index through MCP.
    Ask { question: String, #[arg(long)] model: Option<String> },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    if let Cmd::Analyze { file, mock } = &cli.cmd {
        let (path, analysis) = if *mock {
            analyze_to_artifact(&mut mtd_sw::MockBackend::new(), file)?
        } else {
            #[cfg(windows)]
            {
                analyze_to_artifact(&mut mtd_sw::ComBackend::new(), file)?
            }
            #[cfg(not(windows))]
            {
                bail!("SolidWorks analysis needs Windows; use --mock for canned data")
            }
        };
        print_summary(&path, &analysis);
        return Ok(());
    }
    let db = cli.db.unwrap_or_else(default_db_path);
    let mut store = Store::open(&db)?;
    match cli.cmd {
        Cmd::Index { root, family } => {
            let rep = scan(&mut store, &root, family.as_deref())?;
            println!("{}", serde_json::to_string_pretty(&rep)?);
        }
        Cmd::Search { query, job, kind, no_attic, limit } => {
            let hits = store.search(&query, &SearchOpts { job, kind, include_attic: !no_attic, limit })?;
            for h in &hits {
                println!(
                    "{:>6.2}  {}{}  [{}] {}\n        {}",
                    h.score,
                    h.design_key.as_deref().unwrap_or("-"),
                    if h.superseded { " (superseded)" } else { "" },
                    h.locator,
                    h.rel_path,
                    h.snippet.replace('\n', " ⏎ ")
                );
            }
            if hits.is_empty() {
                println!("no results");
            }
        }
        Cmd::Card { key } => match store.design_card(&key)?.or(store.job_card(&key)?) {
            Some(c) => println!("{c}"),
            None => bail!("no design or job '{key}'"),
        },
        Cmd::Facts { key_like, min, max, limit } => {
            for f in store.find_facts(&key_like, min, max, limit)? {
                println!(
                    "{:<16} {:<44} {} {}  [{} {}]",
                    f.design_key.or(f.job_no).unwrap_or_default(),
                    f.key,
                    f.num.map(|n| n.to_string()).or(f.text).unwrap_or_default(),
                    f.unit.unwrap_or_default(),
                    f.source,
                    f.locator.unwrap_or_default()
                );
            }
        }
        Cmd::Ls { job } => match job {
            None => {
                for j in store.jobs()? {
                    println!("{}  {}  ({} files, {} designs)", j.job_no, j.name.unwrap_or_default(), j.files, j.designs);
                }
            }
            Some(j) => {
                for d in store.designs(Some(&j))? {
                    println!("{:<44} {:<11} {}", d.key, d.status, d.title.unwrap_or_default());
                }
            }
        },
        Cmd::Analyze { .. } => unreachable!("handled above"),
        Cmd::Stats => println!("{}", serde_json::to_string_pretty(&store.stats()?)?),
        Cmd::Ask { question, model } => {
            use mtd_claude::{ClaudeConfig, Event, McpServerSpec, run_turn};
            let mcp = std::env::current_exe()?.with_file_name(if cfg!(windows) { "mtd-mcp.exe" } else { "mtd-mcp" });
            if !mcp.exists() {
                bail!("{} not found: run `cargo build -p mtd-mcp` first", mcp.display());
            }
            let cfg = ClaudeConfig {
                model,
                mcp_servers: vec![McpServerSpec { name: "mtd".into(), command: mcp, args: vec!["--db".into(), db.to_string_lossy().into_owned()], env: Default::default() }],
                append_system_prompt: Some("You are the MTD CAD assistant. Answer only from the MTD memory tools; cite design keys and sources.".into()),
                cwd: Some(std::env::temp_dir().join("mtd-claude-cwd")),
                ..Default::default()
            };
            drop(store); // the MCP child opens its own connection
            let r = run_turn(&cfg, &question, None, |e| match e {
                Event::ToolUse { name, input } => eprintln!("  [tool] {name} {input}"),
                Event::ToolResult { is_error, text } => eprintln!("  [result{}] {} chars", if *is_error { " ERROR" } else { "" }, text.len()),
                _ => {}
            })
            .await?;
            println!("{}", r.text);
            eprintln!("\n(session {:?}, cost ${:?}, tools: {:?})", r.session, r.cost_usd, r.tools_used);
        }
    }
    Ok(())
}

fn sha256_hex(p: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut f = std::fs::File::open(p)?;
    let (mut h, mut buf) = (Sha256::new(), vec![0u8; 1 << 16]);
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(format!("{:x}", h.finalize()))
}

/// Connect, open, analyze, close; write `<dir>/.solidpilot/<file name>.analysis.json` keyed by the file's SHA-256.
fn analyze_to_artifact(sw: &mut dyn SolidWorksBackend, file: &Path) -> Result<(PathBuf, ModelAnalysis)> {
    let file = std::fs::canonicalize(file).map_err(|e| anyhow::anyhow!("{}: {e}", file.display()))?;
    // canonicalize yields verbatim (\?\) paths on Windows, which SolidWorks dislikes.
    let sw_path = PathBuf::from(file.to_string_lossy().trim_start_matches(r"\?\").to_string());
    let ext = sw_path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    if ext != "sldprt" && ext != "sldasm" {
        bail!("expected a .sldprt or .sldasm file, got {}", sw_path.display());
    }
    let hash = sha256_hex(&sw_path)?; // hash before SolidWorks can touch the file
    sw.connect()?;
    sw.open_document(&sw_path)?;
    let result = sw.analyze_model(&sw_path);
    let _ = sw.close_document(&sw_path); // always release, even if analysis failed
    let analysis = result?;
    let dir = sw_path.parent().ok_or_else(|| anyhow::anyhow!("no parent dir"))?.join(".solidpilot");
    std::fs::create_dir_all(&dir)?;
    let out = dir.join(format!("{}.analysis.json", sw_path.file_name().unwrap().to_string_lossy()));
    std::fs::write(&out, serde_json::to_string_pretty(&mtd_sw::to_artifact_json(&analysis, &hash))?)?;
    Ok((out, analysis))
}

fn print_summary(out: &Path, a: &ModelAnalysis) {
    println!("wrote {}", out.display());
    println!("  type: {:?}, {} features, {} dimensions, {} custom properties", a.document_type, a.features.len(), a.dimensions.len(), a.custom_properties.len());
    if let Some(b) = &a.bounding_box {
        let s = b.size();
        println!("  bbox: {:.2} x {:.2} x {:.2} mm", s[0] * 1e3, s[1] * 1e3, s[2] * 1e3);
    }
    if let Some(m) = &a.mass {
        println!("  mass: {:.4} kg, volume {:.1} mm3", m.mass_kg, m.volume_m3 * 1e9);
    }
    if let Some(m) = &a.material {
        println!("  material: {m}");
    }
    println!("run `mtd index <folder>` to ingest it");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_analyze_writes_artifact_that_index_ingests() {
        let dir = std::env::temp_dir().join(format!("mtd-analyze-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let part = dir.join("J007097-00C-00.SLDPRT");
        std::fs::write(&part, b"fake sldprt bytes").unwrap();

        let (out, a) = analyze_to_artifact(&mut mtd_sw::MockBackend::new(), &part).unwrap();
        assert_eq!(out.file_name().unwrap(), "J007097-00C-00.SLDPRT.analysis.json");
        assert_eq!(out.parent().unwrap().file_name().unwrap(), ".solidpilot");
        assert_eq!(a.features.len(), 2);

        let mut store = Store::open(&dir.join("test.db")).unwrap();
        let rep = scan(&mut store, &dir, None).unwrap();
        assert_eq!(rep.pending_solidworks, 0, "artifact should satisfy the pending file: {rep:?}");
        assert!(rep.with_sw_artifact >= 1, "{rep:?}");
        assert!(store.find_facts("geometry.bbox_x_mm", None, None, 5).unwrap().iter().any(|f| (f.num.unwrap() - 26.0).abs() < 1e-6));

        // stale after the source changes
        std::fs::write(&part, b"changed").unwrap();
        let rep = scan(&mut store, &dir, None).unwrap();
        assert_eq!(rep.pending_solidworks, 1, "{rep:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_non_solidworks_files() {
        let dir = std::env::temp_dir().join(format!("mtd-analyze-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("x.pdf");
        std::fs::write(&f, b"x").unwrap();
        assert!(analyze_to_artifact(&mut mtd_sw::MockBackend::new(), &f).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
