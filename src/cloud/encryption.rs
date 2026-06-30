//! End-to-end encryption for cloud sync.
//!
//! The implementation now lives in [`crate::sync::encryption`]; this module
//! re-exports it so existing cloud call sites keep compiling unchanged. The
//! cloud path stays live until it is decommissioned in a later phase, at which
//! point this re-export can be deleted along with the rest of the cloud module.

pub use crate::sync::encryption::*;
