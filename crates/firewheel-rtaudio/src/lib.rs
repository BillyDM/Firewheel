use audioadapter_buffers::direct::InterleavedSlice;
use bevy_platform::sync::{Mutex, OnceLock};
use core::{num::NonZeroU32, time::Duration};
use firewheel_core::node::StreamStatus;
use firewheel_graph::{
    backend::BackendProcessInfo,
    error::{ActivateError, CompileGraphError},
    processor::FirewheelProcessor,
    ActivateInfo, FirewheelContext,
};
use rtaudio::{Api, RtAudioError, StreamConfig};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc,
};

pub use rtaudio;

#[cfg(all(feature = "log", not(feature = "tracing")))]
use log::{error, info, warn};
#[cfg(feature = "tracing")]
use tracing::{error, info, warn};

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

/// An RtAudio stream running a [`FirewheelProcessor`].
///
/// The audio stream is automatically stopped when this struct is dropped.
pub struct RtAudioStream {
    stream_handle: rtaudio::StreamHandle,
    is_running: Arc<AtomicBool>,
}

impl RtAudioStream {
    /// Create a new audio stream with the given [`FirewheelContext`].
    pub fn new(
        cx: &mut FirewheelContext,
        mut config: RtAudioConfig,
    ) -> Result<Self, StartStreamError> {
        info!("Attempting to start RtAudio audio stream...");

        if cx.is_active() {
            return Err(StartStreamError::AlreadyActive);
        }

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

        let process_to_playback_delay = info.latency.map(|latency_frames| {
            Duration::from_secs_f64(latency_frames as f64 / info.sample_rate as f64)
        });

        let activate_info = ActivateInfo {
            sample_rate: NonZeroU32::new(info.sample_rate).unwrap(),
            max_block_frames: NonZeroU32::new(info.max_frames as u32).unwrap(),
            num_stream_in_channels: info.in_channels as u32,
            num_stream_out_channels: info.out_channels as u32,
            input_to_output_latency_seconds: 0.0,
        };

        let processor = cx.activate(activate_info)?;

        let is_running = Arc::new(AtomicBool::new(true));

        let mut cb = DataCallback::new(
            processor,
            info.sample_rate,
            process_to_playback_delay,
            Arc::clone(&is_running),
        );

        stream_handle.start(
            move |buffers: rtaudio::Buffers<'_>,
                  info: &rtaudio::StreamInfo,
                  status: rtaudio::StreamStatus| {
                cb.callback(buffers, info, status);
            },
        )?;

        info!("{}", &success_msg);

        Ok(Self {
            stream_handle,
            is_running,
        })
    }

    /// Poll the status of the audio stream and log any errors/warnings that have occurred.
    ///
    /// Note, if an error is returned, it doesn't always mean that the stream has stopped.
    /// Instead, use [`RtAudioStream::is_running()`] to check if the stream is still running.
    pub fn poll_status(&mut self) -> Vec<RtAudioError> {
        let cb = ERROR_CB_SINGLETON.get_or_init(|| Mutex::new(ErrorCallbackSingleton::new()));

        match cb.lock() {
            Ok(cb_lock) => cb_lock.from_err_rx.try_iter().collect(),
            Err(e) => {
                panic!("Failed to acquire RtAudio error callback lock: {}", e);
            }
        }
    }

    /// Same as [`RtAudioStream::poll_status`], but automatically logs all of the errors/
    /// warnings to the log output.
    #[cfg(any(feature = "log", feature = "tracing"))]
    pub fn log_status(&mut self) {
        for e in self.poll_status() {
            error!("Audio stream error occurred: {}", e);
        }
    }

    /// Returns `true` if the audio stream is currently running.
    ///
    /// Returns `false` if the audio stream has stopped unexpectedly (i.e. an audio device
    /// was disconnected). When this happens, this `RtAudioStream` instance should be dropped,
    /// and a new one created.
    pub fn is_running(&self) -> bool {
        self.is_running.load(Ordering::Relaxed)
    }

    /// Information about the running audio stream
    pub fn stream_info(&self) -> &rtaudio::StreamInfo {
        self.stream_handle.info()
    }
}

impl Drop for RtAudioStream {
    fn drop(&mut self) {
        // Make sure any remaining errors/warnings get logged.
        #[cfg(any(feature = "log", feature = "tracing"))]
        self.log_status();
    }
}

struct DataCallback {
    processor: FirewheelProcessor,
    next_predicted_stream_time: Option<f64>,
    sample_rate_recip: f64,
    process_to_playback_delay: Option<Duration>,
    is_running: Arc<AtomicBool>,
}

impl DataCallback {
    fn new(
        processor: FirewheelProcessor,
        sample_rate: u32,
        process_to_playback_delay: Option<Duration>,
        is_running: Arc<AtomicBool>,
    ) -> Self {
        Self {
            processor,
            next_predicted_stream_time: None,
            sample_rate_recip: (sample_rate as f64).recip(),
            process_to_playback_delay,
            is_running,
        }
    }

    fn callback(
        &mut self,
        mut buffers: rtaudio::Buffers<'_>,
        info: &rtaudio::StreamInfo,
        status: rtaudio::StreamStatus,
    ) {
        let rtaudio::Buffers::Float32 { output, input } = &mut buffers else {
            unreachable!()
        };

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

        self.processor.process(
            &InterleavedSlice::new(input, info.in_channels, frames).unwrap(),
            &mut InterleavedSlice::new_mut(output, info.out_channels, frames).unwrap(),
            BackendProcessInfo {
                frames,
                process_timestamp: None,
                duration_since_stream_start: Duration::from_secs_f64(info.stream_time),
                input_stream_status,
                output_stream_status,
                dropped_frames,
                process_to_playback_delay: self.process_to_playback_delay,
            },
        );
    }
}

impl Drop for DataCallback {
    fn drop(&mut self) {
        self.is_running.store(false, Ordering::Relaxed);
    }
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
                // Make sure the error gets logged even if the handle has been dropped.
                #[cfg(any(feature = "log", feature = "tracing"))]
                error!("Audio stream error occurred: {}", e.0);
            }
        });

        Self { from_err_rx }
    }
}

/// An error occured while trying to start a CPAL audio stream.
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
    RtAudioError(#[from] RtAudioError),
}

impl From<ActivateError> for StartStreamError {
    fn from(e: ActivateError) -> Self {
        match e {
            ActivateError::AlreadyActive => Self::AlreadyActive,
            ActivateError::GraphCompileError(e) => Self::GraphCompileError(e),
        }
    }
}
