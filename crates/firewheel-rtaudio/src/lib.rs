use bevy_platform::sync::{Mutex, OnceLock};
use core::{num::NonZeroU32, time::Duration};
use firewheel_core::{node::StreamStatus, StreamInfo};
use firewheel_graph::{
    backend::{AudioBackend, BackendProcessInfo, DeviceInfoSimple, SimpleStreamConfig},
    processor::FirewheelProcessor,
};
use ringbuf::traits::{Consumer, Producer, Split};
use rtaudio::{Api, DeviceID, DeviceInfo, DeviceParams, RtAudioError, SampleFormat, StreamConfig};
use std::sync::mpsc;

pub use rtaudio;

#[cfg(all(feature = "log", not(feature = "tracing")))]
use log::{error, info, warn};
#[cfg(feature = "tracing")]
use tracing::{error, info, warn};

const MSG_CHANNEL_CAPACITY: usize = 3;

/// The configuration of an RtAudio stream.
#[derive(Default, Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct RtAudioConfig {
    /// The system audio backend API to use.
    ///
    /// By default this is set to `Api::Unspecified` (use the
    /// best working API for the system).
    #[cfg_attr(feature = "serde", serde(default))]
    pub api: Api,
    /// The configuration of the stream.
    #[cfg_attr(feature = "serde", serde(default))]
    pub config: StreamConfig,
}

/// A struct used to retrieve the list of available audio devices
/// on the system and their available ocnfigurations.
pub struct RtAudioEnumerator;

impl RtAudioEnumerator {
    /// The system audio APIs that are available on this system.
    ///
    /// The first API in the list is the default one for the system.
    pub fn available_apis(&self) -> Vec<rtaudio::Api> {
        rtaudio::compiled_apis()
    }

    /// Get a struct used to retrieve the list of available audio devices
    /// for the default system audio API.
    pub fn default_api(&self) -> ApiEnumerator {
        ApiEnumerator {
            host: rtaudio::Host::new(self.available_apis()[0]).unwrap(),
        }
    }

    /// Get a struct used to retrieve the list of available audio devices
    /// for the given system audio API.
    pub fn get_api(api: rtaudio::Api) -> Result<ApiEnumerator, RtAudioError> {
        rtaudio::Host::new(api).map(|host| ApiEnumerator { host })
    }
}

/// A struct used to retrieve the list of available audio devices
/// for a given system audio API.
pub struct ApiEnumerator {
    pub host: rtaudio::Host,
}

impl ApiEnumerator {
    /// The system backend API this enumerator is using.
    pub fn api(&self) -> rtaudio::Api {
        self.host.api()
    }

    /// Get the list of available audio devices.
    pub fn devices(&self) -> &[DeviceInfo] {
        self.host.devices()
    }

    /// Retrieve an iterator over the available output audio devices.
    pub fn iter_output_devices<'a>(&'a self) -> impl Iterator<Item = &'a DeviceInfo> {
        self.host.iter_output_devices()
    }

    /// Retrieve an iterator over the available input audio devices.
    pub fn iter_input_devices<'a>(&'a self) -> impl Iterator<Item = &'a DeviceInfo> {
        self.host.iter_input_devices()
    }

    /// Retrieve an iterator over the available duplex audio devices.
    pub fn iter_duplex_devices<'a>(&'a self) -> impl Iterator<Item = &'a DeviceInfo> {
        self.host.iter_duplex_devices()
    }

    /// Get the index of the default input device.
    ///
    /// Return `None` if no default input device was found.
    pub fn default_input_device_index(&self) -> Option<usize> {
        self.host.default_input_device_index()
    }

    /// Get the index of the default output device.
    ///
    /// Return `None` if no default output device was found.
    pub fn default_output_device_index(&self) -> Option<usize> {
        self.host.default_output_device_index()
    }

    /// Get the index of the default duplex device.
    ///
    /// Return `None` if no default duplex device was found.
    pub fn default_duplex_device_index(&self) -> Option<usize> {
        self.host.default_output_device_index()
    }
}

/// An RtAudio backend for Firewheel
pub struct RtAudioBackend {
    _stream_handle: rtaudio::StreamHandle,
    to_stream_tx: ringbuf::HeapProd<CtxToStreamMsg>,
}

impl AudioBackend for RtAudioBackend {
    type Enumerator = RtAudioEnumerator;
    type Config = RtAudioConfig;
    type StartStreamError = RtAudioError;
    type StreamError = RtAudioError;
    type Instant = bevy_platform::time::Instant;

    fn enumerator() -> Self::Enumerator {
        RtAudioEnumerator {}
    }

    fn input_devices_simple(&mut self) -> Vec<DeviceInfoSimple> {
        let enumerator = RtAudioEnumerator {};
        let api_enumerator = enumerator.default_api();

        let mut default_device_index = None;

        let mut devices: Vec<DeviceInfoSimple> = api_enumerator
            .iter_input_devices()
            .enumerate()
            .map(|(i, info)| {
                if info.is_default_input {
                    default_device_index = Some(i);
                }

                DeviceInfoSimple {
                    name: info.name().to_string(),
                    id: info.id.as_serialized_string(),
                }
            })
            .collect();

        // Make sure the default device is the first in the list.
        if let Some(i) = default_device_index {
            devices.swap(0, i);
        }

        devices
    }

    fn output_devices_simple(&mut self) -> Vec<DeviceInfoSimple> {
        let enumerator = RtAudioEnumerator {};
        let api_enumerator = enumerator.default_api();

        let mut default_device_index = None;

        let mut devices: Vec<DeviceInfoSimple> = api_enumerator
            .iter_output_devices()
            .enumerate()
            .map(|(i, info)| {
                if info.is_default_input {
                    default_device_index = Some(i);
                }

                DeviceInfoSimple {
                    name: info.name().to_string(),
                    id: info.id.as_serialized_string(),
                }
            })
            .collect();

        // Make sure the default device is the first in the list.
        if let Some(i) = default_device_index {
            devices.swap(0, i);
        }

        devices
    }

    fn convert_simple_config(&mut self, config: &SimpleStreamConfig) -> Self::Config {
        RtAudioConfig {
            config: StreamConfig {
                output_device: Some(DeviceParams {
                    device_id: config
                        .output
                        .device
                        .as_ref()
                        .map(|s| DeviceID::from_serialized_string(s)),
                    num_channels: config.output.channels.map(|c| c as u32),
                    ..Default::default()
                }),
                input_device: config.input.as_ref().map(|input_config| DeviceParams {
                    device_id: input_config
                        .device
                        .as_ref()
                        .map(|s| DeviceID::from_serialized_string(s)),
                    num_channels: input_config.channels.map(|c| c as u32),
                    ..Default::default()
                }),
                sample_format: SampleFormat::Float32,
                sample_rate: config.desired_sample_rate,
                buffer_frames: config.desired_block_frames.unwrap_or(1024),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    fn start_stream(
        mut config: Self::Config,
    ) -> Result<(Self, StreamInfo), Self::StartStreamError> {
        info!("Attempting to start RtAudio audio stream...");

        // Make sure the error callback singleton is initialized before starting
        // any stream.
        let _ = ERROR_CB_SINGLETON.get_or_init(|| Mutex::new(ErrorCallbackSingleton::new()));

        // Firewheel always uses f32 sample foramt
        config.config.sample_format = rtaudio::SampleFormat::Float32;

        let host = match rtaudio::Host::new(config.api) {
            Ok(host) => host,
            Err(e) => {
                warn!(
                    "Requested audio API {:?} is not available: {}. Falling back to default API...",
                    &config.api, e
                );
                rtaudio::Host::default()
            }
        };

        let mut stream_handle = host.open_stream(&config.config).map_err(|(_, e)| e)?;

        let info = stream_handle.info();
        let success_msg = format!("Successfully started audio stream: {:?}", &info);

        let stream_info = StreamInfo {
            sample_rate: NonZeroU32::new(info.sample_rate).unwrap(),
            max_block_frames: NonZeroU32::new(info.max_frames as u32).unwrap(),
            num_stream_in_channels: info.in_channels as u32,
            num_stream_out_channels: info.out_channels as u32,
            input_to_output_latency_seconds: 0.0,
            output_device_id: info
                .output_device
                .as_ref()
                .map(|d| d.as_serialized_string())
                .unwrap_or_else(|| String::from("dummy output")),
            input_device_id: info.input_device.as_ref().map(|d| d.as_serialized_string()),
            // The engine will overwrite the other values.
            ..Default::default()
        };

        let (to_stream_tx, from_cx_rx) =
            ringbuf::HeapRb::<CtxToStreamMsg>::new(MSG_CHANNEL_CAPACITY).split();

        let mut cb = DataCallback::new(from_cx_rx, info.sample_rate);

        stream_handle.start(
            move |buffers: rtaudio::Buffers<'_>,
                  info: &rtaudio::StreamInfo,
                  status: rtaudio::StreamStatus| {
                cb.callback(buffers, info, status);
            },
        )?;

        info!("{}", &success_msg);

        Ok((
            RtAudioBackend {
                _stream_handle: stream_handle,
                to_stream_tx,
            },
            stream_info,
        ))
    }

    fn set_processor(&mut self, processor: FirewheelProcessor<Self>) {
        if let Err(_) = self
            .to_stream_tx
            .try_push(CtxToStreamMsg::NewProcessor(processor))
        {
            panic!("Failed to send new processor to RtAudio stream");
        }
    }

    fn poll_status(&mut self) -> Result<(), Self::StreamError> {
        let cb = ERROR_CB_SINGLETON.get_or_init(|| Mutex::new(ErrorCallbackSingleton::new()));

        let errors: Vec<RtAudioError> = match cb.lock() {
            Ok(cb_lock) => cb_lock.from_err_rx.try_iter().collect(),
            Err(e) => {
                panic!("Failed to acquire RtAudio error callback lock: {}", e);
            }
        };

        if !errors.is_empty() {
            if errors.len() > 1 {
                for e in errors.iter() {
                    error!("RtAudio stream error: {}", e);
                }
            }

            Err(errors.last().unwrap().clone())
        } else {
            Ok(())
        }
    }

    fn delay_from_last_process(&self, process_timestamp: Self::Instant) -> Option<Duration> {
        Some(process_timestamp.elapsed())
    }
}

struct DataCallback {
    from_cx_rx: ringbuf::HeapCons<CtxToStreamMsg>,
    processor: Option<FirewheelProcessor<RtAudioBackend>>,
    next_predicted_stream_time: Option<f64>,
    sample_rate_recip: f64,
}

impl DataCallback {
    fn new(from_cx_rx: ringbuf::HeapCons<CtxToStreamMsg>, sample_rate: u32) -> Self {
        Self {
            from_cx_rx,
            processor: None,
            next_predicted_stream_time: None,
            sample_rate_recip: (sample_rate as f64).recip(),
        }
    }

    fn callback(
        &mut self,
        mut buffers: rtaudio::Buffers<'_>,
        info: &rtaudio::StreamInfo,
        status: rtaudio::StreamStatus,
    ) {
        let process_timestamp = bevy_platform::time::Instant::now();

        let rtaudio::Buffers::Float32 { output, input } = &mut buffers else {
            unreachable!()
        };

        for msg in self.from_cx_rx.pop_iter() {
            let CtxToStreamMsg::NewProcessor(p) = msg;
            self.processor = Some(p);
        }

        if let Some(processor) = &mut self.processor {
            let frames = if info.out_channels > 0 {
                output.len() / info.out_channels
            } else if info.in_channels > 0 {
                input.len() / info.in_channels
            } else {
                0
            };

            let mut output_stream_status = StreamStatus::empty();
            let mut input_stream_status = StreamStatus::empty();
            if status.contains(rtaudio::StreamStatus::OUTPUT_UNDERFLOW) {
                output_stream_status.insert(StreamStatus::OUTPUT_UNDERFLOW);
            }
            if status.contains(rtaudio::StreamStatus::INPUT_OVERFLOW) {
                input_stream_status.insert(StreamStatus::INPUT_OVERFLOW);
            }

            let mut dropped_frames = 0;
            if status.contains(rtaudio::StreamStatus::OUTPUT_UNDERFLOW) {
                if let Some(next_predicted_stream_time) = self.next_predicted_stream_time {
                    dropped_frames = ((info.stream_time - next_predicted_stream_time)
                        * info.sample_rate as f64)
                        .round()
                        .max(0.0) as u32
                }
            }
            self.next_predicted_stream_time =
                Some(info.stream_time + (frames as f64 * self.sample_rate_recip));

            processor.process_interleaved(
                input,
                output,
                BackendProcessInfo {
                    num_in_channels: info.in_channels,
                    num_out_channels: info.out_channels,
                    frames,
                    process_timestamp,
                    duration_since_stream_start: Duration::from_secs_f64(info.stream_time),
                    input_stream_status,
                    output_stream_status,
                    dropped_frames,
                },
            );
        } else {
            output.fill(0.0);
        }
    }
}

enum CtxToStreamMsg {
    NewProcessor(FirewheelProcessor<RtAudioBackend>),
}

static ERROR_CB_SINGLETON: OnceLock<Mutex<ErrorCallbackSingleton>> = OnceLock::new();

struct ErrorCallbackSingleton {
    from_err_rx: mpsc::Receiver<RtAudioError>,
}

impl ErrorCallbackSingleton {
    fn new() -> Self {
        let (to_cb_tx, from_err_rx) = mpsc::channel();

        rtaudio::set_error_callback(move |e| {
            if let Err(e) = to_cb_tx.send(e) {
                error!("Failed to send error to Firewheel audio callback: {}", e);
            }
        });

        Self { from_err_rx }
    }
}
