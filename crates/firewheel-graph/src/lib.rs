#![cfg_attr(not(feature = "std"), no_std)]

pub mod backend;
mod context;
pub mod error;
pub mod graph;
pub mod processor;

#[cfg(feature = "unsafe_flush_denormals_to_zero")]
mod ftz;

#[cfg(feature = "scheduled_events")]
pub use context::ClearScheduledEventsType;
pub use context::{ContextQueue, FirewheelConfig, FirewheelCtx};

extern crate alloc;
