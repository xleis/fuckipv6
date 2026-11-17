//! C ABI bridge between an iOS host (Packet Tunnel extension) and the
//! existing embedded fuckipv6 node runtime.
//!
//! The bridge never re-implements the node: it owns an [`tokio::runtime::Runtime`],
//! creates [`fuckipv6_client::embedded::EmbeddedNodeRuntime`] instances and
//! exposes them through an opaque handle plus a bounded, JSON-typed control
//! surface. Packet I/O is delegated to the P1 `ExternalPacketDevice`; the host
//! holds the endpoint token and pumps whole IP packets into its VPN stack.
//!
//! Ownership rules for the C surface:
//! - every `Fv6Runtime *` is owned by the caller and destroyed exactly once;
//! - every endpoint token is owned by the host after a successful
//!   `open_network` callback and must be released exactly once;
//! - buffers are caller-allocated, never retained by Rust;
//! - strings are UTF-8 without a NUL terminator unless documented otherwise;
//! - Rust panics never cross the ABI, they are reported as
//!   [`Fv6Status::Panic`].

mod error;
mod ffi;
mod packet;
mod platform;
mod settings;

#[cfg(test)]
mod tests;

pub use error::{status_from_io, Fv6Status};
pub use ffi::*;
pub use packet::{validate as validate_packet, IpVersion};
pub use platform::{EndpointHandle, HostBridge, HostCallbacks};
pub use settings::{HostAddress, TunnelSettings};
