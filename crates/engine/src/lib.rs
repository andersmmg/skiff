//! aria2-next integration: sidecar process supervision and JSON-RPC transport.
//!
//! This crate is UI-free on purpose. It owns the `aria2-next` engine process
//! (or attaches to one already running), polls task state in a single
//! `system.multicall` round trip per tick, and hands out typed snapshots.

mod client;
mod runtime;
mod sidecar;
mod types;

pub use client::{RpcClient, RpcError};
pub use runtime::{Engine, EngineConfig, EngineEvent, Reconfigurator, request_shutdown};
pub use sidecar::Sidecar;
pub use types::*;
