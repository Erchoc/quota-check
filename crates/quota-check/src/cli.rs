use std::io::IsTerminal;
use std::path::PathBuf;

use anyhow::{bail, Result};
use clap::{Args, Parser, Subcommand};
use serde_json::json;

use quota_check_core::{auth, human, providers};

const PROVIDERS: &[&str] = &["codex", "claude", "kimi", "all"];

#[derive(Parser)]
#[command(
    name = "quota-check",
    version,
    about = "Check Coding Agent quota usage (5h / weekly windows) from the terminal",
    after_help = "Examples:\n  qc all              # every provider you are logged into\n  qc codex            # human-readable on a terminal, JSON when piped\n  qc codex --json     # force raw JSON\n  qc codex --whoami   # which account is this credential\n  qc claude           # refreshes an expired OAuth token automatically\n  qc kimi\n\nInstalled under two names: `quota-check` and the short alias `qc`."
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Check OpenAI Codex quota (reads ~/.codex/auth.json)
    Codex(CodexArgs),
    /// Check Claude Code subscription quota (OAuth, Pro/Max)
    Claude(ClaudeArgs),
    /// Check Kimi Code quota
    Kimi(KimiArgs),
    /// Check every provider that has usable local credentials
    All(AllArgs),
}

/// Output shape, shared by every subcommand.
#[derive(Args, Clone, Copy)]
struct OutputArgs {
    /// Human-readable output (the default when stdout is a terminal)
    #[arg(long)]
    human: bool,

    /// Raw JSON output (the default when stdout is piped or redirected)
    #[arg(long, conflicts_with = "human")]
    json: bool,
}

impl OutputArgs {
    fn is_human(&self) -> bool {
        if self.human {
            true
        } else if self.json {
            false
        } else {
            std::io::stdout().is_terminal()
        }
    }
}

#[derive(Args)]
struct CodexArgs {
    #[command(flatten)]
    out: OutputArgs,

    /// Credential file path (default $CODEX_HOME/auth.json or ~/.codex/auth.json)
    #[arg(long, value_name = "PATH")]
    auth: Option<PathBuf>,

    /// Only show which account this credential belongs to (no quota request)
    #[arg(long)]
    whoami: bool,
}

#[derive(Args)]
struct ClaudeArgs {
    #[command(flatten)]
    out: OutputArgs,

    /// OAuth token (sk-ant-oat...); overrides env/Keychain/credentials file
    #[arg(long, value_name = "TOKEN")]
    token: Option<String>,

    /// Do not exchange a stale refresh token for a fresh access token
    #[arg(long)]
    no_refresh: bool,
}

#[derive(Args)]
struct KimiArgs {
    #[command(flatten)]
    out: OutputArgs,

    /// API key (sk-...); overrides env/credential file discovery
    #[arg(long, value_name = "KEY")]
    key: Option<String>,

    /// API base URL (default https://api.kimi.com/coding/v1;
    /// CN subscriptions may need the moonshot.cn address)
    #[arg(long, value_name = "URL")]
    base: Option<String>,
}

#[derive(Args)]
struct AllArgs {
    #[command(flatten)]
    out: OutputArgs,
}

/// CLI entry point, shared by the `quota-check` and `qc` binaries.
pub fn run() {
    // Friendlier error for unknown providers than clap's default.
    if let Some(first) = std::env::args().nth(1) {
        if !first.starts_with('-') && !PROVIDERS.contains(&first.as_str()) {
            eprintln!(
                "\n  ✗ unknown provider '{first}'\n\n  supported providers: {}\n",
                PROVIDERS.join(", ")
            );
            std::process::exit(2);
        }
    }

    let cli = Cli::parse();
    let result = match cli.command {
        Commands::Codex(args) => run_codex(args),
        Commands::Claude(args) => run_claude(args),
        Commands::Kimi(args) => run_kimi(args),
        Commands::All(args) => run_all(args),
    };
    if let Err(e) = result {
        eprintln!("\n  ✗ {e:#}\n");
        std::process::exit(1);
    }
}

/// One provider's rendered result.
struct Report {
    title: &'static str,
    /// Raw provider response, as returned (what `--json` prints).
    raw: serde_json::Value,
    /// Reshaped document for the renderer, when the raw shape needs it.
    normalized: Option<serde_json::Value>,
    header: Vec<String>,
}

impl Report {
    fn render(&self, human_output: bool) -> String {
        if human_output {
            human::render(
                self.title,
                self.normalized.as_ref().unwrap_or(&self.raw),
                &self.header,
                std::io::stdout().is_terminal(),
            )
        } else {
            serde_json::to_string_pretty(&self.raw).unwrap_or_else(|e| e.to_string())
        }
    }
}

fn print_report(report: &Report, out: OutputArgs) {
    println!("{}", report.render(out.is_human()));
}

// ---------- providers ----------

fn codex_report(args: &CodexArgs) -> Result<Report> {
    let auth_path = args
        .auth
        .clone()
        .unwrap_or_else(auth::default_codex_auth_path);
    let cred = auth::load_auth(&auth_path)?;
    let ident = auth::identity(cred.id_token.as_deref());
    let data = providers::codex::fetch_usage(&cred)?;

    let mut header = vec![];
    if let Some(id) = &ident {
        let bits: Vec<&str> = [id.email.as_deref(), id.plan.as_deref()]
            .into_iter()
            .flatten()
            .collect();
        if !bits.is_empty() {
            header.push(bits.join("  ·  "));
        }
    }
    header.push(format!("creds  {}", auth_path.display()));

    Ok(Report {
        title: "Codex quota",
        raw: data,
        normalized: None,
        header,
    })
}

fn run_codex(args: CodexArgs) -> Result<()> {
    if args.whoami {
        let auth_path = args
            .auth
            .clone()
            .unwrap_or_else(auth::default_codex_auth_path);
        let cred = auth::load_auth(&auth_path)?;
        let ident = auth::identity(cred.id_token.as_deref());
        let payload = json!({
            "authPath": auth_path.display().to_string(),
            "accountId": cred.account_id,
            "email": ident.as_ref().and_then(|i| i.email.clone()),
            "plan": ident.as_ref().and_then(|i| i.plan.clone()),
            "expiresAt": ident.as_ref().and_then(|i| i.expires_at.clone()),
        });
        if args.out.is_human() {
            println!();
            for (k, v) in payload.as_object().unwrap() {
                println!("  {k:<12} {}", v.as_str().unwrap_or("-"));
            }
            println!();
        } else {
            println!("{}", serde_json::to_string_pretty(&payload)?);
        }
        return Ok(());
    }

    print_report(&codex_report(&args)?, args.out);
    Ok(())
}

fn claude_report(args: &ClaudeArgs) -> Result<Report> {
    let candidates = providers::claude::collect_candidates(args.token.as_deref());
    if candidates.is_empty() {
        bail!(
            "no Claude OAuth credentials found\n\n  scanned:\n  --token arg\n  env CLAUDE_CODE_OAUTH_TOKEN\n  macOS Keychain: Claude Code-credentials\n  {}\n\n  run `claude` to log in first, or pass --token",
            providers::claude::credentials_path().display()
        );
    }
    let hit = providers::claude::fetch_usage(&candidates, !args.no_refresh)?;

    let mut header = vec![
        format!("creds  {}", hit.candidate.source),
        format!("       {}", human::mask(&hit.candidate.token)),
    ];
    header.extend(hit.notes.iter().map(|n| format!("       {n}")));

    Ok(Report {
        title: "Claude Code quota",
        raw: hit.data,
        normalized: None,
        header,
    })
}

fn run_claude(args: ClaudeArgs) -> Result<()> {
    print_report(&claude_report(&args)?, args.out);
    Ok(())
}

fn kimi_report(args: &KimiArgs) -> Result<Report> {
    let base = args
        .base
        .clone()
        .unwrap_or_else(|| providers::kimi::DEFAULT_BASE.into())
        .trim_end_matches('/')
        .to_string();
    let candidates = providers::kimi::collect_candidates(args.key.as_deref());
    if candidates.is_empty() {
        bail!(
            "no Kimi credentials found\n\n  scanned:\n  KIMI_API_KEY / MOONSHOT_API_KEY / KIMI_CODE_API_KEY env vars\n  ~/.kimi-code/credentials/kimi-code.json\n  ~/.claude/settings.json and project-level .claude/settings*.json\n  ~/.pi/agent/auth.json, ~/.pi/providers/kimi-coding/config.json\n\n  specify directly: --key sk-xxx"
        );
    }
    let (data, hit) = providers::kimi::fetch_usage(&candidates, &base)?;
    let normalized = providers::kimi::normalize(&data);

    Ok(Report {
        title: "Kimi Code quota",
        raw: data,
        normalized: Some(normalized),
        header: vec![
            format!("creds  {}", hit.source),
            format!("       {}", human::mask(&hit.token)),
        ],
    })
}

fn run_kimi(args: KimiArgs) -> Result<()> {
    print_report(&kimi_report(&args)?, args.out);
    Ok(())
}

/// Every provider in one pass. A provider without usable credentials is
/// reported inline instead of aborting the run.
fn run_all(args: AllArgs) -> Result<()> {
    let out = args.out;
    let results: Vec<(&str, Result<Report>)> = vec![
        (
            "codex",
            codex_report(&CodexArgs {
                out,
                auth: None,
                whoami: false,
            }),
        ),
        (
            "claude",
            claude_report(&ClaudeArgs {
                out,
                token: None,
                no_refresh: false,
            }),
        ),
        (
            "kimi",
            kimi_report(&KimiArgs {
                out,
                key: None,
                base: None,
            }),
        ),
    ];
    let any_ok = results.iter().any(|(_, r)| r.is_ok());

    // Echo back whichever name the user typed (`quota-check` or `qc`).
    let bin = std::env::args()
        .next()
        .and_then(|p| {
            std::path::Path::new(&p)
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "quota-check".into());

    if out.is_human() {
        for (name, r) in &results {
            match r {
                Ok(report) => println!("{}", report.render(true)),
                Err(e) => {
                    // Keep the summary readable: one line per unavailable
                    // provider, details stay behind the dedicated subcommand.
                    let text = e.to_string();
                    let first = text
                        .lines()
                        .next()
                        .unwrap_or("unavailable")
                        .trim_end_matches([':', '.']);
                    println!("\n  {:<7} ✗ {first}", *name);
                    println!("  {:<7}   run `{bin} {name}` for the full diagnosis", "");
                }
            }
        }
        println!();
    } else {
        let mut doc = serde_json::Map::new();
        for (name, r) in &results {
            doc.insert(
                (*name).into(),
                match r {
                    Ok(report) => json!({"ok": true, "data": report.raw}),
                    Err(e) => json!({"ok": false, "error": format!("{e:#}")}),
                },
            );
        }
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::Value::Object(doc))?
        );
    }

    if !any_ok {
        bail!("no provider returned a quota (see the details above)");
    }
    Ok(())
}
