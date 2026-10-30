#[cfg(target_os = "linux")]
pub mod doh_dns;
#[cfg(target_os = "linux")]
pub mod linux_exit;
#[cfg(target_os = "linux")]
pub mod linux_route;

pub mod exit_node;
pub mod table;

pub use exit_node::*;
#[cfg(target_os = "linux")]
pub use linux_exit::*;
