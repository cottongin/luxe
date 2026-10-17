//! luxe-dap — a native Debug Adapter Protocol client and the `debug` tool.
//!
//! Gives the agent a real debugger: launch/attach a program, set breakpoints, step, and inspect
//! threads/stack/variables/expressions across adapters (CodeLLDB/lldb-dap/gdb for native, debugpy
//! for Python) over stdio. Mirrors `luxe-lsp`'s warm-service shape — one long-lived
//! [`service::DapService`] reuses sessions across tool calls. Allowed edge: `dap → core`.

pub mod adapter;
mod client;
pub mod protocol;
pub mod service;
mod tool;

pub use adapter::{
    any_adapter_available, builtin_adapters, AdapterSpec, ConfiguredAdapter, CustomAdapter,
};
pub use client::{DapClient, DapError};
pub use service::{
    BreakpointRequest, DapService, DapSession, DapSessionInfo, FunctionBreakpoint, LaunchOutcome,
    SourceBreakpoint,
};
pub use tool::DebugTool;
