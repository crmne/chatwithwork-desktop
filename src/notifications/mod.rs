//! Private parent/child notification broker protocol.
//!
//! This module deliberately has no native backend or GUI dependency. The
//! supervisor owns the broker's pipes; the sandboxed daemon never opens an OS
//! notification connection itself.

pub mod helper;
pub mod navigation;
pub mod protocol;
pub mod supervisor;
