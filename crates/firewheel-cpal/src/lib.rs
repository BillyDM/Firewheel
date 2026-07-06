use core::{fmt::Debug, num::NonZeroU32, time::Duration};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};

use audioadapter_buffers::direct::InterleavedSlice;
pub use cpal;

use bevy_platform::time::Instant;
pub use cpal::{DeviceId, HostId};
use cpal::{
    SampleFormat,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};
use firewheel_core::node::StreamStatus;
use firewheel_graph::{
    ActivateInfo, FirewheelContext,
    backend::BackendProcessInfo,
    error::{ActivateError, CompileGraphError},
    processor::FirewheelProcessor,
};
use fixed_resample::{ReadStatus, ResamplingChannelConfig, ResamplingProd};

#[cfg(all(feature = "log", not(feature = "tracing")))]
use log::{error, info, warn};
#[cfg(feature = "tracing")]
use tracing::{error, info, warn};

/// 1024 samples is a latency of about 23 milliseconds, which should
/// be good enough for most games.
const DEFAULT_MAX_BLOCK_FRAMES: u32 = 1024;
const BUILD_STREAM_TIMEOUT: Duration = Duration::from_secs(5);
const UNDERRUN_LOG_COOLDOWN: Duration = Duration::from_secs(3);

/// The configuration of an output audio stream in the CPAL backend.
#[derive(Debug, Clone, PartialEq)]
pub struct CpalOutputConfig {
    /// The host to use. Set to `None` to use the
    /// system's default audio host.
    pub host: Option<cpal::HostId>,

    /// The id of the output device to use. Set to `None` to use the
    /// system's default output device.
    ///
    /// By default this is set to `None`.
    pub device_id: Option<DeviceId>,

    /// The desired sample rate to use. Set to `None` to use the device's
    /// default sample rate.
    ///
    /// By default this is set to `None`.
    pub desired_sample_rate: Option<u32>,

    /// The latency/block size of the audio stream to use. Set to
    /// `None` to use the device's default value.
    ///
    /// Smaller values may give better latency, but is not supported on
    /// all platforms and may lead to performance issues.
    ///
    /// This currently has no effect on iOS platforms.
    ///
    /// By default this is set to `Some(1024)`.
    pub desired_block_frames: Option<u32>,

    /// Whether or not to fall back to the default device  if a device
    /// with the given configuration could not be found.
    ///
    /// By default this is set to `true`.
    pub fallback: bool,
}

impl Default for CpalOutputConfig {
    fn default() -> Self {
        Self {
            host: None,
            device_id: None,
            desired_sample_rate: None,
            desired_block_frames: Some(DEFAULT_MAX_BLOCK_FRAMES),
            fallback: true,
        }
    }
}

/// The configuration of an input audio stream in the CPAL backend.
#[derive(Debug, Clone, PartialEq)]
pub struct CpalInputConfig {
    /// The host to use. Set to `None` to use the
    /// system's default audio host.
    pub host: Option<cpal::HostId>,

    /// The id of the input device to use. Set to `None` to use the
    /// system's default input device.
    ///
    /// By default this is set to `None`.
    pub device_id: Option<DeviceId>,

    /// The latency/block size of the audio stream to use. Set to
    /// `None` to use the device's default value.
    ///
    /// Smaller values may give better latency, but is not supported on
    /// all platforms and may lead to performance issues.
    ///
    /// This currently has no effect on iOS platforms.
    ///
    /// By default this is set to `Some(1024)`.
    pub desired_block_frames: Option<u32>,

    /// The configuration of the input to output stream channel.
    pub channel_config: ResamplingChannelConfig,

    /// Whether or not to fall back to the default device  if a device
    /// with the given configuration could not be found.
    ///
    /// By default this is set to `true`.
    pub fallback: bool,

    /// If `true`, then an error will be returned if an input stream could
    /// not be started. If `false`, then the output stream will still
    /// attempt to start with no input stream.
    ///
    /// By default this is set to `false`.
    pub fail_on_no_input: bool,
}

impl Default for CpalInputConfig {
    fn default() -> Self {
        Self {
            host: None,
            device_id: None,
            desired_block_frames: Some(DEFAULT_MAX_BLOCK_FRAMES),
            channel_config: ResamplingChannelConfig::default(),
            fallback: true,
            fail_on_no_input: false,
        }
    }
}

/// The configuration of a CPAL stream.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct CpalConfig {
    /// The configuration of the output stream.
    pub output: CpalOutputConfig,

    /// The configuration of the input stream.
    ///
    /// Set to `None` for no input stream.
    ///
    /// By default this is set to `None`.
    pub input: Option<CpalInputConfig>,
}

/// A struct used to retrieve the list of available audio devices
/// for a given system audio host (API).
pub struct HostEnumerator {
    pub host: cpal::Host,
}

impl HostEnumerator {
    /// The system backend host id (API) this enumerator is using.
    pub fn host_id(&self) -> cpal::HostId {
        self.host.id()
    }

    /// Get the list of available input audio devices.
    pub fn input_devices(&self) -> Vec<DeviceInfo> {
        let mut devices = Vec::with_capacity(8);

        let default_device = self.host.default_input_device();
        let default_device_id = default_device.and_then(|d| match d.id() {
            Ok(id) => Some(id),
            Err(e) => {
                #[cfg(any(feature = "log", feature = "tracing"))]
                warn!("Failed to get ID of default audio input device: {}", e);
                #[cfg(not(any(feature = "log", feature = "tracing")))]
                let _ = e;
                None
            }
        });

        match self.host.input_devices() {
            Ok(input_devices) => {
                for device in input_devices {
                    let Ok(id) = device.id() else {
                        continue;
                    };

                    let is_default = if let Some(default_device_id) = &default_device_id {
                        &id == default_device_id
                    } else {
                        false
                    };

                    let name = device.description().map(|d| d.name().to_string()).ok();

                    devices.push(DeviceInfo {
                        id,
                        name,
                        is_default,
                    })
                }
            }
            Err(e) => {
                #[cfg(any(feature = "log", feature = "tracing"))]
                error!("Failed to get input audio devices: {}", e);
                #[cfg(not(any(feature = "log", feature = "tracing")))]
                let _ = e;
            }
        }

        devices
    }

    /// Get the list of available output audio devices.
    pub fn output_devices(&self) -> Vec<DeviceInfo> {
        let mut devices = Vec::with_capacity(8);

        let default_device = self.host.default_output_device();
        let default_device_id = default_device.and_then(|d| match d.id() {
            Ok(id) => Some(id),
            Err(e) => {
                #[cfg(any(feature = "log", feature = "tracing"))]
                warn!("Failed to get ID of default audio output device: {}", e);
                #[cfg(not(any(feature = "log", feature = "tracing")))]
                let _ = e;
                None
            }
        });

        match self.host.output_devices() {
            Ok(output_devices) => {
                for device in output_devices {
                    let Ok(id) = device.id() else {
                        continue;
                    };

                    let is_default = if let Some(default_device_id) = &default_device_id {
                        &id == default_device_id
                    } else {
                        false
                    };

                    let name = device.description().map(|d| d.name().to_string()).ok();

                    devices.push(DeviceInfo {
                        id,
                        name,
                        is_default,
                    })
                }
            }
            Err(e) => {
                #[cfg(any(feature = "log", feature = "tracing"))]
                error!("Failed to get output audio devices: {}", e);
                #[cfg(not(any(feature = "log", feature = "tracing")))]
                let _ = e;
            }
        }

        devices
    }

    /// Get a struct used to retrieve extra information for the given audio
    /// device.
    ///
    /// Returns `None` if the device could not be found.
    pub fn get_device(&self, device_id: &cpal::DeviceId) -> Option<cpal::Device> {
        self.host.device_by_id(device_id)
    }
}

/// Information about an audio device.
#[derive(Debug, Clone, PartialEq)]
pub struct DeviceInfo {
    /// A stable identifier for an audio device across all supported platforms.
    ///
    /// Device IDs should remain stable across application restarts and can be
    /// serialized using `Display`/`FromStr`.
    ///
    /// A device ID consists of a [`HostId`] identifying the audio backend and
    /// a device-specific identifier string.
    pub id: cpal::DeviceId,
    /// The display name of the device.
    pub name: Option<String>,
    /// Whether or not this device is the default input/output device.
    pub is_default: bool,
}

/// Information about a running CPAL audio stream.
#[derive(Debug, Clone, PartialEq)]
pub struct CpalStreamInfo {
    /// The sample rate of the audio stream.
    pub sample_rate: NonZeroU32,
    /// The maximum number of frames that can appear in a single process cyle.
    pub max_block_frames: NonZeroU32,
    /// The number of input audio channels in the stream.
    pub num_stream_in_channels: u32,
    /// The number of output audio channels in the stream.
    pub num_stream_out_channels: u32,
    /// The latency of the input to output stream in seconds.
    pub input_to_output_latency_seconds: f64,
    /// The ID of the output audio device.
    pub out_device_id: Option<DeviceId>,
    /// The ID of the input audio device.
    pub in_device_id: Option<DeviceId>,
}

/// The system audio hosts (APIs) that are available on this system.
///
/// The first host in the list is the default one for the system.
pub fn available_hosts() -> Vec<cpal::HostId> {
    cpal::available_hosts()
}

/// Get a struct used to retrieve the list of available audio devices
/// for the default system audio host (API).
pub fn default_host_enumerator() -> HostEnumerator {
    HostEnumerator {
        host: cpal::default_host(),
    }
}

/// Get a struct used to retrieve the list of available audio devices
/// for the given system audio host (API).
pub fn host_enumerator(api: HostId) -> Result<HostEnumerator, cpal::Error> {
    cpal::host_from_id(api).map(|host| HostEnumerator { host })
}

/// A CPAL stream running a [`FirewheelProcessor`].
///
/// The audio stream is automatically stopped when this struct is dropped.
pub struct CpalStream {
    _out_stream_handle: cpal::Stream,
    _in_stream_handle: Option<cpal::Stream>,
    from_err_rx: mpsc::Receiver<IoStreamError>,
    stream_info: CpalStreamInfo,
    input_stream_running: Option<Arc<AtomicBool>>,
    output_stream_running: Arc<AtomicBool>,
}

impl CpalStream {
    /// Create a new audio stream with the given [`FirewheelContext`].
    pub fn new(cx: &mut FirewheelContext, config: CpalConfig) -> Result<Self, StartStreamError> {
        #[cfg(any(feature = "log", feature = "tracing"))]
        info!("Attempting to start CPAL audio stream...");

        if cx.is_active() {
            return Err(StartStreamError::AlreadyActive);
        }

        let host = if let Some(host_id) = config.output.host {
            match cpal::host_from_id(host_id) {
                Ok(host) => host,
                Err(e) => {
                    #[cfg(any(feature = "log", feature = "tracing"))]
                    warn!(
                        "Requested audio host {:?} is not available: {}. Falling back to default host...",
                        &host_id, e
                    );
                    #[cfg(not(any(feature = "log", feature = "tracing")))]
                    let _ = e;
                    cpal::default_host()
                }
            }
        } else {
            cpal::default_host()
        };

        let mut out_device = None;
        if let Some(device_id) = &config.output.device_id {
            if let Some(device) = host.device_by_id(device_id)
                && device.supports_output()
            {
                out_device = Some(device);
            }

            if out_device.is_none() {
                #[cfg(any(feature = "log", feature = "tracing"))]
                warn!(
                    "Could not find requested audio output device: {}. Falling back to default device...",
                    &device_id
                );
            }
        }

        if out_device.is_none() {
            let Some(default_device) = host.default_output_device() else {
                return Err(StartStreamError::DefaultOutputDeviceNotFound);
            };
            out_device = Some(default_device);
        }
        let out_device = out_device.unwrap();

        let out_device_id = match out_device.id() {
            Ok(id) => Some(id),
            Err(e) => {
                #[cfg(any(feature = "log", feature = "tracing"))]
                warn!("Failed to get id of output audio device: {}", e);
                #[cfg(not(any(feature = "log", feature = "tracing")))]
                let _ = e;
                None
            }
        };

        let default_config = out_device
            .default_output_config()
            .map_err(StartStreamError::FailedToGetConfig)?;

        let default_sample_rate = default_config.sample_rate();
        // Try to use the common sample rates by default.
        let try_common_sample_rates = default_sample_rate != 44100 && default_sample_rate != 48000;

        #[cfg(not(target_os = "ios"))]
        let desired_block_frames =
            if let &cpal::SupportedBufferSize::Range { min, max } = default_config.buffer_size() {
                config
                    .output
                    .desired_block_frames
                    .map(|f| f.clamp(min, max))
            } else {
                None
            };

        // For some reason fixed buffer sizes on iOS doesn't work in CPAL.
        // I'm not sure if this is a problem on CPAL's end, but I have disabled
        // it for the time being.
        #[cfg(target_os = "ios")]
        let desired_block_frames: Option<u32> = None;

        let mut supports_desired_sample_rate = false;
        let mut supports_44100 = false;
        let mut supports_48000 = false;

        if config.output.desired_sample_rate.is_some() || try_common_sample_rates {
            for cpal_config in out_device
                .supported_output_configs()
                .map_err(StartStreamError::FailedToGetConfig)?
            {
                if let Some(sr) = config.output.desired_sample_rate
                    && !supports_desired_sample_rate
                    && cpal_config.try_with_sample_rate(sr).is_some()
                {
                    supports_desired_sample_rate = true;
                    break;
                }

                if try_common_sample_rates {
                    if !supports_44100 {
                        supports_44100 = cpal_config.try_with_sample_rate(44100).is_some();
                    }
                    if !supports_48000 {
                        supports_48000 = cpal_config.try_with_sample_rate(48000).is_some();
                    }
                }
            }
        }

        let sample_rate = if supports_desired_sample_rate {
            config.output.desired_sample_rate.unwrap()
        } else if try_common_sample_rates {
            if supports_44100 {
                44100
            } else if supports_48000 {
                48000
            } else {
                default_sample_rate
            }
        } else {
            default_sample_rate
        };

        let num_out_channels = default_config.channels() as usize;
        assert_ne!(num_out_channels, 0);

        let desired_buffer_size = if let Some(samples) = desired_block_frames {
            cpal::BufferSize::Fixed(samples)
        } else {
            cpal::BufferSize::Default
        };

        let out_stream_config = cpal::StreamConfig {
            channels: num_out_channels as u16,
            sample_rate,
            buffer_size: desired_buffer_size,
        };

        let max_block_frames = match out_stream_config.buffer_size {
            cpal::BufferSize::Default => DEFAULT_MAX_BLOCK_FRAMES as usize,
            cpal::BufferSize::Fixed(f) => f as usize,
        };

        let (err_to_cx_tx, from_err_rx) = mpsc::channel();

        let mut input_stream = StartInputStreamResult::NotStarted;
        if let Some(input_config) = &config.input {
            input_stream = start_input_stream(
                input_config,
                out_stream_config.sample_rate,
                err_to_cx_tx.clone(),
            )?;
        }

        let (
            in_stream_handle,
            input_stream_cons,
            num_stream_in_channels,
            in_device_id,
            input_to_output_latency_seconds,
            input_stream_running,
        ) = if let StartInputStreamResult::Started {
            stream_handle,
            cons,
            num_stream_in_channels,
            in_device_id,
            input_stream_running,
        } = input_stream
        {
            let input_to_output_latency_seconds = cons.latency_seconds();

            (
                Some(stream_handle),
                Some(cons),
                num_stream_in_channels,
                in_device_id,
                input_to_output_latency_seconds,
                Some(input_stream_running),
            )
        } else {
            (None, None, 0, None, 0.0, None)
        };

        let activate_info = ActivateInfo {
            sample_rate: NonZeroU32::new(out_stream_config.sample_rate).unwrap(),
            max_block_frames: NonZeroU32::new(max_block_frames as u32).unwrap(),
            num_stream_in_channels,
            num_stream_out_channels: num_out_channels as u32,
            input_to_output_latency_seconds,
        };

        let processor = cx.activate(activate_info)?;

        let output_stream_running = Arc::new(AtomicBool::new(true));

        let mut callback = OutputCallback::new(
            num_out_channels,
            max_block_frames,
            out_stream_config.sample_rate,
            processor,
            input_stream_cons,
            err_to_cx_tx.clone(),
            input_stream_running.as_ref().map(Arc::clone),
            Arc::clone(&output_stream_running),
        );

        let out_sample_format = default_config.sample_format();

        #[cfg(any(feature = "log", feature = "tracing"))]
        info!(
            "Starting output audio stream with device \"{:?}\" with configuration {:?} (native sample format: {:?})",
            &out_device_id, &out_stream_config, out_sample_format,
        );

        let scratch_capacity = max_block_frames * num_out_channels;

        #[cfg(any(
            target_os = "linux",
            target_os = "freebsd",
            target_os = "dragonfly",
            target_os = "netbsd"
        ))]
        let is_alsa = host.id() == HostId::Alsa;

        macro_rules! build_output_streams {
            ($sample_format:expr, $(($format:path, $primitive_type:ty)),*) => {
                match $sample_format {
                    $($format => {
                        let mut scratch = scratch_vec(scratch_capacity);

                        out_device.build_output_stream(
                            out_stream_config,
                            move |output: &mut [$primitive_type], info: &cpal::OutputCallbackInfo| {
                                for out_chunk in output.chunks_mut(scratch_capacity) {
                                    let buf = &mut scratch[..out_chunk.len()];

                                    callback.callback(buf, info);

                                    for (o, &f) in out_chunk.iter_mut().zip(buf.iter()) {
                                        // TODO: Add dithering option for better quality?
                                        *o = <$primitive_type as cpal::FromSample<f32>>::from_sample_(f);
                                    }
                                }
                            },
                            err_callback(
                                false,
                                output_stream_running.clone(),
                                err_to_cx_tx.clone(),
                                #[cfg(any(
                                    target_os = "linux",
                                    target_os = "freebsd",
                                    target_os = "dragonfly",
                                    target_os = "netbsd"
                                ))]
                                is_alsa
                            ),
                            Some(BUILD_STREAM_TIMEOUT),
                        )
                    },)*
                    _ => unreachable!(),
                }
            }
        }

        // The cpal ASIO backend requires the callback buffer type to match the
        // driver's native format (unlike WASAPI, which converts internally).
        // For non-f32 formats, render into an f32 scratch buffer and convert
        // on the way out. The f32 path stays a direct passthrough.
        let out_stream_handle = if let SampleFormat::F32 = out_sample_format {
            out_device.build_output_stream(
                out_stream_config,
                move |output: &mut [f32], info: &cpal::OutputCallbackInfo| {
                    callback.callback(output, info);
                },
                err_callback(
                    false,
                    output_stream_running.clone(),
                    err_to_cx_tx.clone(),
                    #[cfg(any(
                        target_os = "linux",
                        target_os = "freebsd",
                        target_os = "dragonfly",
                        target_os = "netbsd"
                    ))]
                    is_alsa,
                ),
                Some(BUILD_STREAM_TIMEOUT),
            )
        } else {
            build_output_streams!(
                out_sample_format,
                (SampleFormat::I8, i8),
                (SampleFormat::I16, i16),
                (SampleFormat::I32, i32),
                (SampleFormat::I64, i64),
                (SampleFormat::U8, u8),
                (SampleFormat::U16, u16),
                (SampleFormat::U32, u32),
                (SampleFormat::U64, u64),
                (SampleFormat::F64, f64)
            )
        }
        .map_err(StartStreamError::BuildStreamError)?;

        if let Err(e) = out_stream_handle.play() {
            if let cpal::ErrorKind::RealtimeDenied = e.kind() {
                #[cfg(any(feature = "log", feature = "tracing"))]
                warn!(
                    "Failed to get realtime priority for output audio thread. This may increase latency and/or audio glitches"
                );
            } else {
                return Err(StartStreamError::PlayStreamError(e));
            }
        }

        let stream_info = CpalStreamInfo {
            sample_rate: activate_info.sample_rate,
            max_block_frames: activate_info.max_block_frames,
            num_stream_in_channels: activate_info.num_stream_in_channels,
            num_stream_out_channels: activate_info.num_stream_out_channels,
            input_to_output_latency_seconds: activate_info.input_to_output_latency_seconds,
            out_device_id,
            in_device_id,
        };

        Ok(Self {
            _out_stream_handle: out_stream_handle,
            _in_stream_handle: in_stream_handle,
            from_err_rx,
            stream_info,
            input_stream_running,
            output_stream_running,
        })
    }

    /// Information about the running audio stream
    pub fn info(&self) -> &CpalStreamInfo {
        &self.stream_info
    }

    /// Poll the status of the audio stream and log any errors/warnings that have occurred.
    ///
    /// Note, if an error is returned, it doesn't always mean that the stream has stopped.
    /// Instead, use [`CpalStream::all_streams_ok()`] to check if the stream is still running
    /// or if the stream needs to be recreated.
    pub fn poll_status(&mut self) -> mpsc::TryIter<'_, IoStreamError> {
        if self._in_stream_handle.is_some() && !self.input_stream_ok() {
            self._in_stream_handle = None;
        }

        self.from_err_rx.try_iter()
    }

    /// Log any stream errors/warnings that have occurred.
    ///
    /// Same as [`CpalStream::poll_status`], but automatically logs all of the errors/
    /// warnings to the log output.
    #[cfg(any(feature = "log", feature = "tracing"))]
    pub fn log_status(&mut self) {
        for e in self.from_err_rx.try_iter() {
            error!("Audio stream error occurred: {}", e);
        }
    }

    /// Returns `true` if the output audio stream is still running.
    ///
    /// Returns `false` if the stream has stopped unexpectedly (i.e. an audio device
    /// was disconnected). When this happens, this `CpalStream` instance should be dropped,
    /// and a new one created.
    pub fn output_stream_ok(&self) -> bool {
        self.output_stream_running.load(Ordering::Relaxed)
    }

    /// Returns `true` if the input audio stream is still running or if an input audio
    /// stream was never created.
    ///
    /// Returns `false` if there is no input stream, or if the input stream has stopped
    /// unexpectedly (i.e. an audio device was disconnected). When this happens, this
    /// `CpalStream` instance should be dropped, and a new one created.
    pub fn input_stream_ok(&self) -> bool {
        self.input_stream_running
            .as_ref()
            .map(|r| r.load(Ordering::Relaxed))
            .unwrap_or(true)
    }

    /// Returns `true` if the all audio streams (input and/or output) are still running.
    ///
    /// Returns `false` if any audio stream has stopped unexpectedly (i.e. an audio device
    /// was disconnected). When this happens, this `CpalStream` instance should be dropped,
    /// and a new one created.
    pub fn all_streams_ok(&self) -> bool {
        self.output_stream_ok() && self.input_stream_ok()
    }
}

impl Drop for CpalStream {
    fn drop(&mut self) {
        // Make sure any remaining errors/warnings get logged.
        #[cfg(any(feature = "log", feature = "tracing"))]
        self.log_status();
    }
}

fn start_input_stream(
    config: &CpalInputConfig,
    output_sample_rate: cpal::SampleRate,
    err_to_cx_tx: mpsc::Sender<IoStreamError>,
) -> Result<StartInputStreamResult, StartStreamError> {
    let host = if let Some(host_id) = config.host {
        match cpal::host_from_id(host_id) {
            Ok(host) => host,
            Err(e) => {
                #[cfg(any(feature = "log", feature = "tracing"))]
                warn!(
                    "Requested audio host {:?} is not available: {}. Falling back to default host...",
                    &host_id, e
                );
                #[cfg(not(any(feature = "log", feature = "tracing")))]
                let _ = e;
                cpal::default_host()
            }
        }
    } else {
        cpal::default_host()
    };

    let mut in_device = None;
    if let Some(device_id) = &config.device_id {
        if let Some(device) = host.device_by_id(device_id)
            && device.supports_input()
        {
            in_device = Some(device);
        }

        if in_device.is_none() {
            if config.fallback {
                #[cfg(any(feature = "log", feature = "tracing"))]
                warn!(
                    "Could not find requested audio input device: {}. Falling back to default device...",
                    &device_id
                );
            } else {
                #[cfg(any(feature = "log", feature = "tracing"))]
                warn!(
                    "Could not find requested audio input device: {}. No input stream will be started.",
                    &device_id
                );
                return Ok(StartInputStreamResult::NotStarted);
            }
        }
    }

    if in_device.is_none() {
        if let Some(default_device) = host.default_input_device() {
            in_device = Some(default_device);
        } else if config.fail_on_no_input {
            return Err(StartStreamError::DefaultInputDeviceNotFound);
        } else {
            #[cfg(any(feature = "log", feature = "tracing"))]
            warn!("No default audio input device found. Input stream will not be started.");
            return Ok(StartInputStreamResult::NotStarted);
        }
    }
    let in_device = in_device.unwrap();

    let in_device_id = match in_device.id() {
        Ok(id) => Some(id),
        Err(e) => {
            #[cfg(any(feature = "log", feature = "tracing"))]
            warn!("Failed to get id of input audio device: {}", e);
            #[cfg(not(any(feature = "log", feature = "tracing")))]
            let _ = e;
            None
        }
    };

    let default_config = in_device
        .default_input_config()
        .map_err(StartStreamError::FailedToGetConfig)?;

    #[cfg(not(target_os = "ios"))]
    let desired_block_frames =
        if let &cpal::SupportedBufferSize::Range { min, max } = default_config.buffer_size() {
            config.desired_block_frames.map(|f| f.clamp(min, max))
        } else {
            None
        };

    // For some reason fixed buffer sizes on iOS doesn't work in CPAL.
    // I'm not sure if this is a problem on CPAL's end, but I have disabled
    // it for the time being.
    #[cfg(target_os = "ios")]
    let desired_block_frames: Option<u32> = None;

    let supported_configs = in_device
        .supported_input_configs()
        .map_err(StartStreamError::FailedToGetConfig)?;

    let mut min_sample_rate = u32::MAX;
    let mut max_sample_rate = 0;
    for config in supported_configs.into_iter() {
        min_sample_rate = min_sample_rate.min(config.min_sample_rate());
        max_sample_rate = max_sample_rate.max(config.max_sample_rate());
    }
    let sample_rate = output_sample_rate.clamp(min_sample_rate, max_sample_rate);

    #[cfg(not(feature = "resample_inputs"))]
    if sample_rate != output_sample_rate {
        if config.fail_on_no_input {
            return Err(StartStreamError::CouldNotMatchSampleRate(
                output_sample_rate,
            ));
        } else {
            #[cfg(any(feature = "log", feature = "tracing"))]
            warn!(
                "Could not use output sample rate {} for the input sample rate. Input stream will not be started",
                output_sample_rate
            );
            return Ok(StartInputStreamResult::NotStarted);
        }
    }

    let num_in_channels = default_config.channels() as usize;
    assert_ne!(num_in_channels, 0);

    let desired_buffer_size = if let Some(samples) = desired_block_frames {
        cpal::BufferSize::Fixed(samples)
    } else {
        cpal::BufferSize::Default
    };

    let max_block_frames = match desired_buffer_size {
        cpal::BufferSize::Default => DEFAULT_MAX_BLOCK_FRAMES as usize,
        cpal::BufferSize::Fixed(f) => f as usize,
    };

    let stream_config = cpal::StreamConfig {
        channels: num_in_channels as u16,
        sample_rate,
        buffer_size: desired_buffer_size,
    };

    let (prod, cons) = fixed_resample::resampling_channel::<f32>(
        num_in_channels,
        sample_rate,
        output_sample_rate,
        true,
        config.channel_config,
    );

    let input_stream_running = Arc::new(AtomicBool::new(true));

    #[cfg(any(feature = "log", feature = "tracing"))]
    info!(
        "Starting input audio stream with device \"{:?}\" with configuration {:?}",
        &in_device_id, &stream_config
    );

    let mut callback = InputCallback {
        prod,
        err_to_cx_tx: err_to_cx_tx.clone(),
        input_stream_running: Arc::clone(&input_stream_running),
    };

    let in_sample_format = default_config.sample_format();

    let scratch_capacity = max_block_frames * num_in_channels;

    #[cfg(any(
        target_os = "linux",
        target_os = "freebsd",
        target_os = "dragonfly",
        target_os = "netbsd"
    ))]
    let is_alsa = host.id() == HostId::Alsa;

    macro_rules! build_input_stream {
        ($sample_format:expr, $(($format:path, $primitive_type:ty)),*) => {
            match $sample_format {
                $($format => {
                    let mut scratch = scratch_vec(scratch_capacity);

                    in_device.build_input_stream(
                        stream_config,
                        move |input: &[$primitive_type], _info: &cpal::InputCallbackInfo| {
                            for in_chunk in input.chunks(scratch_capacity) {
                                for (o, &i) in scratch.iter_mut().zip(in_chunk.iter()) {
                                    *o = <f32 as cpal::FromSample<$primitive_type>>::from_sample_(i);
                                }

                                callback.callback(&scratch[..in_chunk.len()]);
                            }
                        },
                        err_callback(
                            true,
                            input_stream_running.clone(),
                            err_to_cx_tx.clone(),
                            #[cfg(any(
                                target_os = "linux",
                                target_os = "freebsd",
                                target_os = "dragonfly",
                                target_os = "netbsd"
                            ))]
                            is_alsa
                        ),
                        Some(BUILD_STREAM_TIMEOUT),
                    )
                },)*
                _ => unreachable!(),
            }
        }
    }

    // The cpal ASIO backend requires the callback buffer type to match the
    // driver's native format (unlike WASAPI, which converts internally).
    // For non-f32 formats, render into an f32 scratch buffer and convert
    // on the way out. The f32 path stays a direct passthrough.
    let stream_handle = if let SampleFormat::F32 = in_sample_format {
        in_device.build_input_stream(
            stream_config,
            move |input: &[f32], _info: &cpal::InputCallbackInfo| {
                callback.callback(input);
            },
            err_callback(
                true,
                input_stream_running.clone(),
                err_to_cx_tx.clone(),
                #[cfg(any(
                    target_os = "linux",
                    target_os = "freebsd",
                    target_os = "dragonfly",
                    target_os = "netbsd"
                ))]
                is_alsa,
            ),
            Some(BUILD_STREAM_TIMEOUT),
        )
    } else {
        build_input_stream!(
            in_sample_format,
            (SampleFormat::I8, i8),
            (SampleFormat::I16, i16),
            (SampleFormat::I32, i32),
            (SampleFormat::I64, i64),
            (SampleFormat::U8, u8),
            (SampleFormat::U16, u16),
            (SampleFormat::U32, u32),
            (SampleFormat::U64, u64),
            (SampleFormat::F64, f64)
        )
    };

    let stream_handle = match stream_handle {
        Ok(s) => s,
        Err(e) => {
            if config.fail_on_no_input {
                return Err(StartStreamError::BuildStreamError(e));
            } else {
                #[cfg(any(feature = "log", feature = "tracing"))]
                error!(
                    "Failed to build input audio stream, input stream will not be started. {}",
                    e
                );
                return Ok(StartInputStreamResult::NotStarted);
            }
        }
    };

    if let Err(e) = stream_handle.play() {
        if let cpal::ErrorKind::RealtimeDenied = e.kind() {
            #[cfg(any(feature = "log", feature = "tracing"))]
            warn!(
                "Failed to get realtime priority for input audio thread. This may increase latency and/or audio glitches"
            );
        } else if config.fail_on_no_input {
            return Err(StartStreamError::PlayStreamError(e));
        } else {
            #[cfg(any(feature = "log", feature = "tracing"))]
            error!(
                "Failed to start input audio stream, input stream will not be started. {}",
                e
            );
            return Ok(StartInputStreamResult::NotStarted);
        }
    }

    Ok(StartInputStreamResult::Started {
        stream_handle,
        cons,
        num_stream_in_channels: num_in_channels as u32,
        in_device_id,
        input_stream_running,
    })
}

#[allow(clippy::large_enum_variant)]
enum StartInputStreamResult {
    NotStarted,
    Started {
        stream_handle: cpal::Stream,
        cons: fixed_resample::ResamplingCons<f32>,
        num_stream_in_channels: u32,
        in_device_id: Option<DeviceId>,
        input_stream_running: Arc<AtomicBool>,
    },
}

struct InputCallback {
    prod: ResamplingProd<f32>,
    err_to_cx_tx: mpsc::Sender<IoStreamError>,
    input_stream_running: Arc<AtomicBool>,
}

impl InputCallback {
    fn callback(&mut self, input: &[f32]) {
        let _ = self.prod.push_interleaved(input);
    }
}

impl Drop for InputCallback {
    fn drop(&mut self) {
        self.input_stream_running.store(false, Ordering::Relaxed);
        let _ = self
            .err_to_cx_tx
            .send(IoStreamError::Input(cpal::Error::new(
                cpal::ErrorKind::StreamInvalidated,
            )));
    }
}

struct OutputCallback {
    num_out_channels: usize,
    processor: FirewheelProcessor,
    sample_rate: u32,
    sample_rate_recip: f64,
    predicted_delta_time: Duration,
    prev_instant: Option<Instant>,
    stream_start_instant: Instant,
    input_stream_cons: Option<fixed_resample::ResamplingCons<f32>>,
    input_buffer: Vec<f32>,
    err_to_cx_tx: mpsc::Sender<IoStreamError>,
    input_stream_running: Option<Arc<AtomicBool>>,
    output_stream_running: Arc<AtomicBool>,
}

impl OutputCallback {
    #[allow(clippy::too_many_arguments)]
    fn new(
        num_out_channels: usize,
        max_block_frames: usize,
        sample_rate: u32,
        processor: FirewheelProcessor,
        input_stream_cons: Option<fixed_resample::ResamplingCons<f32>>,
        err_to_cx_tx: mpsc::Sender<IoStreamError>,
        input_stream_running: Option<Arc<AtomicBool>>,
        output_stream_running: Arc<AtomicBool>,
    ) -> Self {
        let stream_start_instant = Instant::now();

        let input_buffer = if let Some(cons) = &input_stream_cons {
            scratch_vec(max_block_frames * cons.num_channels())
        } else {
            Vec::new()
        };

        Self {
            num_out_channels,
            processor,
            sample_rate,
            sample_rate_recip: f64::from(sample_rate).recip(),
            predicted_delta_time: Duration::default(),
            prev_instant: None,
            stream_start_instant,
            input_stream_cons,
            input_buffer,
            err_to_cx_tx,
            input_stream_running,
            output_stream_running,
        }
    }

    fn callback(&mut self, output: &mut [f32], info: &cpal::OutputCallbackInfo) {
        let process_timestamp = bevy_platform::time::Instant::now();

        let frames = output.len() / self.num_out_channels;

        let (underflow, dropped_frames) = if let Some(prev_instant) = self.prev_instant {
            let delta_time = process_timestamp - prev_instant;

            let underflow = delta_time > self.predicted_delta_time;

            let dropped_frames = if underflow {
                (delta_time.as_secs_f64() * self.sample_rate as f64).round() as u32
            } else {
                0
            };

            (underflow, dropped_frames)
        } else {
            self.prev_instant = Some(process_timestamp);
            (false, 0)
        };

        // Calculate the next predicted stream time to detect underflows.
        //
        // Add a little bit of wiggle room to account for tiny clock
        // inaccuracies and rounding errors.
        self.predicted_delta_time =
            Duration::from_secs_f64(frames as f64 * self.sample_rate_recip * 1.5);

        let duration_since_stream_start =
            process_timestamp.duration_since(self.stream_start_instant);

        // TODO: PLEASE FIX ME:
        //
        // It appears that for some reason, both Windows and Linux will sometimes return a timestamp which
        // has a value less than the previous timestamp. I am unsure if this is a bug with the APIs, a bug
        // with CPAL, or I'm just misunderstaning how the timestamps are supposed to be used. Either way,
        // it is disabled for now and `bevy_platform::time::Instance::now()` is being used as a workaround above.
        //
        // let (internal_clock_secs, underflow) = if let Some(instant) =
        //     &self.first_internal_clock_instant
        // {
        //     if let Some(prev_stream_instant) = &self.prev_stream_instant {
        //         if info
        //             .timestamp()
        //             .playback
        //             .duration_since(prev_stream_instant)
        //             .is_none()
        //         {
        //             // If this occurs in other APIs as well, then either CPAL is doing
        //             // something wrong, or I'm doing something wrong.
        //             error!("CPAL and/or the system audio API returned invalid timestamp. Please notify the Firewheel developers of this bug.");
        //         }
        //     }
        //
        //     let internal_clock_secs = info
        //         .timestamp()
        //         .playback
        //         .duration_since(instant)
        //         .map(|s| s.as_secs_f64())
        //         .unwrap_or_else(|| self.predicted_stream_secs.unwrap_or(0.0));
        //
        //     let underflow = if let Some(predicted_stream_secs) = self.predicted_stream_secs {
        //         // If the stream time is significantly greater than the predicted stream
        //         // time, it means an output underflow has occurred.
        //         internal_clock_secs > predicted_stream_secs
        //     } else {
        //         false
        //     };
        //
        //     // Calculate the next predicted stream time to detect underflows.
        //     //
        //     // Add a little bit of wiggle room to account for tiny clock
        //     // inaccuracies and rounding errors.
        //     self.predicted_stream_secs =
        //         Some(internal_clock_secs + (frames as f64 * self.sample_rate_recip * 1.2));
        //
        //     self.prev_stream_instant = Some(info.timestamp().playback);
        //
        //     (ClockSeconds(internal_clock_secs), underflow)
        // } else {
        //     self.first_internal_clock_instant = Some(info.timestamp().playback);
        //     (ClockSeconds(0.0), false)
        // };

        let (num_in_channels, input_stream_status) = if let Some(cons) = &mut self.input_stream_cons
        {
            let num_in_channels = cons.num_channels();
            let num_input_samples = frames * num_in_channels;

            if self
                .input_stream_running
                .as_ref()
                .unwrap()
                .load(Ordering::Relaxed)
            {
                let status =
                    cons.read_interleaved(&mut self.input_buffer[..num_input_samples], false);

                let status = match status {
                    ReadStatus::UnderflowOccurred { num_frames_read: _ } => {
                        StreamStatus::OUTPUT_UNDERFLOW
                    }
                    ReadStatus::OverflowCorrected {
                        num_frames_discarded: _,
                    } => StreamStatus::INPUT_OVERFLOW,
                    _ => StreamStatus::empty(),
                };

                (num_in_channels, status)
            } else {
                self.input_buffer[..num_input_samples].fill(0.0);

                (num_in_channels, StreamStatus::CLOSED)
            }
        } else {
            (0, StreamStatus::empty())
        };

        let mut output_stream_status = StreamStatus::empty();
        if underflow {
            output_stream_status.insert(StreamStatus::OUTPUT_UNDERFLOW);
        }

        let timestamp = info.timestamp();
        let process_to_playback_delay = timestamp.playback.duration_since(timestamp.callback);

        self.processor.process(
            &InterleavedSlice::new(
                &self.input_buffer[..frames * num_in_channels],
                num_in_channels,
                frames,
            )
            .unwrap(),
            &mut InterleavedSlice::new_mut(output, self.num_out_channels, frames).unwrap(),
            BackendProcessInfo {
                frames,
                process_timestamp: Some(process_timestamp),
                duration_since_stream_start,
                input_stream_status,
                output_stream_status,
                dropped_frames,
                process_to_playback_delay: Some(process_to_playback_delay),
            },
        );
    }
}

impl Drop for OutputCallback {
    fn drop(&mut self) {
        self.output_stream_running.store(false, Ordering::Relaxed);
        let _ = self
            .err_to_cx_tx
            .send(IoStreamError::Input(cpal::Error::new(
                cpal::ErrorKind::StreamInvalidated,
            )));
    }
}

fn err_callback(
    is_input: bool,
    is_running: Arc<AtomicBool>,
    err_to_cx_tx: mpsc::Sender<IoStreamError>,
    #[cfg(any(
        target_os = "linux",
        target_os = "freebsd",
        target_os = "dragonfly",
        target_os = "netbsd"
    ))]
    is_alsa: bool,
) -> impl FnMut(cpal::Error) + Send + 'static {
    let mut last_underrun_msg_instant: Option<Instant> = None;

    move |err| {
        let do_send = match err.kind() {
            cpal::ErrorKind::Xrun => {
                let mut do_send = true;
                if let Some(instant) = last_underrun_msg_instant
                    && instant.elapsed() < UNDERRUN_LOG_COOLDOWN
                {
                    do_send = false;
                }

                if do_send {
                    last_underrun_msg_instant = Some(Instant::now());
                }

                do_send
            }
            cpal::ErrorKind::DeviceNotAvailable
            | cpal::ErrorKind::HostUnavailable
            | cpal::ErrorKind::BackendError
            | cpal::ErrorKind::Other
            | cpal::ErrorKind::PermissionDenied
            | cpal::ErrorKind::StreamInvalidated => {
                is_running.store(false, Ordering::Relaxed);
                true
            }
            _ => false,
        };

        // Alsa can be very spammy with its warning messages.
        #[cfg(any(
            target_os = "linux",
            target_os = "freebsd",
            target_os = "dragonfly",
            target_os = "netbsd"
        ))]
        let do_send = if do_send
            && is_alsa
            && err.kind() != cpal::ErrorKind::StreamInvalidated
            && let Some(msg) = err.message()
            // Add more filters here if needed.
            && msg.contains("snd_pcm_avail_delay")
        {
            false
        } else {
            do_send
        };

        if do_send
            && let Err(e) = err_to_cx_tx.send(if is_input {
                IoStreamError::Input(err)
            } else {
                IoStreamError::Output(err)
            })
        {
            // Make sure the error gets logged even if the handle has been dropped.
            #[cfg(any(feature = "log", feature = "tracing"))]
            error!("Audio stream error occurred: {}", e.0);
            #[cfg(not(any(feature = "log", feature = "tracing")))]
            let _ = e;
        }
    }
}

/// An error occurred while trying to start a CPAL audio stream.
#[derive(Debug, thiserror::Error)]
pub enum StartStreamError {
    /// The Firewheel context is already active. Either it has never been activated
    /// or the [`FirewheelProcessor`] counterpart has not been dropped yet.
    ///
    /// Note, in rare cases where the audio thread crashes without cleanly
    /// dropping its contents, this may never succeed. Consider adding a
    /// timeout to avoid deadlocking.
    #[error("Failed to activate Firewheel context: The Firewheel context is already active")]
    AlreadyActive,
    /// The audio graph failed to compile.
    #[error("Failed to activate Firewheel context: Audio graph failed to compile: {0}")]
    GraphCompileError(#[from] CompileGraphError),

    #[error("The requested audio input device was not found: {0}")]
    InputDeviceNotFound(String),
    #[error("The requested audio output device was not found: {0}")]
    OutputDeviceNotFound(String),
    #[error("Failed to get default input output device")]
    DefaultInputDeviceNotFound,
    #[error("Failed to get default audio output device")]
    DefaultOutputDeviceNotFound,
    #[error("Failed to get audio device config: {0}")]
    FailedToGetConfig(cpal::Error),
    #[error("Failed to build audio stream: {0}")]
    BuildStreamError(cpal::Error),
    #[error("Failed to play audio stream: {0}")]
    PlayStreamError(cpal::Error),

    #[cfg(not(feature = "resample_inputs"))]
    #[error("Not able to use a sample rate of {0} for the input audio device")]
    CouldNotMatchSampleRate(u32),
}

impl From<ActivateError> for StartStreamError {
    fn from(e: ActivateError) -> Self {
        match e {
            ActivateError::AlreadyActive => Self::AlreadyActive,
            ActivateError::GraphCompileError(e) => Self::GraphCompileError(e),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum IoStreamError {
    /// An error on the input stream occurred.
    #[error("Input audio stream error: {0}")]
    Input(cpal::Error),
    /// An error on the output stream occurred.
    #[error("Output audio stream error: {0}")]
    Output(cpal::Error),
}

fn scratch_vec(len: usize) -> Vec<f32> {
    let mut v = Vec::new();
    v.reserve_exact(len);
    v.resize(len, 0.0f32);
    v
}
