use core::{
    fmt::Debug,
    num::{NonZeroU32, NonZeroUsize},
    time::Duration,
    u32,
};
use std::sync::mpsc;

use bevy_platform::time::Instant;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use firewheel_core::{node::StreamStatus, StreamInfo};
use firewheel_graph::{
    backend::{AudioBackend, DeviceInfo},
    processor::FirewheelProcessor,
    FirewheelCtx,
};
use fixed_resample::{PushStatus, ReadStatus, ResamplingChannelConfig};
use ringbuf::traits::{Consumer, Producer, Split};

/// 1024 samples is a latency of about 23 milliseconds, which should
/// be good enough for most games.
const DEFAULT_MAX_BLOCK_FRAMES: u32 = 1024;
const MAX_BLOCK_FRAMES: u32 = 8192;
const BUILD_STREAM_TIMEOUT: Duration = Duration::from_secs(5);
const MSG_CHANNEL_CAPACITY: usize = 4;
const MAX_INPUT_CHANNELS: usize = 16;

pub type FirewheelContext = FirewheelCtx<CpalBackend>;

/// The configuration of an output audio stream in the CPAL backend.
#[derive(Debug, Clone, PartialEq)]
pub struct CpalOutputConfig {
    /// The host to use. Set to `None` to use the
    /// system's default audio host.
    pub host: Option<cpal::HostId>,

    /// The name of the output device to use. Set to `None` to use the
    /// system's default output device.
    ///
    /// By default this is set to `None`.
    pub device_name: Option<String>,

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
            device_name: None,
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

    /// The name of the input device to use. Set to `None` to use the
    /// system's default input device.
    ///
    /// By default this is set to `None`.
    pub device_name: Option<String>,

    /// The latency/block size of the audio stream to use. Set to
    /// `None` to use the device's default value.
    ///
    /// Smaller values may give better latency, but is not supported on
    /// all platforms and may lead to performance issues.
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
            device_name: None,
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

/// A CPAL backend for Firewheel
pub struct CpalBackend {
    from_err_rx: mpsc::Receiver<cpal::StreamError>,
    to_stream_tx: ringbuf::HeapProd<CtxToStreamMsg>,
    _out_stream_handle: cpal::Stream,
    _in_stream_handle: Option<cpal::Stream>,
}

impl AudioBackend for CpalBackend {
    type Config = CpalConfig;
    type StartStreamError = StreamStartError;
    type StreamError = cpal::StreamError;

    fn available_input_devices() -> Vec<DeviceInfo> {
        let mut devices = Vec::with_capacity(8);

        // TODO: Iterate over all the available hosts?
        let host = cpal::default_host();

        let default_device_name = if let Some(default_device) = host.default_input_device() {
            match default_device.name() {
                Ok(n) => Some(n),
                Err(e) => {
                    log::warn!("Failed to get name of default audio input device: {}", e);
                    None
                }
            }
        } else {
            None
        };

        match host.input_devices() {
            Ok(input_devices) => {
                for device in input_devices {
                    let Ok(name) = device.name() else {
                        continue;
                    };

                    let is_default = if let Some(default_device_name) = &default_device_name {
                        &name == default_device_name
                    } else {
                        false
                    };

                    let default_in_config = match device.default_input_config() {
                        Ok(c) => c,
                        Err(e) => {
                            if is_default {
                                log::warn!("Failed to get default config for the default audio input device: {}", e);
                            }
                            continue;
                        }
                    };

                    devices.push(DeviceInfo {
                        name,
                        num_channels: default_in_config.channels(),
                        is_default,
                    })
                }
            }
            Err(e) => {
                log::error!("Failed to get input audio devices: {}", e);
            }
        }

        devices
    }

    fn available_output_devices() -> Vec<DeviceInfo> {
        let mut devices = Vec::with_capacity(8);

        // TODO: Iterate over all the available hosts?
        let host = cpal::default_host();

        let default_device_name = if let Some(default_device) = host.default_output_device() {
            match default_device.name() {
                Ok(n) => Some(n),
                Err(e) => {
                    log::warn!("Failed to get name of default audio output device: {}", e);
                    None
                }
            }
        } else {
            None
        };

        match host.output_devices() {
            Ok(output_devices) => {
                for device in output_devices {
                    let Ok(name) = device.name() else {
                        continue;
                    };

                    let is_default = if let Some(default_device_name) = &default_device_name {
                        &name == default_device_name
                    } else {
                        false
                    };

                    let default_out_config = match device.default_output_config() {
                        Ok(c) => c,
                        Err(e) => {
                            if is_default {
                                log::warn!("Failed to get default config for the default audio output device: {}", e);
                            }
                            continue;
                        }
                    };

                    devices.push(DeviceInfo {
                        name,
                        num_channels: default_out_config.channels(),
                        is_default,
                    })
                }
            }
            Err(e) => {
                log::error!("Failed to get output audio devices: {}", e);
            }
        }

        devices
    }

    fn start_stream(config: Self::Config) -> Result<(Self, StreamInfo), Self::StartStreamError> {
        log::info!("Attempting to start CPAL audio stream...");

        let host = if let Some(host_id) = config.output.host {
            match cpal::host_from_id(host_id) {
                Ok(host) => host,
                Err(e) => {
                    log::warn!("Requested audio host {:?} is not available: {}. Falling back to default host...", &host_id, e);
                    cpal::default_host()
                }
            }
        } else {
            cpal::default_host()
        };

        let mut out_device = None;
        if let Some(device_name) = &config.output.device_name {
            match host.output_devices() {
                Ok(mut output_devices) => {
                    if let Some(d) = output_devices.find(|d| {
                        if let Ok(name) = d.name() {
                            &name == device_name
                        } else {
                            false
                        }
                    }) {
                        out_device = Some(d);
                    } else if config.output.fallback {
                        log::warn!("Could not find requested audio output device: {}. Falling back to default device...", &device_name);
                    } else {
                        return Err(StreamStartError::OutputDeviceNotFound(device_name.clone()));
                    }
                }
                Err(e) => {
                    if config.output.fallback {
                        log::error!("Failed to get output audio devices: {}. Falling back to default device...", e);
                    } else {
                        return Err(e.into());
                    }
                }
            }
        }

        if out_device.is_none() {
            let Some(default_device) = host.default_output_device() else {
                return Err(StreamStartError::DefaultOutputDeviceNotFound);
            };
            out_device = Some(default_device);
        }
        let out_device = out_device.unwrap();

        let out_device_name = out_device.name().unwrap_or_else(|e| {
            log::warn!("Failed to get name of output audio device: {}", e);
            String::from("unknown device")
        });

        let default_config = out_device.default_output_config()?;

        let default_sample_rate = default_config.sample_rate().0;
        // Try to use the common sample rates by default.
        let try_common_sample_rates = default_sample_rate != 44100 && default_sample_rate != 48000;

        let desired_block_frames =
            if let &cpal::SupportedBufferSize::Range { min, max } = default_config.buffer_size() {
                config
                    .output
                    .desired_block_frames
                    .map(|f| f.clamp(min, max))
            } else {
                None
            };

        let mut supports_desired_sample_rate = false;
        let mut supports_44100 = false;
        let mut supports_48000 = false;

        if config.output.desired_sample_rate.is_some() || try_common_sample_rates {
            for cpal_config in out_device.supported_output_configs()? {
                if let Some(sr) = config.output.desired_sample_rate {
                    if !supports_desired_sample_rate {
                        if cpal_config
                            .try_with_sample_rate(cpal::SampleRate(sr))
                            .is_some()
                        {
                            supports_desired_sample_rate = true;
                            break;
                        }
                    }
                }

                if try_common_sample_rates {
                    if !supports_44100 {
                        if cpal_config
                            .try_with_sample_rate(cpal::SampleRate(44100))
                            .is_some()
                        {
                            supports_44100 = true;
                        }
                    }
                    if !supports_48000 {
                        if cpal_config
                            .try_with_sample_rate(cpal::SampleRate(48000))
                            .is_some()
                        {
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
            sample_rate: cpal::SampleRate(sample_rate),
            buffer_size: desired_buffer_size,
        };

        let (max_block_frames, actual_max_block_frames) = match out_stream_config.buffer_size {
            cpal::BufferSize::Default => {
                (DEFAULT_MAX_BLOCK_FRAMES as usize, MAX_BLOCK_FRAMES as usize)
            }
            cpal::BufferSize::Fixed(f) => (f as usize, f as usize),
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
            input_device_name,
            input_to_output_latency_seconds,
        ) = if let StartInputStreamResult::Started {
            stream_handle,
            cons,
            num_stream_in_channels,
            input_device_name,
        } = input_stream
        {
            let input_to_output_latency_seconds = cons.latency_seconds();

            (
                Some(stream_handle),
                Some(cons),
                num_stream_in_channels,
                Some(input_device_name),
                input_to_output_latency_seconds,
            )
        } else {
            (None, None, 0, None, 0.0)
        };

        let (to_stream_tx, from_cx_rx) =
            ringbuf::HeapRb::<CtxToStreamMsg>::new(MSG_CHANNEL_CAPACITY).split();

        let mut data_callback = DataCallback::new(
            num_out_channels,
            actual_max_block_frames,
            from_cx_rx,
            out_stream_config.sample_rate.0,
            input_stream_cons,
        );

        log::info!(
            "Starting output audio stream with device \"{}\" with configuration {:?}",
            &out_device_name,
            &out_stream_config
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
            sample_rate: NonZeroU32::new(out_stream_config.sample_rate.0).unwrap(),
            max_block_frames: NonZeroU32::new(max_block_frames as u32).unwrap(),
            num_stream_in_channels,
            num_stream_out_channels: num_out_channels as u32,
            input_to_output_latency_seconds,
            output_device_name: Some(out_device_name),
            input_device_name,
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

    fn set_processor(&mut self, processor: FirewheelProcessor) {
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
                log::warn!("Requested audio host {:?} is not available: {}. Falling back to default host...", &host_id, e);
                cpal::default_host()
            }
        }
    } else {
        cpal::default_host()
    };

    let mut in_device = None;
    if let Some(device_name) = &config.device_name {
        match host.input_devices() {
            Ok(mut input_devices) => {
                if let Some(d) = input_devices.find(|d| {
                    if let Ok(name) = d.name() {
                        &name == device_name
                    } else {
                        false
                    }
                }) {
                    in_device = Some(d);
                } else if config.fallback {
                    log::warn!("Could not find requested audio input device: {}. Falling back to default device...", &device_name);
                } else if config.fail_on_no_input {
                    return Err(StreamStartError::InputDeviceNotFound(device_name.clone()));
                } else {
                    log::warn!("Could not find requested audio input device: {}. No input stream will be started.", &device_name);
                    return Ok(StartInputStreamResult::NotStarted);
                }
            }
            Err(e) => {
                if config.fallback {
                    log::warn!(
                        "Failed to get output audio devices: {}. Falling back to default device...",
                        e
                    );
                } else if config.fail_on_no_input {
                    return Err(e.into());
                } else {
                    log::warn!(
                        "Failed to get output audio devices: {}. No input stream will be started.",
                        e
                    );
                    return Ok(StartInputStreamResult::NotStarted);
                }
            }
        }
    }

    if in_device.is_none() {
        if let Some(default_device) = host.default_input_device() {
            in_device = Some(default_device);
        } else if config.fail_on_no_input {
            return Err(StreamStartError::DefaultInputDeviceNotFound);
        } else {
            log::warn!("No default audio input device found. Input stream will not be started.");
            return Ok(StartInputStreamResult::NotStarted);
        }
    }
    let in_device = in_device.unwrap();

    let in_device_name = in_device.name().unwrap_or_else(|e| {
        log::warn!("Failed to get name of input audio device: {}", e);
        String::from("unknown device")
    });

    let default_config = in_device.default_input_config()?;

    let desired_block_frames =
        if let &cpal::SupportedBufferSize::Range { min, max } = default_config.buffer_size() {
            config.desired_block_frames.map(|f| f.clamp(min, max))
        } else {
            None
        };

    let supported_configs = in_device.supported_input_configs()?;

    let mut min_sample_rate = u32::MAX;
    let mut max_sample_rate = 0;
    for config in supported_configs.into_iter() {
        min_sample_rate = min_sample_rate.min(config.min_sample_rate().0);
        max_sample_rate = max_sample_rate.max(config.max_sample_rate().0);
    }
    let sample_rate =
        cpal::SampleRate(output_sample_rate.0.clamp(min_sample_rate, max_sample_rate));

    #[cfg(not(feature = "resample_inputs"))]
    if sample_rate != output_sample_rate {
        if config.fail_on_no_input {
            return Err(StreamStartError::CouldNotMatchSampleRate(
                output_sample_rate.0,
            ));
        } else {
            log::warn!("Could not use output sample rate {} for the input sample rate. Input stream will not be started", output_sample_rate.0);
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
        sample_rate.0,
        output_sample_rate.0,
        config.channel_config,
    );

    log::info!(
        "Starting input audio stream with device \"{}\" with configuration {:?}",
        &in_device_name,
        &stream_config
    );

    let stream_handle = match in_device.build_input_stream(
        &stream_config,
        move |input: &[f32], _info: &cpal::InputCallbackInfo| {
            let status = prod.push_interleaved(input);

            match status {
                PushStatus::OverflowOccurred { num_frames_pushed: _ } => {
                    // TODO: Logging is not realtime-safe. Find a different way to notify the user.
                    log::warn!("Overflow occured in audio input to output channel! Try increasing the channel capacity.");
                }
                PushStatus::UnderflowCorrected { num_zero_frames_pushed: _ } => {
                    // TODO: Logging is not realtime-safe. Find a different way to notify the user.
                    log::warn!("Underflow occured in audio input to output channel! Try increasing the channel latency.");
                }
                _ => {}
            }
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
                log::error!(
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
            log::error!(
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
        input_device_name: in_device_name,
    })
}

enum StartInputStreamResult {
    NotStarted,
    Started {
        stream_handle: cpal::Stream,
        cons: fixed_resample::ResamplingCons<f32>,
        num_stream_in_channels: u32,
        input_device_name: String,
    },
}

struct DataCallback {
    num_out_channels: usize,
    from_cx_rx: ringbuf::HeapCons<CtxToStreamMsg>,
    processor: Option<FirewheelProcessor>,
    sample_rate: u32,
    sample_rate_recip: f64,
    //_first_internal_clock_instant: Option<cpal::StreamInstant>,
    //_prev_stream_instant: Option<cpal::StreamInstant>,
    predicted_delta_time: Duration,
    prev_instant: Option<Instant>,
    input_stream_cons: Option<fixed_resample::ResamplingCons<f32>>,
    input_buffer: Vec<f32>,
}

impl DataCallback {
    fn new(
        num_out_channels: usize,
        max_block_frames: usize,
        from_cx_rx: ringbuf::HeapCons<CtxToStreamMsg>,
        sample_rate: u32,
        input_stream_cons: Option<fixed_resample::ResamplingCons<f32>>,
    ) -> Self {
        let input_buffer = if let Some(cons) = &input_stream_cons {
            let mut v = Vec::new();
            v.reserve_exact(max_block_frames * cons.num_channels().get());
            v.resize(max_block_frames * cons.num_channels().get(), 0.0);
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
        //             log::error!("CPAL and/or the system audio API returned invalid timestamp. Please notify the Firewheel developers of this bug.");
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

        let num_in_chanenls = if let Some(cons) = &mut self.input_stream_cons {
            let num_in_channels = cons.num_channels().get();

            // TODO: Have some realtime-safe way to notify users of underflows and overflows.
            let status = cons.read_interleaved(&mut self.input_buffer[..frames * num_in_channels]);

            match status {
                ReadStatus::UnderflowOccurred { num_frames_read: _ } => {
                    // TODO: Logging is not realtime-safe. Find a different way to notify the user.
                    log::warn!("Underflow occured in audio input to output channel! Try increasing the channel latency.");
                }
                ReadStatus::OverflowCorrected {
                    num_frames_discarded: _,
                } => {
                    // TODO: Logging is not realtime-safe. Find a different way to notify the user.
                    log::warn!("Overflow occured in audio input to output channel! Try increasing the channel capacity.");
                }
                _ => {}
            }

            num_in_channels
        } else {
            0
        };

        if let Some(processor) = &mut self.processor {
            let mut stream_status = StreamStatus::empty();

            if underflow {
                stream_status.insert(StreamStatus::OUTPUT_UNDERFLOW);
            }

            processor.process_interleaved(
                &self.input_buffer[..frames * num_in_chanenls],
                output,
                num_in_chanenls,
                self.num_out_channels,
                frames,
                process_timestamp,
                stream_status,
                dropped_frames,
            );
        } else {
            output.fill(0.0);
            return;
        }
    }
}

enum CtxToStreamMsg {
    NewProcessor(FirewheelProcessor),
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
