use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

mod commands;
mod config;

// Release tag embedded by CI (e.g. "0.1.0-15"). Falls back to the Cargo.toml
// version for local/dev builds. Kept at module scope so clap can use it as the
// `version` attribute.
pub(crate) const CTX_VERSION: &str = match option_env!("CTX_RELEASE_VERSION") {
    Some(s) => trim_leading_v(s),
    None => env!("CARGO_PKG_VERSION"),
};

const fn trim_leading_v(s: &str) -> &str {
    match s.as_bytes() {
        [b'v', ..] => match s.split_at_checked(1) {
            Some((_, rest)) => rest,
            None => s,
        },
        _ => s,
    }
}

#[derive(Parser)]
#[command(
    name = "ctx",
    version = CTX_VERSION,
    about = "Local context engine for TS/JS/CSS/HTML"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
    #[arg(long, global = true, default_value = "info")]
    log: String,
}

#[derive(Subcommand)]
enum Cmd {
    /// Initialize per-repo state (~/.ctx/repos/<hash>/).
    Init {
        #[arg(default_value = ".")]
        path: PathBuf,
    },
    /// Full (re)index of the repo and exit.
    Index {
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Force a full re-index (placeholder — same as default in Phase 1).
        #[arg(long)]
        full: bool,
    },
    /// Start the watcher + MCP stdio server.
    Serve {
        #[arg(default_value = ".")]
        path: PathBuf,
    },
    /// Print index health.
    Status {
        #[arg(default_value = ".")]
        path: PathBuf,
    },
    /// Update ctx to the latest published release.
    Update {
        /// Skip confirmation prompt.
        #[arg(long)]
        force: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    init_tracing(&cli.log);
    match cli.cmd {
        Cmd::Init { path } => commands::init::run(&path).await,
        Cmd::Index { path, full: _ } => commands::index::run(&path).await,
        Cmd::Serve { path } => commands::serve::run(&path).await,
        Cmd::Status { path } => commands::status::run(&path).await,
        Cmd::Update { force } => {
            // self_update uses blocking I/O; run off the async executor.
            tokio::task::spawn_blocking(move || commands::update::run(force)).await??;
            Ok(())
        }
    }
}

fn init_tracing(filter: &str) {
    use tracing_subscriber::{fmt, EnvFilter};
    let env = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(filter));
    // CRITICAL: when serve runs, stdout is the MCP JSON-RPC transport —
    // tracing MUST go to stderr only. fmt::init() defaults to stdout.
    let _ = fmt::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(env)
        .try_init();
}
