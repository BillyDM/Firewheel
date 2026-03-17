use bevy_platform::time::Instant;
use core::time::Duration;
use firewheel_core::node::StreamStatus;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendProcessInfo {
    pub num_in_channels: usize,
    pub num_out_channels: usize,
    pub frames: usize,
    pub process_timestamp: Option<Instant>,
    pub duration_since_stream_start: Duration,
    pub input_stream_status: StreamStatus,
    pub output_stream_status: StreamStatus,
    pub dropped_frames: u32,

    /// The estimated time between when this process loop was called and
    /// when the data will be delivered to the output device for playback.
    ///
    /// If the audio backend does not provide this information, then set
    /// this to `None`.
    pub playback_delay: Option<Duration>,
}
