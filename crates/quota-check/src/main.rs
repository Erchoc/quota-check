use std::io::IsTerminal;
use std::path::PathBuf;

use anyhow::Result;
use clap::{Args, Parser, Subcommand};

use quota_check_core::{auth, human, providers};

#[derive(Parser)]
#[command(
    name = "quota-check",
    version,
    about = "查看 Coding Agent 的小时 / 周额度用量（Codex，更多 provider 在路上）",
    after_help = "示例：\n  quota-check codex            # 原始 JSON\n  quota-check codex --human    # 人类可读\n  quota-check codex --whoami   # 这份凭据是谁"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// 查询 OpenAI Codex 额度（读本地 ~/.codex/auth.json）
    Codex(CodexArgs),
}

#[derive(Args)]
struct CodexArgs {
    /// 输出人类可读格式（默认输出原始 JSON）
    #[arg(long)]
    human: bool,

    /// 指定凭据文件路径（默认 $CODEX_HOME/auth.json 或 ~/.codex/auth.json）
    #[arg(long, value_name = "PATH")]
    auth: Option<PathBuf>,

    /// 只看这份凭据属于哪个账号，不请求额度接口
    #[arg(long)]
    whoami: bool,
}

fn main() {
    let cli = Cli::parse();
    let result = match cli.command {
        Commands::Codex(args) => run_codex(args),
    };
    if let Err(e) = result {
        eprintln!("\n  ✗ {e:#}\n");
        std::process::exit(1);
    }
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
                let v = v.as_str().unwrap_or("-");
                println!("  {k:<12} {v}");
            }
            println!();
        } else {
            println!("{}", serde_json::to_string_pretty(&payload)?);
        }
        return Ok(());
    }

    let data = providers::codex::fetch_usage(&cred)?;

    if args.human {
        println!(
            "{}",
            human::render(&data, ident.as_ref(), &auth_path.display().to_string(), tty)
        );
    } else {
        println!("{}", serde_json::to_string_pretty(&data)?);
    }
    Ok(())
}
