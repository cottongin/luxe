//! Subcommands — the arguments each `luxe <command>` takes.

use clap::{Parser, Subcommand};

/// Subcommands for luxe.
#[derive(Subcommand, Debug)]
pub(crate) enum SubCommand {
    /// Sign in to a provider, check credentials, or sign out.
    Auth(AuthArgs),
    /// Read luxe's user guide, for this version.
    Docs(DocsArgs),
    /// Check which language servers, debug adapters and tools work on this machine.
    Doctor,
    /// Set luxe up: a theme, a provider, the tools and the start screen.
    Init(InitArgs),
    /// Join a session another luxe on this machine is running, speaking the `--rpc` protocol on
    /// stdin and stdout.
    Join(JoinArgs),
    /// Serve your sessions to your own phone or tablet, through Tailscale.
    #[cfg(feature = "remote")]
    Remote(RemoteArgs),
    /// Run the agent through a phased plan on its own.
    Loop(LoopArgs),
    /// Add, list and sign in to MCP servers.
    Mcp(McpArgs),
    /// List models, show one's details, or refresh the catalog.
    Models(ModelsArgs),
    /// List this project's sessions.
    Sessions(SessionsArgs),
    /// Update luxe with the command for however it was installed.
    Update(UpdateArgs),
    /// Summarize a `LUXE_WIRE_LOG` capture.
    Wire(WireArgs),
}

/// Update arguments.
#[derive(Parser, Debug)]
pub(crate) struct UpdateArgs {
    /// Only say whether a newer release exists, and how to get it.
    #[arg(long)]
    pub(crate) check: bool,
}

/// Docs arguments.
#[derive(Parser, Debug)]
pub(crate) struct DocsArgs {
    /// A page to print, such as `install` or `hooks`. Without one, lists every page.
    pub(crate) page: Option<String>,
    /// Print the lines across every page that contain all of these words.
    #[arg(long)]
    pub(crate) search: Option<String>,
}

/// Wire arguments.
#[derive(Parser, Debug)]
pub(crate) struct WireArgs {
    /// Path to a log written by `LUXE_WIRE_LOG=<path> luxe ...`.
    pub(crate) log: std::path::PathBuf,
    /// Break the tool-schema total down per tool.
    #[arg(long)]
    pub(crate) tools: bool,
}

/// Auth arguments.
#[derive(Parser, Debug)]
pub(crate) struct AuthArgs {
    #[command(subcommand)]
    pub(crate) command: AuthCommand,
}

/// Auth subcommand — provider credentials.
#[derive(Subcommand, Debug)]
pub(crate) enum AuthCommand {
    /// Sign in to a provider (API key or subscription).
    Login {
        /// Provider id (e.g. `openai`, `anthropic`, `github-copilot`). Omit to choose from a list.
        provider: Option<String>,
        /// Which login to run: `api-key` or `subscription`. Only needed when a provider has both.
        #[arg(long)]
        method: Option<String>,
        /// Supply the API key directly instead of being prompted. Beware shell history.
        #[arg(long, conflicts_with = "stdin")]
        api_key: Option<String>,
        /// Read the API key from stdin (`pass show openai | luxe auth login openai --stdin`).
        #[arg(long)]
        stdin: bool,
        /// Skip the "does this key work" request — for an air-gapped or proxied setup.
        #[arg(long)]
        no_verify: bool,
    },
    /// List every provider and how each one is configured.
    List,
    /// Show credential detail (kind, source, expiry) for one provider or all configured ones.
    Status {
        /// Provider id. Omit for every configured provider.
        provider: Option<String>,
    },
    /// Remove a stored credential. Environment variables are left alone.
    Logout {
        /// Provider id.
        provider: String,
    },
}

/// Models arguments.
#[derive(Parser, Debug)]
pub(crate) struct ModelsArgs {
    #[command(subcommand)]
    pub(crate) command: ModelsCommand,
}

/// Models subcommand — what luxe knows about a model, and where it learned it.
#[derive(Subcommand, Debug)]
pub(crate) enum ModelsCommand {
    /// List known models for the providers luxe can route to.
    List {
        /// Only this provider.
        #[arg(long)]
        provider: Option<String>,
        /// Only models pinned in `models.json` / `auth.json`.
        #[arg(long)]
        configured: bool,
    },
    /// Show one model's resolved metadata and which layer supplied each field.
    Show {
        /// `provider/id`, or a bare id to use the active provider.
        model: String,
    },
    /// Revalidate the models.dev cache now.
    Refresh {
        /// Fetch even when the cache is still current.
        #[arg(long)]
        force: bool,
    },
}

/// Init arguments.
#[derive(Parser, Debug)]
pub(crate) struct InitArgs {
    /// Start over: remove the existing config before setting up.
    #[arg(long)]
    pub(crate) force: bool,
    /// Write a config that chooses nothing, without asking, so every default applies.
    #[arg(long)]
    pub(crate) defaults: bool,
}

/// Join arguments.
#[derive(Parser, Debug)]
pub(crate) struct JoinArgs {
    /// The session: its id or the start of it, or the process id of the luxe running it. Omit
    /// it to join the one running in this folder.
    pub(crate) target: Option<String>,
    /// List the sessions running on this machine that can be joined, and exit.
    #[arg(long, conflicts_with = "target")]
    pub(crate) list: bool,
}

/// Remote arguments.
#[cfg(feature = "remote")]
#[derive(Parser, Debug)]
pub(crate) struct RemoteArgs {
    #[command(subcommand)]
    pub(crate) command: Option<RemoteCommand>,
    /// The port to listen on, on this machine only.
    #[arg(long, default_value_t = 4519, global = true)]
    pub(crate) port: u16,
    /// Also let this tailnet login in, besides this machine's owner (repeatable).
    #[arg(long = "allow", value_name = "LOGIN", global = true)]
    pub(crate) allow: Vec<String>,
}

/// Remote subcommand — pair and manage your devices.
#[cfg(feature = "remote")]
#[derive(Subcommand, Debug)]
pub(crate) enum RemoteCommand {
    /// Check Tailscale, publish the bridge on your tailnet, start it at login, and pair a device.
    Setup,
    /// Show a QR code and link that pair one phone or tablet, good for ten minutes.
    Pair,
    /// List the paired devices.
    Devices {
        #[command(subcommand)]
        command: Option<DevicesCommand>,
    },
    /// Manage the user service that runs the bridge at login.
    Service {
        #[command(subcommand)]
        command: ServiceCommand,
    },
}

/// Service subcommand.
#[cfg(feature = "remote")]
#[derive(Subcommand, Debug)]
pub(crate) enum ServiceCommand {
    /// Stop the service and stop it starting at login.
    Remove,
}

/// Devices subcommand.
#[cfg(feature = "remote")]
#[derive(Subcommand, Debug)]
pub(crate) enum DevicesCommand {
    /// Unpair a device and close its open sessions.
    Revoke {
        /// The device's id, from `luxe remote devices`
        id: String,
    },
}

/// Sessions arguments.
#[derive(Parser, Debug)]
pub(crate) struct SessionsArgs {
    #[command(subcommand)]
    pub(crate) command: SessionsCommand,
}

/// Sessions subcommand — inspect resumable sessions.
#[derive(Subcommand, Debug)]
pub(crate) enum SessionsCommand {
    /// List sessions for the current project (newest first).
    List,
}

/// Loop arguments.
#[derive(Parser, Debug)]
pub(crate) struct LoopArgs {
    #[command(subcommand)]
    pub(crate) command: LoopCommand,
}

/// MCP arguments.
#[derive(Parser, Debug)]
pub(crate) struct McpArgs {
    #[command(subcommand)]
    pub(crate) command: McpCommand,
}

/// Loop subcommand — manage continuous development loops.
#[derive(Subcommand, Debug)]
pub(crate) enum LoopCommand {
    /// Start a new continuous loop.
    Start {
        /// Print nothing but the banner: no per-turn activity. The log and the journal are
        /// written either way.
        #[arg(long)]
        quiet: bool,

        /// Maximum turns per run (default: 50).
        #[arg(long, default_value_t = 50)]
        max_turns: u32,

        /// Compact at this percentage of the usable window (default: `compaction.at`, else 75).
        #[arg(long, value_parser = clap::value_parser!(u8).range(1..=100))]
        compact_percent: Option<u8>,

        /// Pause at this percentage of context window (default: 90).
        #[arg(long, default_value_t = 90)]
        pause_percent: u8,
    },

    /// Stop the current loop.
    Stop,

    /// Pause the current loop.
    Pause,

    /// Resume a paused loop.
    Resume,

    /// Show loop status.
    Status,

    /// Show loop phase (and optionally set it).
    Phase {
        /// Set phase to this number.
        #[arg(long)]
        set: Option<u32>,
    },

    /// Reset the run's turn count, so the next `luxe loop start` gets all of `--max-turns` again.
    Fresh,
}

/// MCP subcommand — manage MCP servers and tools.
#[derive(Subcommand, Debug)]
pub(crate) enum McpCommand {
    /// List configured MCP servers.
    List,
    /// Add an MCP server.
    Add {
        /// Server ID.
        #[arg(long)]
        id: String,

        /// Server name.
        #[arg(long)]
        name: String,

        /// Command to run (for stdio transport).
        #[arg(long)]
        command: Option<String>,

        /// URL (for HTTP/SSE transport).
        #[arg(long)]
        url: Option<String>,
    },
    /// Remove an MCP server.
    Remove {
        /// Server ID to remove.
        #[arg(long)]
        id: String,
    },
    /// List tools from all MCP servers.
    Tools,
    /// Authorize an OAuth-protected MCP server (opens a browser flow).
    Login {
        /// Server ID to authorize.
        #[arg(long)]
        id: String,
    },
}
