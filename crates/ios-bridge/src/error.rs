//! Stable numeric statuses shared by every C entry point.

use std::io;

/// Numeric status returned by the C ABI. Values are part of the ABI contract
/// and must match `include/fuckipv6_ios.h`.
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fv6Status {
    /// The call completed; output buffers hold the result.
    Ok = 0,
    /// A pointer, length, option or UTF-8 payload was rejected.
    InvalidArgument = 1,
    /// The handle was null, already released or not produced by this library.
    InvalidHandle = 2,
    /// The runtime is in a state that does not allow the operation.
    State = 3,
    /// The host or the node reported an I/O failure.
    Io = 4,
    /// A bounded wait elapsed. The operation may still be running.
    Timeout = 5,
    /// The packet device was closed.
    Closed = 6,
    /// The output buffer is smaller than the value reported in `out_len`.
    BufferTooSmall = 7,
    /// The outbound queue is full; retry after draining.
    QueueFull = 8,
    /// The bytes are not a complete IP packet accepted by the device.
    InvalidPacket = 9,
    /// A Rust panic was caught at the ABI boundary.
    Panic = 10,
    /// A conflicting operation is already in progress.
    Busy = 11,
    /// The node returned a typed `NodeProblem`; read `fv6_runtime_last_error`.
    NodeProblem = 12,
}

impl Fv6Status {
    pub const fn as_i32(self) -> i32 {
        self as i32
    }
}

/// Maps packet-device I/O failures onto the ABI status domain. The mapping is
/// stable: `WouldBlock` means backpressure, `BrokenPipe` means the tunnel is
/// closed, and invalid packet errors stay distinguishable from transport I/O.
pub fn status_from_io(kind: io::ErrorKind) -> Fv6Status {
    match kind {
        io::ErrorKind::WouldBlock => Fv6Status::QueueFull,
        io::ErrorKind::BrokenPipe
        | io::ErrorKind::NotConnected
        | io::ErrorKind::ConnectionAborted
        | io::ErrorKind::ConnectionReset
        | io::ErrorKind::UnexpectedEof => Fv6Status::Closed,
        io::ErrorKind::InvalidData | io::ErrorKind::InvalidInput => Fv6Status::InvalidPacket,
        io::ErrorKind::TimedOut => Fv6Status::Timeout,
        _ => Fv6Status::Io,
    }
}
