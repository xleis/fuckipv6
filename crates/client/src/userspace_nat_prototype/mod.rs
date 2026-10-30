//! NX-16 userspace NAT feasibility core.
//!
//! This module is compiled only with the non-default `userspace-nat-prototype`
//! feature. It is not wired into packet forwarding, configuration, CLI, WebUI or
//! GPUI. The production data path must continue to fail closed when this feature
//! is disabled.

mod packet;
mod table;

pub use packet::{MalformedPacket, TcpFlags, TransportProtocol};
pub use table::{
    Direction, Endpoint, FlowKey, FlowSnapshot, MemoryBudget, NatConfig, NatDecision, NatError,
    NatStats, NatTable, TcpConnectionState,
};

#[cfg(test)]
mod tests;
