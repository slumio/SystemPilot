use clap::{Args, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "syspilot", version, about = "Evidence-first Linux diagnostics")]
pub struct Cli {
    #[arg(long, global = true)]
    pub json: bool,
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    Setup(SetupArgs),
    Install(InstallArgs),
    Uninstall(UninstallArgs),
    Status,
    Doctor,
    Evidence(EvidenceArgs),
    Cases {
        #[command(subcommand)]
        action: CaseAction,
    },
    Alerts {
        #[command(subcommand)]
        action: AlertStateAction,
    },
    Support {
        #[command(subcommand)]
        action: SupportAction,
    },
    Completions {
        shell: Shell,
    },
    Fleet {
        #[command(subcommand)]
        action: FleetAction,
    },
    Daemon,
    Events,
    Monitor,
    Provider {
        name: String,
    },
    Model {
        name: String,
    },
    Pull {
        model: String,
        #[arg(long)]
        set_active: bool,
    },
    Index {
        #[arg(long)]
        force: bool,
    },
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
    Ask(AskArgs),
    Explain(ExplainArgs),
    Version,
}

#[derive(Debug, Args)]
pub struct SetupArgs {
    #[arg(long, conflicts_with_all = ["line", "check"])]
    pub tui: bool,
    #[arg(long, conflicts_with_all = ["tui", "check"])]
    pub line: bool,
    #[arg(long, conflicts_with_all = ["tui", "line"])]
    pub check: bool,
}
#[derive(Debug, Args)]
pub struct InstallArgs {
    #[arg(long)]
    pub binary: bool,
    #[arg(long, requires = "binary")]
    pub force: bool,
}
#[derive(Debug, Args)]
pub struct UninstallArgs {
    #[arg(long)]
    pub binary: bool,
}
#[derive(Debug, Args)]
pub struct EvidenceArgs {
    #[arg(long)]
    pub pid: Option<String>,
}
#[derive(Debug, Subcommand)]
pub enum CaseAction {
    List,
    Show { id: String },
    Export { id: String, path: Option<PathBuf> },
    Delete { id: String },
}
#[derive(Debug, Subcommand)]
pub enum AlertStateAction {
    List,
    Acknowledge { id: String },
    Resolve { id: String },
    Suppress { id: String },
}
#[derive(Debug, Subcommand)]
pub enum SupportAction {
    Bundle {
        #[command(subcommand)]
        action: BundleAction,
    },
}
#[derive(Debug, Subcommand)]
pub enum BundleAction {
    Create { path: Option<PathBuf> },
}
#[derive(Debug, Clone, ValueEnum)]
pub enum Shell {
    Bash,
    Zsh,
    Fish,
}
#[derive(Debug, Subcommand)]
pub enum FleetAction {
    Enroll { endpoint: String, node_id: String },
    Status,
    Disable,
}

#[derive(Debug, Subcommand)]
pub enum ConfigAction {
    Telemetry {
        #[command(subcommand)]
        action: Option<TelemetryAction>,
    },
    Alert {
        #[command(subcommand)]
        action: AlertConfigAction,
    },
    SetKey {
        provider: String,
    },
    SetUrl {
        provider: String,
        url: String,
    },
    Set {
        option: String,
        value: String,
    },
    Rollback,
}
#[derive(Debug, Subcommand)]
pub enum TelemetryAction {
    Enable { endpoint: String, node_id: String },
    Disable,
    Show,
    Preview { target: Option<String> },
}
#[derive(Debug, Subcommand)]
pub enum AlertConfigAction {
    Add {
        id: String,
        match_type: String,
        process_name: String,
    },
    Remove {
        id: String,
    },
    List,
}
#[derive(Debug, Args)]
pub struct AskArgs {
    pub question: String,
    #[arg(long)]
    pub file: Option<PathBuf>,
    #[arg(long)]
    pub no_index: bool,
}
#[derive(Debug, Args)]
pub struct ExplainArgs {
    #[arg(long)]
    pub pid: Option<String>,
    #[arg(long)]
    pub deep: bool,
    #[arg(long)]
    pub ebpf: bool,
    #[arg(long, requires = "pid")]
    pub causal: bool,
    #[arg(long, default_value_t = 1)]
    pub number: usize,
    #[arg(long)]
    pub no_index: bool,
}
