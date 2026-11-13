//! Shared product-domain contracts for node control, network governance and
//! long-running operations.
//!
//! These types are transport-neutral and intentionally contain no plaintext
//! secret material. Adapters may project them into HTTP, SSE, IPC, GPUI,
//! WebUI or Android DTOs, but must not define a second state machine.

use std::error::Error;
use std::fmt;

pub mod access_control;
pub mod application_identity;
pub mod audit;
pub mod capability;
pub mod coordinator;
pub mod event;
pub mod exit_contribution;
pub mod exit_health;
pub mod exit_selection;
pub mod field_privacy;
pub mod file_share;
pub mod groups;
pub mod ids;
pub mod invitation;
pub mod leak_detection;
pub mod membership;
pub mod membership_lifecycle;
pub mod network;
pub mod node;
pub mod operation;
pub mod permissions;
pub mod policy_test;
pub mod relay;
pub mod security_policy;
pub mod topology;
pub mod traffic_failure;
pub mod traffic_policy;
pub mod traffic_policy_test;
pub mod traffic_routing_page;
pub mod traffic_session;

pub use access_control::*;
pub use application_identity::*;
pub use audit::*;
pub use capability::*;
pub use coordinator::*;
pub use event::*;
pub use exit_contribution::*;
pub use exit_health::*;
pub use exit_selection::*;
pub use field_privacy::*;
pub use file_share::*;
pub use groups::*;
pub use ids::*;
pub use invitation::*;
pub use leak_detection::*;
pub use membership::*;
pub use membership_lifecycle::*;
pub use network::*;
pub use node::*;
pub use operation::*;
pub use permissions::*;
pub use policy_test::*;
pub use relay::*;
pub use security_policy::*;
pub use topology::*;
pub use traffic_failure::*;
pub use traffic_policy::*;
pub use traffic_policy_test::*;
pub use traffic_routing_page::*;
pub use traffic_session::*;

/// Stable, machine-readable domain violation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DomainError {
    code: &'static str,
    message: String,
}

impl DomainError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn invalid_id(message: impl Into<String>) -> Self {
        Self::new("domain.invalid_id", message)
    }

    pub fn invalid_secret_ref(message: impl Into<String>) -> Self {
        Self::new("domain.invalid_secret_ref", message)
    }

    pub fn invalid_value(message: impl Into<String>) -> Self {
        Self::new("domain.invalid_value", message)
    }

    pub fn invalid_transition(message: impl Into<String>) -> Self {
        Self::new("domain.invalid_transition", message)
    }

    pub fn invariant(message: impl Into<String>) -> Self {
        Self::new("domain.invariant_violation", message)
    }

    pub fn code(&self) -> &'static str {
        self.code
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for DomainError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl Error for DomainError {}

pub type DomainResult<T> = Result<T, DomainError>;
