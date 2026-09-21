use serde_json::Value;
use crate::{error::Failure, model::*};
pub fn notify(peer:&Peer, body:&str, skip:Skip<'_>) -> Outcome { todo!() }
pub fn probe(peer:&Peer) -> Result<Value,Failure> { todo!() }
pub fn discover(urls:Option<&[String]>, workspace:Option<&str>) -> Found { todo!() }
