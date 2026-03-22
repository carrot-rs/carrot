pub mod auth;
mod conn;
mod message_stream;
mod notification;
mod peer;

pub use carrot_proto as proto;
pub use carrot_proto::{Receipt, TypedEnvelope, error::*};
pub use conn::Connection;
pub use notification::*;
pub use peer::*;
mod macros;

#[cfg(feature = "inazuma")]
mod proto_client;
#[cfg(feature = "inazuma")]
pub use proto_client::*;

pub const PROTOCOL_VERSION: u32 = 68;
