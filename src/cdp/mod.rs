pub mod cdp;
pub mod cdp_protocol;
pub mod click;
pub mod write;
pub mod ws_server;

pub use cdp::CDP;

#[allow(unused_imports)]
pub mod prelude {
    pub use super::click::Click;
    pub use super::write::Write;
    pub use super::CDP;
    pub use base64::prelude::*;
    pub use log::{debug, error, info, warn};
    pub use reqwest;
    pub use serde_json::{json, Value};
    pub use std::sync::Arc;
    pub use tokio::sync::Mutex;
    pub use tokio::time::{sleep, Duration};
}
