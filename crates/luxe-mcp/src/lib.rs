//! luxe-mcp — Model Context Protocol orchestration.
//!
//! The problem this solves: MCP tool definitions are verbose. Registering every tool from every
//! server up front burns thousands of context tokens whether or not the agent uses them, and eager
//! connections spawn processes at launch. Instead luxe exposes **one** proxy tool ([`McpProxyTool`],
//! `mcp`) — the agent *discovers* tools on demand (`search` → `describe` → `call`) — backed by a
//! [`ServerRegistry`] that connects servers **lazily** and caches their tool metadata so discovery
//! works without a live connection.
//!
//! Architecture: the low-level stdio transport + MCP types live in `luxe-core` (no HTTP dep
//! there); this crate carries the orchestration plus the Streamable-HTTP transport and OAuth that
//! core's no-HTTP rule keeps out of core. MCP servers stay untrusted — calls are permission-gated
//! and results are surfaced as data, never instructions.
//!
//! Both transports are reached through [`connect`], so nothing has to know which one a server
//! uses. A remote server is configured with a URL and, where it needs one, a bearer token or an
//! OAuth flow — which is how a vendor's hosted MCP server (Firecrawl, Exa, and the rest) is used
//! from luxe without luxe carrying an adapter for it.

pub mod http;
pub mod oauth;
mod registry;
mod secret;
pub mod token_store;
mod tool;

pub use http::McpHttpError;
pub use oauth::OAuthError;
pub use registry::{open_connection as connect, RegistryError, ServerRegistry, ToolRef};
pub use secret::SecretError;
pub use token_store::TokenStoreError;
pub use tool::McpProxyTool;
