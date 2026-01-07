#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(not(feature = "std"))]
use bevy_platform::prelude::String;

pub mod atomic_float;
pub mod channel_config;
pub mod clock;
pub mod diff;
pub mod dsp;
pub mod event;
pub mod log;
pub mod mask;
pub mod node;
pub mod param;
pub mod sample_resource;
pub mod vector;

pub use rtgc as collector;

use core::num::NonZeroU32;

extern crate self as firewheel_core;

/// Information about a running audio stream.
#[derive(Debug, Clone, PartialEq)]
pub struct StreamInfo {
    pub sample_rate: NonZeroU32,
    /// The reciprocal of the sample rate.
    pub sample_rate_recip: f64,
    /// The sample rate of the previous stream. (If this is the first stream, then this
    /// will just be a copy of [`StreamInfo::sample_rate`]).
    pub prev_sample_rate: NonZeroU32,
    pub max_block_frames: NonZeroU32,
    pub num_stream_in_channels: u32,
    pub num_stream_out_channels: u32,
    /// The latency of the input to output stream in seconds.
    pub input_to_output_latency_seconds: f64,
    pub declick_frames: NonZeroU32,
    /// The identifier of the output audio device (converted to a string).
    pub output_device_id: String,
    /// The identifier of the input audio device (converted to a string).
    pub input_device_id: Option<String>,
}

impl Default for StreamInfo {
    fn default() -> Self {
        Self {
            sample_rate: NonZeroU32::new(44100).unwrap(),
            sample_rate_recip: 44100.0f64.recip(),
            prev_sample_rate: NonZeroU32::new(44100).unwrap(),
            max_block_frames: NonZeroU32::new(1024).unwrap(),
            num_stream_in_channels: 0,
            num_stream_out_channels: 2,
            input_to_output_latency_seconds: 0.0,
            declick_frames: NonZeroU32::MIN,
            output_device_id: String::new(),
            input_device_id: None,
        }
    }
}
