pub mod adapters;
#[cfg(catalog)]
mod catalog;
pub mod cli;
mod commands;
pub mod discovery;
pub mod error;
pub mod guidance;
pub mod identity;
mod mcp;
pub mod model;
pub mod notify;
pub mod os;
mod schema;
pub mod store;
pub mod validate;
mod write_turn;
