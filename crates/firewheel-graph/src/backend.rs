use core::error::Error;
use core::time::Duration;

#[cfg(not(feature = "std"))]
use bevy_platform::prelude::{String, Vec};

use firewheel_core::{node::StreamStatus, StreamInfo};

use crate::processor::FirewheelProcessor;

/// A trait describing an audio backend.
///
/// When an instance is dropped, then it must automatically stop its
/// corresponding audio stream.
///
/// All methods in this trait are only ever invoked from the main
/// thread (the thread where the [`crate::context::FirewheelCtx`]
/// lives).
pub trait AudioBackend: Sized {
    /// A unique identifier for an audio device that is persistable
    /// across reboots.
    type DeviceID;
    /// An enum of the available audio APIs on the system.
    type AudioAPI;
    /// Extra information for an input audio device.
    type ExtraInputDeviceInfo;
    /// Extra information for an output audio device.
    type ExtraOutputDeviceInfo;
    /// The configuration of the audio stream.
    type Config;
    /// An error when starting a new audio stream.
    type StartStreamError: Error;
    /// An error that has caused the audio stream to stop.
    type StreamError: Error;
    /// A type describing an instant in time.
    type Instant: Send + Clone;

    /// Return a list of the available input devices.
    ///
    /// * `api` - The system audio API to scan. Set to `None` to use the
    /// default API for the system.
    fn available_input_devices(api: Option<Self::AudioAPI>) -> Vec<DeviceInfo<Self::DeviceID>> {
        let _ = api;
        Vec::new()
    }
    /// Return a list of the available output devices.
    ///
    /// * `api` - The system audio API to scan. Set to `None` to use the
    /// default API for the system.
    fn available_output_devices(api: Option<Self::AudioAPI>) -> Vec<DeviceInfo<Self::DeviceID>> {
        let _ = api;
        Vec::new()
    }

    /// Return extra information about the given input device.
    ///
    /// * `device_id` - The unique identifier of the input audio device.
    /// * `api` - The system audio API this device belongs to. Set to `None`
    /// if using the default API for the system.
    ///
    /// Returns `None` if a device with the given ID was not found.
    fn extra_input_device_info(
        device_id: &Self::DeviceID,
        api: Option<Self::AudioAPI>,
    ) -> Option<Self::ExtraInputDeviceInfo> {
        let _ = device_id;
        let _ = api;
        None
    }
    /// Return extra information about the given output device.
    ///
    /// * `device_id` - The unique identifier of the input audio device.
    /// * `api` - The system audio API this device belongs to. Set to `None`
    /// if using the default API for the system.
    ///
    /// Returns `None` if a device with the given ID was not found.
    fn extra_output_device_info(
        device_id: &Self::DeviceID,
        api: Option<Self::AudioAPI>,
    ) -> Option<Self::ExtraOutputDeviceInfo> {
        let _ = device_id;
        let _ = api;
        None
    }

    /// Start the audio stream with the given configuration, and return
    /// a handle for the audio stream.
    fn start_stream(config: Self::Config) -> Result<(Self, StreamInfo), Self::StartStreamError>;

    /// Send the given processor to the audio thread for processing.
    fn set_processor(&mut self, processor: FirewheelProcessor<Self>);

    /// Poll the status of the running audio stream. Return an error if the
    /// audio stream has stopped for any reason.
    fn poll_status(&mut self) -> Result<(), Self::StreamError>;

    /// Return the amount of time that has elapsed from the instant
    /// [`FirewheelProcessor::process_interleaved`] was last called and now.
    ///
    /// The given `process_timestamp` is the `Self::Instant` that was passed
    /// to the latest call to [`FirewheelProcessor::process_interleaved`].
    /// This can be used to calculate the delay if needed.
    ///
    /// If for any reason the delay could not be determined, return `None`.
    fn delay_from_last_process(&self, process_timestamp: Self::Instant) -> Option<Duration>;
}

/// Information about an audio device.
#[derive(Debug, Clone, PartialEq)]
pub struct DeviceInfo<DeviceID> {
    pub id: DeviceID,
    pub name: Option<String>,
    pub is_default: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendProcessInfo<B: AudioBackend> {
    pub num_in_channels: usize,
    pub num_out_channels: usize,
    pub frames: usize,
    pub process_timestamp: B::Instant,
    pub duration_since_stream_start: Duration,
    pub input_stream_status: StreamStatus,
    pub output_stream_status: StreamStatus,
    pub dropped_frames: u32,
}
