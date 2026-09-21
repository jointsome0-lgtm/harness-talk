use std::path::{Path, PathBuf};
use crate::{error::{Error, Failure}, model::*};
pub struct Store { path: PathBuf }
impl Store {
    pub fn open(path: &Path, create: bool) -> Result<Self, Error> { todo!() }
    pub fn path(&self) -> &Path { &self.path }
    pub fn add_peer(&self, name:&str, harness:Harness, session:&str, workspace:&str, socket:Option<&str>, url:Option<&str>) -> Result<Peer,Error> { todo!() }
    pub fn peer(&self, name:&str) -> Result<Peer,Error> { todo!() }
    pub fn session_peer(&self, h:Harness, session:&str) -> Result<Option<Peer>,Error> { todo!() }
    pub fn peers(&self) -> Result<Vec<Peer>,Error> { todo!() }
    pub fn retire(&self, name:&str) -> Result<Peer,Error> { todo!() }
    pub fn restore(&self, name:&str) -> Result<Peer,Error> { todo!() }
    pub fn save(&self, sender:&str, recipient:&str, body:&str, id:Option<&str>, in_reply_to:Option<&str>) -> Result<(Message,bool),Error> { todo!() }
    pub fn notify_once(&self, id:&str, notify:&dyn Fn(&Peer,&Message)->Outcome, dismiss:&dyn Fn(&Peer,&Message)->Cleanup) -> Result<Message,Error> { todo!() }
    pub fn get(&self, id:&str, actor:Option<&str>) -> Result<Message,Error> { todo!() }
    pub fn ack(&self, id:&str, actor:&str, dismiss:&dyn Fn(&Peer,&Message)->Cleanup) -> Result<Message,Error> { todo!() }
    pub fn wait(&self, id:&str, actor:&str, seconds:f64) -> Result<Message,Error> { todo!() }
    pub fn inbox(&self, actor:&str, limit:i64, after_seq:Option<i64>) -> Result<Page,Error> { todo!() }
    pub fn sent(&self, actor:&str, limit:i64, before_seq:Option<i64>, bodies:bool) -> Result<Page,Error> { todo!() }
}
pub fn skip_reason_readonly(db:&Path, message_id:&str, recipient:&str) -> Result<Option<SkipReason>,Failure> { todo!() }
