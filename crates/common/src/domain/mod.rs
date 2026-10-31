//! Transport-neutral domain contracts shared by every UI and runtime adapter.
//!
//! The modules in this namespace are the source of truth for state machines,
//! permissions, policies, operations, events, and audit projections. Adapters
//! may translate these types but must not redefine their semantics.

pub mod audit;
pub mod capability;
pub mod coordinator;
pub mod event;
pub mod foundation;
pub mod identity;
pub mod ids;
pub mod membership;
pub mod network;
pub mod operation;
pub mod permission;
pub mod security;
pub mod traffic;
