use std::path::Path;
use serde_json::Value;
use crate::error::Failure;
pub struct Rpc;
impl Rpc {
    pub fn connect_unix(path:&Path) -> Result<Self,Failure> { todo!() }
    pub fn spawn_stdio() -> Result<Self,Failure> { todo!() }
    pub fn call(&mut self, method:&str, params:Value) -> Result<Value,Failure> { todo!() }
}
