//! Owner-only local authentication bootstrap.
//!
//! The bootstrap code is accepted only from local loopback callers and is
//! never persisted in plaintext. WebAuthn ceremony state remains in memory
//! until the owner record is atomically committed.

pub(crate) mod service;
pub(crate) mod store;
pub(crate) mod webauthn;
