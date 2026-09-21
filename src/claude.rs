use std::path::PathBuf;
use serde_json::Value;
use crate::{error::Failure, model::*};
pub fn agents() -> Result<Vec<Value>,Failure> { todo!() }
pub fn session_metadata(pid:i64) -> Result<Value,Failure> { todo!() }
pub fn live_socket(peer:&Peer) -> Result<PathBuf,Failure> { todo!() }
pub fn notify(peer:&Peer, message:&Message, body:&str, skip:Skip<'_>) -> Outcome { todo!() }
pub fn probe(peer:&Peer) -> Result<Value,Failure> { todo!() }
