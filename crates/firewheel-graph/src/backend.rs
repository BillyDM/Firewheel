use core::error::Error;
use core::time::Duration;

use firewheel_core::StreamInfo;

use crate::processor::FirewheelProcessor;

/// A trait describing an audio backend.
///
/// When an instance is dropped, then it must automatically stop its
/// corresponding audio stream.
pub trait AudioBackend: Sized {
    /// The configuration of the audio stream.
    type Config;
    /// An error when starting a new audio stream.
    type StartStreamError: Error;
    /// An error that has caused the audio stream to stop.
    type StreamError: Error;

    /// A type describing an instant in time.
    type Instant: Send + Clone;

    /// Return a list of the available input devices.
    fn available_input_devices() -> Vec<DeviceInfo> {
        Vec::new()
    }
    /// Return a list of the available output devices.
    fn available_output_devices() -> Vec<DeviceInfo> {
        Vec::new()
    }

    /// Start the audio stream with the given configuration, and return
    /// a handle for the audio stream.
    fn start_stream(config: Self::Config) -> Result<(Self, StreamInfo), Self::StartStreamError>;

    /// Send the given processor to the audio thread for processing.
    ///
    /// This is called once after a successful call to `start_stream`.
    fn set_processor(&mut self, processor: FirewheelProcessor<Self>);

    /// Poll the status of the running audio stream. Return an error if the
    /// audio stream has stopped for any reason.
    fn poll_status(&mut self) -> Result<(), Self::StreamError>;

    /// Get the current time.
    fn now(&self) -> Self::Instant;

    /// Get the elapsed time between the two given instants.
    ///
    /// If `earlier` is greater than `later`, then return `None`.
    fn duration_between(&self, earlier: Self::Instant, later: Self::Instant) -> Option<Duration>;
}

/// Information about an audio device.
#[derive(Debug, Clone, PartialEq)]
pub struct DeviceInfo {
    pub name: String,
    pub num_channels: u16,
    pub is_default: bool,
}
