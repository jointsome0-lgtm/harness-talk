//! How the catalogue reaches a device it found.
pub(super) mod ssh;

use crate::error::Error;

/// One way to open a byte pipe to a found device. The catalogue is fetched over it and the
/// MCP session runs over it.
pub(super) trait Link: Send + Sync {
    /// The command whose stdin and stdout are the pipe to one operation of the endpoint.
    fn command(&self, operation: &str) -> Vec<String>;
    /// The path is still the one discovery found.
    fn check(&self) -> Result<(), Error>;
}

/// The connector an operator spelled out with `mcp --connect`. There is nothing to check.
pub(super) struct Given(pub Vec<String>);
impl Link for Given {
    fn command(&self, _operation: &str) -> Vec<String> {
        self.0.clone()
    }
    fn check(&self) -> Result<(), Error> {
        Ok(())
    }
}
