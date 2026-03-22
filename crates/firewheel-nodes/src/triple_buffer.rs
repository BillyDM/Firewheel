use bevy_platform::sync::{Arc, Mutex, MutexGuard};
use core::num::{NonZeroU32, NonZeroUsize};
use firewheel_core::node::NodeError;
use firewheel_core::{
    channel_config::{ChannelConfig, ChannelCount, NonZeroChannelCount},
    diff::{Diff, EventQueue, Patch, PatchError, PathBuilder},
    dsp::buffer::SequentialBuffer,
    event::{ParamData, ProcEvents},
    node::{
        AudioNode, AudioNodeInfo, AudioNodeProcessor, ConstructProcessorContext, ProcBuffers,
        ProcExtra, ProcInfo, ProcStreamCtx, ProcessStatus,
    },
    StreamInfo,
};

#[cfg(not(feature = "std"))]
use num_traits::Float;

/// The configuration of a [`TripleBufferNode`]
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
    pub fn output<'a>(&'a mut self) -> OutputDataGuard<'a> {
        OutputDataGuard {
            guarded_state: self.active_state.lock().unwrap(),
        }
    }
}

struct ActiveState {
    consumer: triple_buffer::Output<TripleBufferData>,
    sample_rate: NonZeroU32,
}

pub struct OutputData<'a> {
    /// The samples of data.
    ///
    /// Note, the length of this buffer may be longer than the actual number of
    /// frames that are currently written to. Only read up to [`OutputData::frames`]
    /// from this buffer.
    pub buffer: &'a SequentialBuffer<f32>,

    /// The number of frames of usable data that are in [`OutputData::buffer`].
    pub frames: usize,

    /// A value equal to how many times the buffer has been updated since the node
    /// was first created. This can be used to quickly check if the buffer differs
    /// from the previous read.
    pub generation: u64,
}

pub struct OutputDataGuard<'a> {
    guarded_state: MutexGuard<'a, Option<ActiveState>>,
}

impl<'a> OutputDataGuard<'a> {
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

    /// Get the latest audio data.
    ///
    /// If the node is not currently active, then this will return `None`.
    pub fn data<'b>(&'b mut self) -> Option<OutputData<'b>> {
        self.guarded_state.as_mut().map(|s| {
            let c = s.consumer.read();
            OutputData {
                buffer: &c.buffer,
                frames: c.frames,
                generation: c.generation,
            }
        })
    }

    /// Peek the data that is currently in the buffer without checking if
    /// there is new data.
    ///
    /// If the node is not currently active, then this will return `None`.
    pub fn peek_data<'b>(&'b self) -> Option<OutputData<'b>> {
        self.guarded_state.as_ref().map(|s| {
            let c = s.consumer.output_buffer();
            OutputData {
                buffer: &c.buffer,
                frames: c.frames,
                generation: c.generation,
            }
        })
    }
}

impl AudioNode for TripleBufferNode {
    type Configuration = TripleBufferConfig;

    fn info(&self, config: &Self::Configuration) -> Result<AudioNodeInfo, NodeError> {
        Ok(AudioNodeInfo::new()
            .debug_name("triple_buffer")
            .channel_config(ChannelConfig {
                num_inputs: config.channels.get(),
                num_outputs: ChannelCount::ZERO,
            })
            .custom_state(TripleBufferState {
                num_channels: config.channels,
                active_state: Arc::new(Mutex::new(None)),
            }))
    }

    fn construct_processor(
        &self,
        config: &Self::Configuration,
        mut cx: ConstructProcessorContext,
    ) -> Result<impl AudioNodeProcessor, NodeError> {
        let sample_rate = cx.stream_info.sample_rate;
        let max_window_size_frames = config.max_window_size.as_frames(sample_rate) as usize;

        let (producer, consumer) =
            triple_buffer::triple_buffer::<TripleBufferData>(&TripleBufferData::new(
                NonZeroUsize::new(config.channels.get().get() as usize).unwrap(),
                max_window_size_frames,
                0,
            ));

        let state = cx.custom_state_mut::<TripleBufferState>().unwrap();

        *state.active_state.lock().unwrap() = Some(ActiveState {
            consumer,
            sample_rate,
        });
        let active_state = Arc::clone(&state.active_state);

        let window_size_frames =
            (self.window_size.as_frames(sample_rate) as usize).min(max_window_size_frames);

        Ok(Processor {
            producer: Some(producer),
            config: *config,
            max_window_size_frames,
            params: *self,
            window_size_frames,
            tmp_ring_buffer: SequentialBuffer::new(
                NonZeroUsize::new(config.channels.get().get() as usize).unwrap(),
                max_window_size_frames,
            ),
            ring_buf_ptr: 0,
            active_state,
            generation: 0,
            prev_publish_was_silent: true,
            num_silent_frames_in_tmp: window_size_frames,
            tmp_buffer_needs_cleared: false,
            num_inputs: config.channels.get().get() as usize,
        })
    }
}

struct Processor {
    producer: Option<triple_buffer::Input<TripleBufferData>>,
    config: TripleBufferConfig,
    max_window_size_frames: usize,

    params: TripleBufferNode,
    window_size_frames: usize,

    tmp_ring_buffer: SequentialBuffer<f32>,
    ring_buf_ptr: usize,

    // The processor only uses this when a new stream has started.
    active_state: Arc<Mutex<Option<ActiveState>>>,
    generation: u64,

    prev_publish_was_silent: bool,
    num_silent_frames_in_tmp: usize,
    tmp_buffer_needs_cleared: bool,
    num_inputs: usize,
}

impl AudioNodeProcessor for Processor {
    fn process(
        &mut self,
        info: &ProcInfo,
        buffers: Option<ProcBuffers>,
        events: &mut ProcEvents,
        _extra: &mut ProcExtra,
    ) -> ProcessStatus {
        let was_enabled = self.params.enabled;

        let mut new_window_size_frames = self.window_size_frames;
        for patch in events.drain_patches::<TripleBufferNode>() {
            match patch {
                TripleBufferNodePatch::WindowSize(window_size) => {
                    new_window_size_frames = (window_size.as_frames(info.sample_rate) as usize)
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
                    let data = producer.input_buffer_mut();

                    for buf_ch in data.buffer.iter_channels_mut() {
                        buf_ch[..new_window_size_frames].fill(0.0);
                    }

                    self.generation += 1;
                    data.generation = self.generation;
                    data.frames = new_window_size_frames;
                }

                producer.publish();

                for tmp_ch in self.tmp_ring_buffer.iter_channels_mut() {
                    tmp_ch[..new_window_size_frames].fill(0.0);
                }

                self.window_size_frames = new_window_size_frames;
                self.ring_buf_ptr = 0;
                self.prev_publish_was_silent = true;
                self.num_silent_frames_in_tmp = new_window_size_frames;
                self.tmp_buffer_needs_cleared = false;
            }

            return ProcessStatus::ClearAllOutputs;
        }

        let mut resized = false;
        if self.window_size_frames != new_window_size_frames {
            let prev = self.window_size_frames;

            // Use the data in the triple buffer as a temporary scratch buffer.
            let data = producer.input_buffer_mut();

            for (buf_ch, tmp_ch) in data
                .buffer
                .iter_channels_mut()
                .zip(self.tmp_ring_buffer.iter_channels_mut())
            {
                let (head, tail) = tmp_ch[..prev].split_at(self.ring_buf_ptr);
                buf_ch[..tail.len()].copy_from_slice(tail);
                if tail.len() < prev {
                    buf_ch[tail.len()..prev].copy_from_slice(head);
                }

                // Rebuild tmp_ch at the new window size.
                if prev >= new_window_size_frames {
                    tmp_ch[..new_window_size_frames]
                        .copy_from_slice(&buf_ch[prev - new_window_size_frames..prev]);
                } else {
                    let pad = new_window_size_frames - prev;
                    tmp_ch[..pad].fill(0.0);
                    tmp_ch[pad..new_window_size_frames].copy_from_slice(&buf_ch[..prev]);
                }
            }

            self.window_size_frames = new_window_size_frames;
            self.ring_buf_ptr = 0;
            self.num_silent_frames_in_tmp = 0;
            resized = true;
        }

        let input_is_silent = info.in_silence_mask.all_channels_silent(self.num_inputs);
        if input_is_silent {
            self.num_silent_frames_in_tmp =
                (self.num_silent_frames_in_tmp + info.frames).min(self.window_size_frames);
        } else {
            self.num_silent_frames_in_tmp = 0;
        }

        if self.num_silent_frames_in_tmp == self.window_size_frames
            && self.prev_publish_was_silent
            && !resized
        {
            // The previous publish already contained silence, so no need to publish again.
            self.tmp_buffer_needs_cleared = true;
            return ProcessStatus::ClearAllOutputs;
        }

        if let Some(buffers) = buffers {
            if info.frames >= self.window_size_frames {
                // Just copy all the new data.
                for (tmp_ch, in_ch) in self
                    .tmp_ring_buffer
                    .iter_channels_mut()
                    .zip(buffers.inputs.iter())
                {
                    tmp_ch[..self.window_size_frames].copy_from_slice(
                        &in_ch[info.frames - self.window_size_frames..info.frames],
                    );
                }
                self.ring_buf_ptr = 0;
                self.tmp_buffer_needs_cleared = false;
            } else {
                if self.tmp_buffer_needs_cleared {
                    self.tmp_buffer_needs_cleared = false;

                    for tmp_ch in self.tmp_ring_buffer.iter_channels_mut() {
                        tmp_ch[..self.window_size_frames].fill(0.0);
                    }
                    self.ring_buf_ptr = 0;

                    self.num_silent_frames_in_tmp = self.window_size_frames;
                }

                let first_copy_frames =
                    info.frames.min(self.window_size_frames - self.ring_buf_ptr);
                let second_copy_frames = info.frames - first_copy_frames;

                for (tmp_ch, in_ch) in self
                    .tmp_ring_buffer
                    .iter_channels_mut()
                    .zip(buffers.inputs.iter())
                {
                    if first_copy_frames > 0 {
                        tmp_ch[self.ring_buf_ptr..self.ring_buf_ptr + first_copy_frames]
                            .copy_from_slice(&in_ch[..first_copy_frames]);
                    }

                    if second_copy_frames > 0 {
                        tmp_ch[..second_copy_frames]
                            .copy_from_slice(&in_ch[first_copy_frames..info.frames]);
                    }
                }

                self.ring_buf_ptr = if second_copy_frames > 0 {
                    second_copy_frames
                } else {
                    self.ring_buf_ptr + first_copy_frames
                };
            }
        } else {
            if info.did_just_bypass {
                self.tmp_buffer_needs_cleared = false;
                for tmp_ch in self.tmp_ring_buffer.iter_channels_mut() {
                    tmp_ch[..self.window_size_frames].fill(0.0);
                }
                self.ring_buf_ptr = 0;
                self.num_silent_frames_in_tmp = self.window_size_frames;
            }
        }

        {
            let buffer = producer.input_buffer_mut();

            for (buf_ch, tmp_ch) in buffer
                .buffer
                .iter_channels_mut()
                .zip(self.tmp_ring_buffer.iter_channels())
            {
                let (head, tail) = tmp_ch[..self.window_size_frames].split_at(self.ring_buf_ptr);
                buf_ch[..tail.len()].copy_from_slice(tail);
                buf_ch[tail.len()..self.window_size_frames].copy_from_slice(head);
            }

            self.generation += 1;
            buffer.generation = self.generation;
            buffer.frames = self.window_size_frames;
        }

        producer.publish();

        self.prev_publish_was_silent = self.num_silent_frames_in_tmp == self.window_size_frames;

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

        self.tmp_ring_buffer = SequentialBuffer::new(
            NonZeroUsize::new(self.config.channels.get().get() as usize).unwrap(),
            self.max_window_size_frames,
        );

        self.ring_buf_ptr = 0;
        self.num_silent_frames_in_tmp = self.window_size_frames;
        self.tmp_buffer_needs_cleared = false;
        self.prev_publish_was_silent = true;

        self.generation += 1;

        let (producer, consumer) =
            triple_buffer::triple_buffer::<TripleBufferData>(&TripleBufferData::new(
                NonZeroUsize::new(self.config.channels.get().get() as usize).unwrap(),
                self.max_window_size_frames,
                self.generation,
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
    buffer: SequentialBuffer<f32>,
    max_frames: usize,
    frames: usize,
    generation: u64,
}

impl TripleBufferData {
    fn new(num_channels: NonZeroUsize, max_frames: usize, generation: u64) -> Self {
        Self {
            buffer: SequentialBuffer::new(num_channels, max_frames),
            max_frames,
            frames: 0,
            generation,
        }
    }
}

impl Clone for TripleBufferData {
    fn clone(&self) -> Self {
        Self::new(self.buffer.num_channels(), self.max_frames, self.generation)
    }
}
