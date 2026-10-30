//! Compatibility module for the existing CLI entry point.
//!
//! The native implementation lives in [`crate::gpui`]. This thin re-export keeps
//! the previously public module path stable without maintaining a second UI.

pub use crate::gpui::{is_available, run, NativeNodeSession};
