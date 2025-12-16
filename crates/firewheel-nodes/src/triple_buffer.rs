use bevy_platform::sync::{Arc, Mutex, MutexGuard};
use core::num::NonZeroU32;
use firewheel_core::{
    channel_config::{ChannelConfig, ChannelCount, NonZeroChannelCount},
    diff::{Diff, EventQueue, Patch, PatchError, PathBuilder},
    event::{ParamData, ProcEvents},
    node::{
        AudioNode, AudioNodeInfo, AudioNodeProcessor, ConstructProcessorContext, ProcBuffers,
        ProcExtra, ProcInfo, ProcStreamCtx, ProcessStatus,
    },
    StreamInfo,
};

/// The configuration of a [`StreamReaderNode`]
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "bevy", derive(bevy_ecs::prelude::Component))]
#[cfg_attr(feature = "bevy_reflect", derive(bevy_reflect::Reflect))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TripleBufferConfig {
    /// The number of channels
    pub channels: NonZeroChannelCount,
    /// The maximum window size that can be used
    pub max_window_size: WindowSize,
}

impl Default for TripleBufferConfig {
    fn default() -> Self {
        Self {
            channels: NonZeroChannelCount::STEREO,
            max_window_size: WindowSize::default(),
        }
    }
}

/// The window size for a [`TripleBufferNode`]
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "bevy", derive(bevy_ecs::prelude::Component))]
#[cfg_attr(feature = "bevy_reflect", derive(bevy_reflect::Reflect))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum WindowSize {
    /// Use the capacity in units of samples (of a single channel
    /// of audio)
    Samples(u32),
    /// Use the capacity in units of seconds
    Seconds(f64),
}

impl WindowSize {
    pub fn as_frames(&self, sample_rate: NonZeroU32) -> u32 {
        match self {
            Self::Samples(samples) => *samples,
            Self::Seconds(seconds) => (seconds * (sample_rate.get() as f64)).round() as u32,
        }
    }
}

impl Default for WindowSize {
    fn default() -> Self {
        Self::Samples(2048)
    }
}

impl Diff for WindowSize {
    fn diff<E: EventQueue>(&self, baseline: &Self, path: PathBuilder, event_queue: &mut E) {
        if self != baseline {
            match self {
                WindowSize::Samples(samples) => event_queue.push_param(*samples, path),
                WindowSize::Seconds(seconds) => event_queue.push_param(*seconds, path),
            }
        }
    }
}

impl Patch for WindowSize {
    type Patch = Self;

    fn patch(data: &ParamData, _: &[u32]) -> Result<Self::Patch, PatchError> {
        match data {
            ParamData::U32(samples) => Ok(Self::Samples(*samples)),
            ParamData::F64(seconds) => Ok(Self::Seconds(*seconds)),
            _ => Err(PatchError::InvalidData),
        }
    }

    fn apply(&mut self, value: Self::Patch) {
        *self = value;
    }
}

/// A node that sends raw audio data from the audio graph to another
/// thread. Useful for cases where you only care about the latest data
/// in the buffer, such as for creating visualizers.
#[derive(Diff, Patch, Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "bevy", derive(bevy_ecs::prelude::Component))]
#[cfg_attr(feature = "bevy_reflect", derive(bevy_reflect::Reflect))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TripleBufferNode {
    /// The window size (the number of frames in each channel in the output buffer)
    pub window_size: WindowSize,
    /// Whether or not the node is enabled.
    ///
    /// Disable when not in use to save on CPU resources.
    pub enabled: bool,
}

impl Default for TripleBufferNode {
    fn default() -> Self {
        Self {
            window_size: WindowSize::default(),
            enabled: true,
        }
    }
}

#[derive(Clone)]
pub struct TripleBufferState {
    num_channels: NonZeroChannelCount,
    active_state: Arc<Mutex<Option<ActiveState>>>,
}

impl TripleBufferState {
    /// The number of channels in this buffer.
    pub fn num_channels(&self) -> NonZeroChannelCount {
        self.num_channels
    }

    /// Get the latest audio data in the triple buffer.
    pub fn output<'a>(&'a mut self) -> OutputAudioData<'a> {
        OutputAudioData {
            guarded_state: self.active_state.lock().unwrap(),
        }
    }
}

struct ActiveState {
    consumer: triple_buffer::Output<TripleBufferData>,
    sample_rate: NonZeroU32,
}

pub struct OutputAudioData<'a> {
    guarded_state: MutexGuard<'a, Option<ActiveState>>,
}

impl<'a> OutputAudioData<'a> {
    /// Returns `true` if the node is currently active.
    pub fn is_active(&self) -> bool {
        self.guarded_state.is_some()
    }

    /// The sample rate of the audio data.
    ///
    /// If the node is not currently active, then this will return `None`.
    pub fn sample_rate(&self) -> Option<NonZeroU32> {
        self.guarded_state.as_ref().map(|s| s.sample_rate)
    }

    /// Get the latest channels of audio data.
    ///
    /// The samples are in de-interleaved format (one `Vec` for each channel). The
    /// length of each `Vec` will be equal to the `window_size` parameter at the
    /// time the buffer was last updated.
    ///
    /// If the node is not currently active, then this will return `None`.
    pub fn channels<'b>(&'b mut self) -> Option<&'b [Vec<f32>]> {
        self.guarded_state
            .as_mut()
            .map(|s| s.consumer.read().buffers.as_slice())
    }

    /// Peek the audio data that is currently in the buffer without checking if
    /// there is new data.
    ///
    /// The samples are in de-interleaved format (one `Vec` for each channel). The
    /// length of each `Vec` will be equal to the `window_size` parameter at the
    /// time the buffer was last updated.
    ///
    /// If the node is not currently active, then this will return `None`.
    pub fn peek_channels<'b>(&'b self) -> Option<&'b [Vec<f32>]> {
        self.guarded_state
            .as_ref()
            .map(|s| s.consumer.peek_output_buffer().buffers.as_slice())
    }
}

impl AudioNode for TripleBufferNode {
    type Configuration = TripleBufferConfig;

    fn info(&self, config: &Self::Configuration) -> AudioNodeInfo {
        AudioNodeInfo::new()
            .debug_name("triple_buffer")
            .channel_config(ChannelConfig {
                num_inputs: config.channels.get(),
                num_outputs: ChannelCount::ZERO,
            })
            .custom_state(TripleBufferState {
                num_channels: config.channels,
                active_state: Arc::new(Mutex::new(None)),
            })
    }

    fn construct_processor(
        &self,
        config: &Self::Configuration,
        mut cx: ConstructProcessorContext,
    ) -> impl AudioNodeProcessor {
        let sample_rate = cx.stream_info.sample_rate;
        let max_window_size_frames = config.max_window_size.as_frames(sample_rate) as usize;

        let (producer, consumer) = triple_buffer::triple_buffer::<TripleBufferData>(
            &TripleBufferData::new(config.channels.get().get() as usize, max_window_size_frames),
        );

        let state = cx.custom_state_mut::<TripleBufferState>().unwrap();

        *state.active_state.lock().unwrap() = Some(ActiveState {
            consumer,
            sample_rate,
        });
        let active_state = Arc::clone(&state.active_state);

        let window_size_frames =
            (self.window_size.as_frames(sample_rate) as usize).min(max_window_size_frames);

        let tmp_ring_buffer = (0..config.channels.get().get() as usize)
            .map(|_| {
                let mut v = Vec::new();
                v.reserve_exact(max_window_size_frames);
                v.resize(window_size_frames, 0.0);
                v
            })
            .collect();

        Processor {
            producer: Some(producer),
            config: *config,
            max_window_size_frames,
            params: *self,
            window_size_frames,
            tmp_ring_buffer,
            ring_buf_ptr: 0,
            active_state,
        }
    }
}

struct Processor {
    producer: Option<triple_buffer::Input<TripleBufferData>>,
    config: TripleBufferConfig,
    max_window_size_frames: usize,

    params: TripleBufferNode,
    window_size_frames: usize,

    tmp_ring_buffer: Vec<Vec<f32>>,
    ring_buf_ptr: usize,

    // The processor only uses this when a new stream has started.
    active_state: Arc<Mutex<Option<ActiveState>>>,
}

impl AudioNodeProcessor for Processor {
    fn process(
        &mut self,
        info: &ProcInfo,
        buffers: ProcBuffers,
        events: &mut ProcEvents,
        _extra: &mut ProcExtra,
    ) -> ProcessStatus {
        let was_enabled = self.params.enabled;

        for patch in events.drain_patches::<TripleBufferNode>() {
            match patch {
                TripleBufferNodePatch::WindowSize(window_size) => {
                    self.window_size_frames = (window_size.as_frames(info.sample_rate) as usize)
                        .min(self.max_window_size_frames);
                }
                _ => {}
            }

            self.params.apply(patch);
        }

        let producer = self.producer.as_mut().unwrap();

        if !self.params.enabled {
            if was_enabled {
                {
                    let out_buffer = producer.input_buffer_mut();

                    for out_ch in out_buffer.buffers.iter_mut() {
                        out_ch.clear();
                        out_ch.resize(self.window_size_frames, 0.0);
                    }
                }

                producer.publish();

                for tmp_ch in self.tmp_ring_buffer.iter_mut() {
                    tmp_ch.clear();
                    tmp_ch.resize(self.window_size_frames, 0.0);
                }
                self.ring_buf_ptr = 0;
            }

            return ProcessStatus::ClearAllOutputs;
        }

        if self.tmp_ring_buffer[0].len() != self.window_size_frames {
            let prev_window_size_frames = self.tmp_ring_buffer[0].len();

            // Use the data in the triple buffer as a temporary scratch buffer.
            let buffer = producer.input_buffer_mut();

            let first_copy_frames = prev_window_size_frames - self.ring_buf_ptr;
            let second_copy_frames = prev_window_size_frames - first_copy_frames;

            for (buf_ch, tmp_ch) in buffer
                .buffers
                .iter_mut()
                .zip(self.tmp_ring_buffer.iter_mut())
            {
                buf_ch.clear();

                if first_copy_frames > 0 {
                    buf_ch.extend_from_slice(
                        &tmp_ch[self.ring_buf_ptr..self.ring_buf_ptr + first_copy_frames],
                    );
                }
                if second_copy_frames > 0 {
                    buf_ch.extend_from_slice(&tmp_ch[0..second_copy_frames]);
                }

                tmp_ch.clear();
                if prev_window_size_frames >= self.window_size_frames {
                    tmp_ch.extend_from_slice(
                        &buf_ch[prev_window_size_frames - self.window_size_frames
                            ..prev_window_size_frames],
                    );
                } else {
                    tmp_ch.resize(self.window_size_frames - prev_window_size_frames, 0.0);
                    tmp_ch.extend_from_slice(&buf_ch[0..prev_window_size_frames]);
                }
            }

            self.ring_buf_ptr = 0;
        }

        if info.frames >= self.window_size_frames {
            // Just copy all the new data.
            for (tmp_ch, in_ch) in self.tmp_ring_buffer.iter_mut().zip(buffers.inputs.iter()) {
                tmp_ch[0..self.window_size_frames]
                    .copy_from_slice(&in_ch[info.frames - self.window_size_frames..info.frames]);
            }
            self.ring_buf_ptr = 0;
        } else {
            let first_copy_frames = info.frames.min(self.window_size_frames - self.ring_buf_ptr);
            let second_copy_frames = info.frames - first_copy_frames;

            for (tmp_ch, in_ch) in self.tmp_ring_buffer.iter_mut().zip(buffers.inputs.iter()) {
                if first_copy_frames > 0 {
                    tmp_ch[self.ring_buf_ptr..self.ring_buf_ptr + first_copy_frames]
                        .copy_from_slice(&in_ch[0..first_copy_frames]);
                }

                if second_copy_frames > 0 {
                    tmp_ch[0..second_copy_frames].copy_from_slice(
                        &in_ch[first_copy_frames..first_copy_frames + second_copy_frames],
                    );
                }
            }

            self.ring_buf_ptr = if second_copy_frames > 0 {
                second_copy_frames
            } else {
                self.ring_buf_ptr + first_copy_frames
            };
        }

        {
            let buffer = producer.input_buffer_mut();

            let first_copy_frames = self.window_size_frames - self.ring_buf_ptr;
            let second_copy_frames = self.window_size_frames - first_copy_frames;

            for (buf_ch, tmp_ch) in buffer.buffers.iter_mut().zip(self.tmp_ring_buffer.iter()) {
                buf_ch.clear();

                if first_copy_frames > 0 {
                    buf_ch.extend_from_slice(
                        &tmp_ch[self.ring_buf_ptr..self.ring_buf_ptr + first_copy_frames],
                    );
                }
                if second_copy_frames > 0 {
                    buf_ch.extend_from_slice(&tmp_ch[0..second_copy_frames]);
                }
            }
        }

        producer.publish();

        ProcessStatus::ClearAllOutputs
    }

    fn stream_stopped(&mut self, _context: &mut ProcStreamCtx) {
        *self.active_state.lock().unwrap() = None;
        self.producer = None;
    }

    fn new_stream(&mut self, stream_info: &StreamInfo, _context: &mut ProcStreamCtx) {
        self.max_window_size_frames = self
            .config
            .max_window_size
            .as_frames(stream_info.sample_rate) as usize;

        self.window_size_frames = (self.params.window_size.as_frames(stream_info.sample_rate)
            as usize)
            .min(self.max_window_size_frames);

        self.tmp_ring_buffer = (0..self.config.channels.get().get() as usize)
            .map(|_| {
                let mut v = Vec::new();
                v.reserve_exact(self.max_window_size_frames);
                v.resize(self.window_size_frames, 0.0);
                v
            })
            .collect();
        self.ring_buf_ptr = 0;

        let (producer, consumer) =
            triple_buffer::triple_buffer::<TripleBufferData>(&TripleBufferData::new(
                self.config.channels.get().get() as usize,
                self.max_window_size_frames,
            ));

        *self.active_state.lock().unwrap() = Some(ActiveState {
            consumer,
            sample_rate: stream_info.sample_rate,
        });

        self.producer = Some(producer);
    }
}

// A wrapper to ensure that the triple buffer uses `reserve_exact` when cloning
// the initial buffers.
struct TripleBufferData {
    buffers: Vec<Vec<f32>>,
    max_frames: usize,
}

impl TripleBufferData {
    fn new(num_channels: usize, max_frames: usize) -> Self {
        let mut buffers = Vec::new();
        buffers.reserve_exact(num_channels);

        buffers = (0..num_channels)
            .map(|_| {
                let mut v = Vec::new();
                v.reserve_exact(max_frames);
                v.resize(max_frames, 0.0);
                v
            })
            .collect();

        Self {
            buffers,
            max_frames,
        }
    }
}

impl Clone for TripleBufferData {
    fn clone(&self) -> Self {
        Self::new(self.buffers.len(), self.max_frames)
    }
}
