use std::path::PathBuf;
use serde_json::Value;
use crate::error::Failure;
pub fn codex_home() -> PathBuf { todo!() }
pub fn state_path() -> Result<PathBuf,Failure> { todo!() }
pub fn saved_thread(id:&str) -> Result<Option<(String,Value,i64,Value)>,Failure> { todo!() }
