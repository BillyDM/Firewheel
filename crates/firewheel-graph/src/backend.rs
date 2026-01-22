use bevy_platform::prelude::{String, Vec};
use core::error::Error;
use core::time::Duration;

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
    /// The type used to retrieve the list of available audio devices on
    /// the system and their available ocnfigurations.
    type Enumerator;
    /// The configuration of the audio stream.
    type Config: Default;
    /// An error when starting a new audio stream.
    type StartStreamError: Error;
    /// An error that has caused the audio stream to stop.
    type StreamError: Error;
    /// A type describing an instant in time.
    type Instant: Send + Clone;

    /// Get a struct used to retrieve the list of available audio devices
    /// on the system and their available ocnfigurations.
    fn enumerator() -> Self::Enumerator;

    /// Get a list of available input audio devices (for the default API).
    ///
    /// The first item in the list is the default device.
    fn input_devices_simple(&mut self) -> Vec<DeviceInfoSimple> {
        Vec::new()
    }

    /// Get a list of available output audio devices (for the default API).
    ///
    /// The first item in the list is the default device.
    fn output_devices_simple(&mut self) -> Vec<DeviceInfoSimple> {
        Vec::new()
    }

    /// Convert the easy-to-use, backend-agnostic audio stream configuration
    /// into the corresponding backend-specific configuration.
    fn convert_simple_config(&mut self, config: &SimpleStreamConfig) -> Self::Config {
        let _ = config;
        Self::Config::default()
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

/// Basic information about an audio device. It contains the name of the
/// device and a unique identifier that persists across reboots.
#[derive(Default, Debug, Clone, PartialEq)]
#[cfg_attr(feature = "bevy_reflect", derive(bevy_reflect::Reflect))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DeviceInfoSimple {
    /// The display name of this audio device.
    pub name: String,

    /// A unique identifier for the device, serialized into a string.
    ///
    /// This identifier persists across application restarts and system
    /// reboots.
    pub id: String,
}

/// The configuration of an input/output device for a [`SimpleStreamConfig`]
#[derive(Default, Debug, Clone, PartialEq)]
#[cfg_attr(feature = "bevy_reflect", derive(bevy_reflect::Reflect))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SimpleDeviceConfig {
    /// The ID of the device to use. (The ID from [`DeviceInfoSimple::id`].)
    ///
    /// Set to `None` to use the default device.
    ///
    /// By default this is set to `None`.
    pub device: Option<String>,

    /// The number of channels to use. The backend may end up using a different
    /// channel count if the given channel count is not supported.
    ///
    /// Set to `None` to use the default number of channels for the device.
    ///
    /// By default this is set to `None`.
    pub channels: Option<usize>,
}

/// An easy-to-use, backend-agnostic configuration for an audio stream
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "bevy_reflect", derive(bevy_reflect::Reflect))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SimpleStreamConfig {
    /// The configuration of the output audio device
    pub output: SimpleDeviceConfig,

    /// The configuration of the input audio device
    ///
    /// Set to `None` to not connect an input audio device.
    ///
    /// By default this is set to `None`.
    pub input: Option<SimpleDeviceConfig>,

    /// The block size (latency) to use. Set to `None` to use the device's default
    /// block size. The backend may end up using a different block size if the given
    /// block size is not supported.
    ///
    /// By default this is set to `Some(1024)`, which should be good enough for most
    /// games.
    ///
    /// If your application is not a game and doesn't need low latency playback,
    /// then prefer to set this to `None` to reduce system resources.
    pub desired_block_frames: Option<u32>,

    /// The sample rate to use. The backend may end up using a different sample
    /// rate if the given sample rate is not supported.
    ///
    /// Set to `None` to use the device's default sample rate.
    ///
    /// By default this is set to `None`.
    pub desired_sample_rate: Option<u32>,
}

impl Default for SimpleStreamConfig {
    fn default() -> Self {
        Self {
            output: SimpleDeviceConfig::default(),
            input: None,
            desired_block_frames: Some(1024),
            desired_sample_rate: None,
        }
    }
}
