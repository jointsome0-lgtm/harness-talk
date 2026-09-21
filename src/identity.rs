use std::path::Path;
use crate::model::NativeSession;
pub fn claude_session(env:&dyn Fn(&str)->Option<String>, proc_root:&Path, sessions_dir:Option<&Path>, pid:Option<u32>) -> NativeSession { todo!() }
