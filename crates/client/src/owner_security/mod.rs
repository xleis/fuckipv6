//! Owner-only authentication bootstrap.
//!
//! The bootstrap code is never persisted in plaintext. Local and verified
//! public HTTPS channels use distinct WebAuthn origins, and WebAuthn ceremony
//! state remains in memory until the owner record is atomically committed.

pub(crate) mod audit;
pub(crate) mod service;
pub(crate) mod store;
pub(crate) mod webauthn;
