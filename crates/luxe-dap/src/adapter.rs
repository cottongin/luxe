//! Debug-adapter registry — resolves a program to a DAP adapter command + launch arguments.
//!
//! All supported adapters speak DAP over **stdio** (`codelldb` with no args, `python -m
//! debugpy.adapter`, `lldb-dap`, `gdb --interpreter=dap`), so the client needs a single transport.
//! Compiled languages debug a *binary*, so the adapter is chosen by explicit request or by a
//! native-first fallback rather than by source extension.

use crate::client::DapError;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::Path;

/// How an adapter shapes its `launch` request arguments.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdapterKind {
    /// Python via debugpy.
    Debugpy,
    /// Native (Rust/C/C++) via CodeLLDB or lldb-dap.
    Lldb,
    /// Native via `gdb --interpreter=dap`.
    Gdb,
    /// Go via Delve (`dlv dap`).
    Delve,
}

/// How the client connects to the adapter's DAP stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Transport {
    /// DAP over the adapter's stdio (debugpy, CodeLLDB, lldb-dap, gdb).
    Stdio,
    /// The adapter dials in to us over TCP; `placeholder` in the command is replaced with our
    /// loopback `host:port` (Delve `--client-addr`, js-debug).
    TcpDialIn { placeholder: String },
}

/// A resolved debug adapter: how to launch the adapter process + how to shape `launch` args.
#[derive(Debug, Clone)]
pub struct AdapterSpec {
    /// Stable adapter id (`debugpy`, `codelldb`, `lldb-dap`, `gdb`, `dlv`).
    pub id: String,
    /// Adapter command (the DAP server itself).
    pub command: Vec<String>,
    /// Launch-argument dialect.
    pub kind: AdapterKind,
    /// How the client connects to the adapter.
    pub transport: Transport,
}

impl AdapterSpec {
    /// Build the `launch` request arguments for a target program.
    pub fn launch_arguments(
        &self,
        program: &str,
        args: &[String],
        cwd: &str,
        stop_on_entry: bool,
    ) -> Value {
        match self.kind {
            AdapterKind::Debugpy => json!({
                "request": "launch",
                "program": program,
                "args": args,
                "console": "internalConsole",
                "cwd": cwd,
                "stopOnEntry": stop_on_entry,
            }),
            AdapterKind::Lldb => json!({
                "request": "launch",
                "program": program,
                "args": args,
                "cwd": cwd,
                "stopOnEntry": stop_on_entry,
            }),
            AdapterKind::Gdb => json!({
                "request": "launch",
                "program": program,
                "args": args,
                "cwd": cwd,
                "stopAtBeginningOfMainSubprogram": stop_on_entry,
            }),
            // Delve `mode: debug` builds + runs the package/file at `program`.
            AdapterKind::Delve => json!({
                "request": "launch",
                "mode": "debug",
                "program": program,
                "args": args,
                "cwd": cwd,
                "stopOnEntry": stop_on_entry,
            }),
        }
    }

    /// Build the `attach` request arguments: by `port` (debugpy connect) or `pid` (native).
    pub fn attach_arguments(&self, pid: Option<i64>, port: Option<i64>) -> Value {
        match self.kind {
            AdapterKind::Debugpy => {
                if let Some(port) = port {
                    json!({ "request": "attach", "connect": { "host": "127.0.0.1", "port": port } })
                } else {
                    json!({ "request": "attach", "processId": pid })
                }
            }
            AdapterKind::Lldb | AdapterKind::Gdb => {
                json!({ "request": "attach", "pid": pid })
            }
            AdapterKind::Delve => {
                json!({ "request": "attach", "mode": "local", "processId": pid })
            }
        }
    }
}

/// Placeholder in the Delve command replaced with our loopback `host:port` for dial-in.
const DAP_ADDR: &str = "${DAP_ADDR}";

// === Config-driven custom adapters ===

/// Launch-argument dialect a custom adapter reuses (most DAP adapters fit one of these shapes).
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Dialect {
    Debugpy,
    Lldb,
    Gdb,
    Delve,
}

impl Dialect {
    fn kind(self) -> AdapterKind {
        match self {
            Dialect::Debugpy => AdapterKind::Debugpy,
            Dialect::Lldb => AdapterKind::Lldb,
            Dialect::Gdb => AdapterKind::Gdb,
            Dialect::Delve => AdapterKind::Delve,
        }
    }
}

/// How a custom adapter is reached.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum TransportConfig {
    /// DAP over the adapter's stdio (default).
    #[default]
    Stdio,
    /// The adapter dials in to us over TCP (include an address placeholder in `command`).
    TcpDialIn,
}

/// A user-configured adapter from `~/.config/luxe/debug-adapters.json`. Lets users (or a model
/// working in Luxe) add debuggers for more languages without a code change, reusing a built-in
/// launch dialect.
#[derive(Debug, Clone, Deserialize)]
pub struct CustomAdapter {
    /// Stable id, selectable via the debug tool's `adapter` arg.
    pub id: String,
    /// Adapter command (argv). For `tcp_dial_in`, include the address placeholder.
    pub command: Vec<String>,
    /// Source extensions this adapter auto-handles (leading dot optional), e.g. `["rb"]`.
    #[serde(default)]
    pub extensions: Vec<String>,
    /// Which built-in launch dialect to use.
    pub dialect: Dialect,
    /// Transport (default stdio).
    #[serde(default)]
    pub transport: TransportConfig,
    /// For `tcp_dial_in`: the substring in `command` replaced with our `host:port`
    /// (default `${DAP_ADDR}`).
    #[serde(default)]
    pub addr_placeholder: Option<String>,
}

/// A resolved custom adapter: its spec plus the extensions it auto-handles.
#[derive(Debug, Clone)]
pub struct ConfiguredAdapter {
    pub spec: AdapterSpec,
    pub extensions: Vec<String>,
}

impl ConfiguredAdapter {
    /// Build a resolved adapter from its config entry.
    pub fn from_custom(custom: &CustomAdapter) -> Self {
        let transport = match custom.transport {
            TransportConfig::Stdio => Transport::Stdio,
            TransportConfig::TcpDialIn => Transport::TcpDialIn {
                placeholder: custom
                    .addr_placeholder
                    .clone()
                    .unwrap_or_else(|| DAP_ADDR.to_string()),
            },
        };
        ConfiguredAdapter {
            spec: AdapterSpec {
                id: custom.id.clone(),
                command: custom.command.clone(),
                kind: custom.dialect.kind(),
                transport,
            },
            extensions: custom
                .extensions
                .iter()
                .map(|e| e.trim_start_matches('.').to_string())
                .collect(),
        }
    }
}

/// The shipped default debug adapters, using the same `CustomAdapter` schema as user config, so
/// built-in + user adapters share one parse + resolution path.
const DEFAULT_ADAPTERS_JSON: &str = include_str!("default-debug-adapters.json");

/// The shipped built-in debug adapters (debugpy, codelldb, lldb-dap, gdb, dlv), for
/// status/introspection surfaces like the `/debug` command.
pub fn builtin_adapters() -> Vec<ConfiguredAdapter> {
    default_adapters()
}

/// Parse the bundled default adapters. Compile-time-embedded + covered by
/// `default_adapters_bundle_parses`, so a failure is a build-time bug, never a runtime path.
fn default_adapters() -> Vec<ConfiguredAdapter> {
    let customs: Vec<CustomAdapter> = serde_json::from_str(DEFAULT_ADAPTERS_JSON)
        .expect("bundled default-debug-adapters.json must be valid");
    customs.iter().map(ConfiguredAdapter::from_custom).collect()
}

/// Whether any debug adapter — user-configured or built-in — is actually installed.
///
/// The `debug` tool is the most expensive schema in the set, and it is dead weight on a machine
/// with no adapter: every call can only answer "no debug adapter found". Registration is gated on
/// this so that cost is paid only where the capability exists. Nothing is hidden by doing so —
/// unlike deferring a working tool, an absent adapter means there is no debugging to offer.
pub fn any_adapter_available(custom: &[ConfiguredAdapter]) -> bool {
    custom
        .iter()
        .chain(default_adapters().iter())
        .any(|c| c.spec.command.first().is_some_and(|program| on_path(program)))
}

/// Resolves the adapter for a target program.
///
/// An explicit `adapter` id wins (user adapters shadow built-ins). Otherwise an adapter whose
/// `extensions` match the program's extension wins (user first, then built-in — e.g. `.py`→debugpy,
/// `.go`→dlv), and everything else (a compiled binary) takes the first installed native adapter:
/// CodeLLDB, then lldb-dap, then gdb.
pub fn resolve(
    program: &str,
    adapter: Option<&str>,
    custom: &[ConfiguredAdapter],
) -> Result<AdapterSpec, DapError> {
    let builtin = default_adapters();
    // User adapters take precedence over built-ins with the same id/extension.
    let ordered = || custom.iter().chain(builtin.iter());

    if let Some(id) = adapter {
        return ordered()
            .find(|c| c.spec.id == id)
            .map(|c| c.spec.clone())
            .ok_or_else(|| DapError::AdapterUnavailable(format!("unknown adapter '{id}'")))
            .and_then(ensure_installed);
    }

    if let Some(ext) = Path::new(program).extension().and_then(|e| e.to_str()) {
        if let Some(found) = ordered().find(|c| c.extensions.iter().any(|e| e == ext)) {
            return ensure_installed(found.spec.clone());
        }
    }

    // Compiled binary: first installed native adapter (Delve is Go-source-specific, excluded).
    builtin
        .iter()
        .filter(|c| matches!(c.spec.kind, AdapterKind::Lldb | AdapterKind::Gdb))
        .find(|c| on_path(&c.spec.command[0]))
        .map(|c| c.spec.clone())
        .ok_or_else(|| {
            DapError::AdapterUnavailable(
                "no native debug adapter found (install codelldb, lldb-dap, or gdb)".to_string(),
            )
        })
}

fn ensure_installed(spec: AdapterSpec) -> Result<AdapterSpec, DapError> {
    if on_path(&spec.command[0]) {
        Ok(spec)
    } else {
        Err(DapError::AdapterUnavailable(format!(
            "debug adapter '{}' not found on PATH",
            spec.command[0]
        )))
    }
}

/// Whether an executable is resolvable — `PATH`, a well-known install directory, or an explicit
/// path. Debug adapters are installed by `cargo install`, `pipx`, or an editor extension, none of
/// which reliably land on the `PATH` an app inherits.
fn on_path(program: &str) -> bool {
    luxe_core::program::is_available(program)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn python_targets_resolve_to_debugpy_dialect() {
        let spec = AdapterSpec {
            id: "debugpy".into(),
            command: vec!["python3".into()],
            kind: AdapterKind::Debugpy,
            transport: Transport::Stdio,
        };
        let args = spec.launch_arguments("/w/app.py", &["--flag".into()], "/w", true);
        assert_eq!(args["program"], "/w/app.py");
        assert_eq!(args["console"], "internalConsole");
        assert_eq!(args["stopOnEntry"], true);
    }

    #[test]
    fn gdb_uses_its_own_stop_key() {
        let spec = AdapterSpec {
            id: "gdb".into(),
            command: vec!["gdb".into()],
            kind: AdapterKind::Gdb,
            transport: Transport::Stdio,
        };
        let args = spec.launch_arguments("/w/bin", &[], "/w", true);
        assert_eq!(args["stopAtBeginningOfMainSubprogram"], true);
    }

    #[test]
    fn unknown_adapter_is_rejected() {
        assert!(resolve("/w/app.py", Some("nope"), &[]).is_err());
    }

    #[test]
    fn default_adapters_bundle_parses() {
        let adapters = default_adapters();
        let ids: Vec<&str> = adapters.iter().map(|a| a.spec.id.as_str()).collect();
        for id in ["debugpy", "codelldb", "lldb-dap", "gdb", "dlv"] {
            assert!(ids.contains(&id), "missing built-in adapter {id}");
        }
        let dlv = adapters.iter().find(|a| a.spec.id == "dlv").unwrap();
        assert_eq!(dlv.spec.kind, AdapterKind::Delve);
        assert!(matches!(dlv.spec.transport, Transport::TcpDialIn { .. }));
        assert!(dlv.extensions.contains(&"go".to_string()));
        let debugpy = adapters.iter().find(|a| a.spec.id == "debugpy").unwrap();
        assert!(debugpy.extensions.contains(&"py".to_string()));
    }

    #[test]
    fn custom_adapter_parses_and_maps() {
        let json = r#"{"id":"ruby","command":["rdbg","--dap","${DAP_ADDR}"],
            "extensions":[".rb"],"dialect":"debugpy","transport":"tcp_dial_in"}"#;
        let custom: CustomAdapter = serde_json::from_str(json).unwrap();
        let cfg = ConfiguredAdapter::from_custom(&custom);
        assert_eq!(cfg.spec.id, "ruby");
        assert_eq!(cfg.spec.kind, AdapterKind::Debugpy);
        assert!(matches!(cfg.spec.transport, Transport::TcpDialIn { .. }));
        assert_eq!(cfg.extensions, vec!["rb"]); // leading dot stripped
    }

    /// A program that is genuinely installed, so `ensure_installed` passes on its own merits
    /// rather than on the fixture's. `sh` is not that program: Windows has no POSIX shell unless
    /// somebody installed one.
    fn installed_program() -> String {
        if cfg!(windows) { "cmd" } else { "sh" }.to_string()
    }

    #[test]
    fn custom_adapter_wins_by_id_and_extension() {
        let custom = ConfiguredAdapter {
            spec: AdapterSpec {
                id: "myruby".into(),
                command: vec![installed_program()],
                kind: AdapterKind::Debugpy,
                transport: Transport::Stdio,
            },
            extensions: vec!["rb".into()],
        };
        let extra = std::slice::from_ref(&custom);
        assert_eq!(resolve("/w/x", Some("myruby"), extra).unwrap().id, "myruby");
        assert_eq!(resolve("/w/app.rb", None, extra).unwrap().id, "myruby");
    }

    /// The `debug` schema is the largest in the set; it should not be paid for on a machine that
    /// cannot debug. An adapter with a command that resolves to nothing must not count.
    #[test]
    fn availability_tracks_whether_an_adapter_command_resolves() {
        let missing = ConfiguredAdapter::from_custom(&CustomAdapter {
            id: "ghost".to_string(),
            command: vec!["definitely-not-a-real-adapter-xyz".to_string()],
            extensions: vec!["zz".to_string()],
            dialect: Dialect::Lldb,
            transport: TransportConfig::default(),
            addr_placeholder: None,
        });
        assert!(!any_adapter_available(&[missing]) || any_adapter_available(&[]));

        let real = ConfiguredAdapter::from_custom(&CustomAdapter {
            id: "shell".to_string(),
            command: vec![installed_program()],
            extensions: vec!["zz".to_string()],
            dialect: Dialect::Lldb,
            transport: TransportConfig::default(),
            addr_placeholder: None,
        });
        assert!(any_adapter_available(&[real]), "an installed command counts as available");
    }
}
