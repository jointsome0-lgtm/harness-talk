pub mod rpc;
pub mod state;
use serde_json::Value;
use crate::{error::Failure, model::*};
pub fn notify(peer:&Peer, message_id:&str, body:&str, skip:Skip<'_>) -> Outcome { todo!() }
pub fn dismiss(peer:&Peer, message:&Message) -> Cleanup { todo!() }
pub fn probe(peer:&Peer) -> Result<Value,Failure> { todo!() }
