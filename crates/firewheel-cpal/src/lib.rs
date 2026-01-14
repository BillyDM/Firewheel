use core::{
    fmt::Debug,
    num::{NonZeroU32, NonZeroUsize},
    time::Duration,
    u32,
};
use std::sync::mpsc;

pub use cpal;

use bevy_platform::time::Instant;
use cpal::{
    traits::{DeviceTrait, HostTrait, StreamTrait},
    DeviceId, HostId, HostUnavailable,
};
use firewheel_core::{node::StreamStatus, StreamInfo};
use firewheel_graph::{
    backend::{AudioBackend, BackendProcessInfo},
    processor::FirewheelProcessor,
};
use fixed_resample::{ReadStatus, ResamplingChannelConfig};
use ringbuf::traits::{Consumer, Producer, Split};

#[cfg(all(feature = "log", not(feature = "tracing")))]
use log::{error, info, warn};
#[cfg(feature = "tracing")]
use tracing::{error, info, warn};

/// 1024 samples is a latency of about 23 milliseconds, which should
/// be good enough for most games.
const DEFAULT_MAX_BLOCK_FRAMES: u32 = 1024;
const INPUT_ALLOC_BLOCK_FRAMES: usize = 4096;
const BUILD_STREAM_TIMEOUT: Duration = Duration::from_secs(5);
const MSG_CHANNEL_CAPACITY: usize = 4;
const MAX_INPUT_CHANNELS: usize = 16;

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
#[derive(Debug, Clone, PartialEq)]
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

impl Default for CpalConfig {
    fn default() -> Self {
        Self {
            output: CpalOutputConfig::default(),
            input: None,
        }
    }
}

/// A struct used to retrieve the list of available audio devices
/// on the system and their available ocnfigurations.
pub struct CpalEnumerator;

impl CpalEnumerator {
    /// The system audio hosts (APIs) that are available on this system.
    ///
    /// The first host in the list is the default one for the system.
    pub fn available_hosts() -> Vec<cpal::HostId> {
        cpal::available_hosts()
    }

    /// Get a struct used to retrieve the list of available audio devices
    /// for the default system audio host (API).
    pub fn default_host() -> HostEnumerator {
        HostEnumerator {
            host: cpal::default_host(),
        }
    }

    /// Get a struct used to retrieve the list of available audio devices
    /// for the given system audio host (API).
    pub fn get_host(api: HostId) -> Result<HostEnumerator, HostUnavailable> {
        cpal::host_from_id(api).map(|host| HostEnumerator { host })
    }
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
                warn!("Failed to get ID of default audio input device: {}", e);
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
                error!("Failed to get input audio devices: {}", e);
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
                warn!("Failed to get ID of default audio output device: {}", e);
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
                error!("Failed to get output audio devices: {}", e);
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

/// A CPAL backend for Firewheel
pub struct CpalBackend {
    from_err_rx: mpsc::Receiver<cpal::StreamError>,
    to_stream_tx: ringbuf::HeapProd<CtxToStreamMsg>,
    _out_stream_handle: cpal::Stream,
    _in_stream_handle: Option<cpal::Stream>,
}

impl AudioBackend for CpalBackend {
    type Enumerator = CpalEnumerator;
    type Config = CpalConfig;
    type StartStreamError = StreamStartError;
    type StreamError = cpal::StreamError;
    type Instant = bevy_platform::time::Instant;

    fn enumerator() -> Self::Enumerator {
        CpalEnumerator {}
    }

    fn start_stream(config: Self::Config) -> Result<(Self, StreamInfo), Self::StartStreamError> {
        info!("Attempting to start CPAL audio stream...");

        let host = if let Some(host_id) = config.output.host {
            match cpal::host_from_id(host_id) {
                Ok(host) => host,
                Err(e) => {
                    warn!("Requested audio host {:?} is not available: {}. Falling back to default host...", &host_id, e);
                    cpal::default_host()
                }
            }
        } else {
            cpal::default_host()
        };

        let mut out_device = None;
        if let Some(device_id) = &config.output.device_id {
            if let Some(device) = host.device_by_id(device_id) {
                if device.supports_output() {
                    out_device = Some(device);
                }
            }

            if out_device.is_none() {
                warn!("Could not find requested audio output device: {}. Falling back to default device...", &device_id);
            }
        }

        if out_device.is_none() {
            let Some(default_device) = host.default_output_device() else {
                return Err(StreamStartError::DefaultOutputDeviceNotFound);
            };
            out_device = Some(default_device);
        }
        let out_device = out_device.unwrap();

        let output_device_id = out_device.id().map(|d| d.to_string()).unwrap_or_else(|e| {
            warn!("Failed to get id of output audio device: {}", e);
            String::from("unknown")
        });

        let default_config = out_device.default_output_config()?;

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
            for cpal_config in out_device.supported_output_configs()? {
                if let Some(sr) = config.output.desired_sample_rate {
                    if !supports_desired_sample_rate {
                        if cpal_config.try_with_sample_rate(sr).is_some() {
                            supports_desired_sample_rate = true;
                            break;
                        }
                    }
                }

                if try_common_sample_rates {
                    if !supports_44100 {
                        if cpal_config.try_with_sample_rate(44100).is_some() {
                            supports_44100 = true;
                        }
                    }
                    if !supports_48000 {
                        if cpal_config.try_with_sample_rate(48000).is_some() {
                            supports_48000 = true;
                        }
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
            input_stream_handle,
            input_stream_cons,
            num_stream_in_channels,
            input_device_id,
            input_to_output_latency_seconds,
        ) = if let StartInputStreamResult::Started {
            stream_handle,
            cons,
            num_stream_in_channels,
            input_device_id,
        } = input_stream
        {
            let input_to_output_latency_seconds = cons.latency_seconds();

            (
                Some(stream_handle),
                Some(cons),
                num_stream_in_channels,
                Some(input_device_id),
                input_to_output_latency_seconds,
            )
        } else {
            (None, None, 0, None, 0.0)
        };

        let (to_stream_tx, from_cx_rx) =
            ringbuf::HeapRb::<CtxToStreamMsg>::new(MSG_CHANNEL_CAPACITY).split();

        let mut data_callback = DataCallback::new(
            num_out_channels,
            from_cx_rx,
            out_stream_config.sample_rate,
            input_stream_cons,
        );

        info!(
            "Starting output audio stream with device \"{}\" with configuration {:?}",
            &output_device_id, &out_stream_config
        );

        let out_stream_handle = out_device.build_output_stream(
            &out_stream_config,
            move |output: &mut [f32], info: &cpal::OutputCallbackInfo| {
                data_callback.callback(output, info);
            },
            move |err| {
                let _ = err_to_cx_tx.send(err);
            },
            Some(BUILD_STREAM_TIMEOUT),
        )?;

        out_stream_handle.play()?;

        let stream_info = StreamInfo {
            sample_rate: NonZeroU32::new(out_stream_config.sample_rate).unwrap(),
            max_block_frames: NonZeroU32::new(max_block_frames as u32).unwrap(),
            num_stream_in_channels,
            num_stream_out_channels: num_out_channels as u32,
            input_to_output_latency_seconds,
            output_device_id,
            input_device_id,
            // The engine will overwrite the other values.
            ..Default::default()
        };

        Ok((
            Self {
                from_err_rx,
                to_stream_tx,
                _out_stream_handle: out_stream_handle,
                _in_stream_handle: input_stream_handle,
            },
            stream_info,
        ))
    }

    fn set_processor(&mut self, processor: FirewheelProcessor<Self>) {
        if let Err(_) = self
            .to_stream_tx
            .try_push(CtxToStreamMsg::NewProcessor(processor))
        {
            panic!("Failed to send new processor to cpal stream");
        }
    }

    fn poll_status(&mut self) -> Result<(), Self::StreamError> {
        if let Ok(e) = self.from_err_rx.try_recv() {
            Err(e)
        } else {
            Ok(())
        }
    }

    fn delay_from_last_process(&self, process_timestamp: Self::Instant) -> Option<Duration> {
        Some(process_timestamp.elapsed())
    }
}

fn start_input_stream(
    config: &CpalInputConfig,
    output_sample_rate: cpal::SampleRate,
    err_to_cx_tx: mpsc::Sender<cpal::StreamError>,
) -> Result<StartInputStreamResult, StreamStartError> {
    let host = if let Some(host_id) = config.host {
        match cpal::host_from_id(host_id) {
            Ok(host) => host,
            Err(e) => {
                warn!("Requested audio host {:?} is not available: {}. Falling back to default host...", &host_id, e);
                cpal::default_host()
            }
        }
    } else {
        cpal::default_host()
    };

    let mut in_device = None;
    if let Some(device_id) = &config.device_id {
        if let Some(device) = host.device_by_id(device_id) {
            if device.supports_input() {
                in_device = Some(device);
            }
        }

        if in_device.is_none() {
            if config.fallback {
                warn!("Could not find requested audio input device: {}. Falling back to default device...", &device_id);
            } else {
                warn!("Could not find requested audio input device: {}. No input stream will be started.", &device_id);
                return Ok(StartInputStreamResult::NotStarted);
            }
        }
    }

    if in_device.is_none() {
        if let Some(default_device) = host.default_input_device() {
            in_device = Some(default_device);
        } else if config.fail_on_no_input {
            return Err(StreamStartError::DefaultInputDeviceNotFound);
        } else {
            warn!("No default audio input device found. Input stream will not be started.");
            return Ok(StartInputStreamResult::NotStarted);
        }
    }
    let in_device = in_device.unwrap();

    let in_device_id = in_device.id().map(|id| id.to_string()).unwrap_or_else(|e| {
        warn!("Failed to get ID of input audio device: {}", e);
        String::from("unknown")
    });

    let default_config = in_device.default_input_config()?;

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

    let supported_configs = in_device.supported_input_configs()?;

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
            return Err(StreamStartError::CouldNotMatchSampleRate(
                output_sample_rate,
            ));
        } else {
            warn!("Could not use output sample rate {} for the input sample rate. Input stream will not be started", output_sample_rate);
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

    let stream_config = cpal::StreamConfig {
        channels: num_in_channels as u16,
        sample_rate,
        buffer_size: desired_buffer_size,
    };

    let (mut prod, cons) = fixed_resample::resampling_channel::<f32, MAX_INPUT_CHANNELS>(
        NonZeroUsize::new(num_in_channels).unwrap(),
        sample_rate,
        output_sample_rate,
        config.channel_config,
    );

    info!(
        "Starting input audio stream with device \"{}\" with configuration {:?}",
        &in_device_id, &stream_config
    );

    let stream_handle = match in_device.build_input_stream(
        &stream_config,
        move |input: &[f32], _info: &cpal::InputCallbackInfo| {
            let _ = prod.push_interleaved(input);
        },
        move |err| {
            let _ = err_to_cx_tx.send(err);
        },
        Some(BUILD_STREAM_TIMEOUT),
    ) {
        Ok(s) => s,
        Err(e) => {
            if config.fail_on_no_input {
                return Err(StreamStartError::BuildStreamError(e));
            } else {
                error!(
                    "Failed to build input audio stream, input stream will not be started. {}",
                    e
                );
                return Ok(StartInputStreamResult::NotStarted);
            }
        }
    };

    if let Err(e) = stream_handle.play() {
        if config.fail_on_no_input {
            return Err(StreamStartError::PlayStreamError(e));
        } else {
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
        input_device_id: in_device_id,
    })
}

enum StartInputStreamResult {
    NotStarted,
    Started {
        stream_handle: cpal::Stream,
        cons: fixed_resample::ResamplingCons<f32>,
        num_stream_in_channels: u32,
        input_device_id: String,
    },
}

struct DataCallback {
    num_out_channels: usize,
    from_cx_rx: ringbuf::HeapCons<CtxToStreamMsg>,
    processor: Option<FirewheelProcessor<CpalBackend>>,
    sample_rate: u32,
    sample_rate_recip: f64,
    //_first_internal_clock_instant: Option<cpal::StreamInstant>,
    //_prev_stream_instant: Option<cpal::StreamInstant>,
    predicted_delta_time: Duration,
    prev_instant: Option<Instant>,
    stream_start_instant: Instant,
    input_stream_cons: Option<fixed_resample::ResamplingCons<f32>>,
    input_buffer: Vec<f32>,
}

impl DataCallback {
    fn new(
        num_out_channels: usize,
        from_cx_rx: ringbuf::HeapCons<CtxToStreamMsg>,
        sample_rate: u32,
        input_stream_cons: Option<fixed_resample::ResamplingCons<f32>>,
    ) -> Self {
        let stream_start_instant = Instant::now();

        let input_buffer = if let Some(cons) = &input_stream_cons {
            let mut v = Vec::new();
            v.reserve_exact(INPUT_ALLOC_BLOCK_FRAMES * cons.num_channels().get());
            v.resize(INPUT_ALLOC_BLOCK_FRAMES * cons.num_channels().get(), 0.0);
            v
        } else {
            Vec::new()
        };

        Self {
            num_out_channels,
            from_cx_rx,
            processor: None,
            sample_rate,
            sample_rate_recip: f64::from(sample_rate).recip(),
            //_first_internal_clock_instant: None,
            //_prev_stream_instant: None,
            predicted_delta_time: Duration::default(),
            prev_instant: None,
            stream_start_instant,
            input_stream_cons,
            input_buffer,
        }
    }

    fn callback(&mut self, output: &mut [f32], _info: &cpal::OutputCallbackInfo) {
        let process_timestamp = bevy_platform::time::Instant::now();

        for msg in self.from_cx_rx.pop_iter() {
            let CtxToStreamMsg::NewProcessor(p) = msg;
            self.processor = Some(p);
        }

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
        // innacuracies and rounding errors.
        self.predicted_delta_time =
            Duration::from_secs_f64(frames as f64 * self.sample_rate_recip * 1.2);

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
        //     // innacuracies and rounding errors.
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
            let num_in_channels = cons.num_channels().get();

            let num_input_samples = frames * num_in_channels;
            // Some platforms like wasapi might occasionally send a really large number of frames
            // to process. Since CPAL doesn't tell us the actual maximum block size of the stream,
            // there is not much we can do about it except to allocate when that happens.
            if num_input_samples > self.input_buffer.len() {
                self.input_buffer.resize(num_input_samples, 0.0);
            }

            let status = cons.read_interleaved(&mut self.input_buffer[..num_input_samples]);

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
            (0, StreamStatus::empty())
        };

        if let Some(processor) = &mut self.processor {
            let mut output_stream_status = StreamStatus::empty();

            if underflow {
                output_stream_status.insert(StreamStatus::OUTPUT_UNDERFLOW);
            }

            processor.process_interleaved(
                &self.input_buffer[..frames * num_in_channels],
                output,
                BackendProcessInfo {
                    num_in_channels,
                    num_out_channels: self.num_out_channels,
                    frames,
                    process_timestamp,
                    duration_since_stream_start,
                    input_stream_status,
                    output_stream_status,
                    dropped_frames,
                },
            );
        } else {
            output.fill(0.0);
            return;
        }
    }
}

enum CtxToStreamMsg {
    NewProcessor(FirewheelProcessor<CpalBackend>),
}

/// An error occured while trying to start a CPAL audio stream.
#[derive(Debug, thiserror::Error)]
pub enum StreamStartError {
    #[error("The requested audio input device was not found: {0}")]
    InputDeviceNotFound(String),
    #[error("The requested audio output device was not found: {0}")]
    OutputDeviceNotFound(String),
    #[error("Could not get audio devices: {0}")]
    FailedToGetDevices(#[from] cpal::DevicesError),
    #[error("Failed to get default input output device")]
    DefaultInputDeviceNotFound,
    #[error("Failed to get default audio output device")]
    DefaultOutputDeviceNotFound,
    #[error("Failed to get audio device configs: {0}")]
    FailedToGetConfigs(#[from] cpal::SupportedStreamConfigsError),
    #[error("Failed to get audio device config: {0}")]
    FailedToGetConfig(#[from] cpal::DefaultStreamConfigError),
    #[error("Failed to build audio stream: {0}")]
    BuildStreamError(#[from] cpal::BuildStreamError),
    #[error("Failed to play audio stream: {0}")]
    PlayStreamError(#[from] cpal::PlayStreamError),

    #[cfg(not(feature = "resample_inputs"))]
    #[error("Not able to use a samplerate of {0} for the input audio device")]
    CouldNotMatchSampleRate(u32),
}
