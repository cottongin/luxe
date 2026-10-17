mod activity_projector;
mod auth_terminal;
mod cli;
mod config_editor;
mod dashboard_data;
mod engine_setup;
mod git_probe;
mod graph_hook;
#[cfg(test)]
mod guide_check;
mod init;
mod logging;
mod loop_run;
mod loop_view;
mod model_catalog;
mod output;
mod phase_review_context;
mod pull_request_watch;
mod review_agent;
mod review_insight;
mod status_data;
mod update_check;
mod workspace_data;
mod workspace_runtime;

use clap::Parser;

fn main() -> anyhow::Result<()> {
    let args = cli::Args::parse();
    // Before the runtime, deliberately: claiming the slot republishes an environment variable for
    // our children to inherit, and mutating the environment is only sound while this process is
    // still single-threaded.
    if let Err(refused) = luxe_core::program::claim_nesting_slot() {
        eprintln!("luxe: {refused}");
        std::process::exit(1);
    }
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    match runtime.block_on(cli::run(args)) {
        // Printed where it happened; the exit status is what is left to say
        Err(error) if error.is::<cli::headless::AlreadyReported>() => std::process::exit(1),
        // A refused flag combination reads as a sentence, not as a failure to be debugged
        Err(error) if error.is::<cli::UsageError>() => {
            eprintln!("luxe: {error}");
            std::process::exit(1)
        }
        result => result,
    }
}
