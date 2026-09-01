//! quota-check core library: provider plugins, credential loading,
//! quota normalization and human-readable rendering.
//!
//! To add a new provider (claude / gemini / ...):
//! 1. Add a module under `providers/` implementing `fetch_usage(auth) -> Value`
//! 2. Register a subcommand in the CLI. The human renderer needs no changes
//!    (it adapts to the response structure).

pub mod auth;
pub mod human;
pub mod providers;
