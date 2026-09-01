use std::io::IsTerminal;
use std::path::PathBuf;

use anyhow::{bail, Result};
use clap::{Args, Parser, Subcommand};

use quota_check_core::{auth, human, providers};

const PROVIDERS: &[&str] = &["codex", "claude", "kimi"];

#[derive(Parser)]
#[command(
    name = "quota-check",
    version,
    about = "Check Coding Agent quota usage (5h / weekly windows) from the terminal",
    after_help = "Examples:\n  quota-check codex            # raw JSON\n  quota-check codex --human    # human-readable\n  quota-check codex --whoami   # which account is this credential\n  quota-check claude --human\n  quota-check kimi --human"
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
}

#[derive(Args)]
struct CodexArgs {
    /// Human-readable output (default is raw JSON)
    #[arg(long)]
    human: bool,

    /// Credential file path (default $CODEX_HOME/auth.json or ~/.codex/auth.json)
    #[arg(long, value_name = "PATH")]
    auth: Option<PathBuf>,

    /// Only show which account this credential belongs to (no quota request)
    #[arg(long)]
    whoami: bool,
}

#[derive(Args)]
struct ClaudeArgs {
    /// Human-readable output (default is raw JSON)
    #[arg(long)]
    human: bool,

    /// OAuth token (sk-ant-oat...); overrides env/Keychain/credentials file
    #[arg(long, value_name = "TOKEN")]
    token: Option<String>,
}

#[derive(Args)]
struct KimiArgs {
    /// Human-readable output (default is raw JSON)
    #[arg(long)]
    human: bool,

    /// API key (sk-...); overrides env/credential file discovery
    #[arg(long, value_name = "KEY")]
    key: Option<String>,

    /// API base URL (default https://api.kimi.com/coding/v1;
    /// CN subscriptions may need the moonshot.cn address)
    #[arg(long, value_name = "URL")]
    base: Option<String>,
}

fn main() {
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
    };
    if let Err(e) = result {
        eprintln!("\n  ✗ {e:#}\n");
        std::process::exit(1);
    }
}

fn print_result(
    title: &str,
    data: &serde_json::Value,
    normalized: Option<&serde_json::Value>,
    header: &[String],
    human_flag: bool,
) -> Result<()> {
    if human_flag {
        let tty = std::io::stdout().is_terminal();
        println!(
            "{}",
            human::render(title, normalized.unwrap_or(data), header, tty)
        );
    } else {
        println!("{}", serde_json::to_string_pretty(data)?);
    }
    Ok(())
}

fn run_codex(args: CodexArgs) -> Result<()> {
    let auth_path = args.auth.unwrap_or_else(auth::default_codex_auth_path);
    let cred = auth::load_auth(&auth_path)?;
    let ident = auth::identity(cred.id_token.as_deref());
    let tty = std::io::stdout().is_terminal();

    if args.whoami {
        let payload = serde_json::json!({
            "authPath": auth_path.display().to_string(),
            "accountId": cred.account_id,
            "email": ident.as_ref().and_then(|i| i.email.clone()),
            "plan": ident.as_ref().and_then(|i| i.plan.clone()),
            "expiresAt": ident.as_ref().and_then(|i| i.expires_at.clone()),
        });
        if args.human {
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
    let _ = tty;

    print_result("Codex quota", &data, None, &header, args.human)
}

fn run_claude(args: ClaudeArgs) -> Result<()> {
    let candidates = providers::claude::collect_candidates(args.token.as_deref());
    if candidates.is_empty() {
        bail!(
            "no Claude OAuth credentials found. scanned:\n  --token arg\n  env CLAUDE_CODE_OAUTH_TOKEN\n  macOS Keychain: Claude Code-credentials\n  {}\n\n  run `claude` to log in first, or pass --token",
            providers::claude::credentials_path().display()
        );
    }
    let (data, hit) = providers::claude::fetch_usage(&candidates)?;
    let header = vec![
        format!("creds  {}", hit.source),
        format!("       {}", human::mask(&hit.token)),
    ];
    print_result("Claude Code quota", &data, None, &header, args.human)
}

fn run_kimi(args: KimiArgs) -> Result<()> {
    let base = args
        .base
        .unwrap_or_else(|| providers::kimi::DEFAULT_BASE.into())
        .trim_end_matches('/')
        .to_string();
    let candidates = providers::kimi::collect_candidates(args.key.as_deref());
    if candidates.is_empty() {
        bail!(
            "no credential candidates found. scanned:\n  KIMI_API_KEY / MOONSHOT_API_KEY / KIMI_CODE_API_KEY env vars\n  ~/.kimi-code/credentials/kimi-code.json\n  ~/.claude/settings.json and project-level .claude/settings*.json\n  ~/.pi/agent/auth.json, ~/.pi/providers/kimi-coding/config.json\n\n  specify directly: --key sk-xxx"
        );
    }
    let (data, hit) = providers::kimi::fetch_usage(&candidates, &base)?;
    let normalized = providers::kimi::normalize(&data);
    let header = vec![
        format!("creds  {}", hit.source),
        format!("       {}", human::mask(&hit.token)),
    ];
    print_result("Kimi Code quota", &data, Some(&normalized), &header, args.human)
}
