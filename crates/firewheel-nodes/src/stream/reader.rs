use bevy_platform::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use core::{
    num::{NonZeroU32, NonZeroUsize},
    ops::Range,
};

use firewheel_core::{
    channel_config::{ChannelConfig, ChannelCount, NonZeroChannelCount},
    collector::ArcGc,
    event::{NodeEventList, NodeEventType},
    node::{
        AudioNode, AudioNodeInfo, AudioNodeProcessor, ConstructProcessorContext, ProcBuffers,
        ProcInfo, ProcessStatus,
    },
    sync_wrapper::SyncWrapper,
};
use fixed_resample::{PushStatus, ReadStatus, ResamplingChannelConfig};

pub const MAX_CHANNELS: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "bevy", derive(bevy_ecs::prelude::Component))]
pub struct StreamReaderConfig {
    /// The number of channels.
    pub channels: NonZeroChannelCount,
}

impl Default for StreamReaderConfig {
    fn default() -> Self {
        Self {
            channels: NonZeroChannelCount::STEREO,
        }
    }
}

#[derive(Default, Debug, Clone, Copy)]
#[cfg_attr(feature = "bevy", derive(bevy_ecs::prelude::Component))]
pub struct StreamReaderNode;

#[derive(Clone)]
pub struct StreamReaderState {
    channels: NonZeroChannelCount,
    active_state: Option<ActiveState>,
    shared_state: ArcGc<SharedState>,
}

impl StreamReaderState {
    pub fn new(channels: NonZeroChannelCount) -> Self {
        assert!((channels.get().get() as usize) < MAX_CHANNELS);

        Self {
            channels,
            active_state: None,
            shared_state: ArcGc::new(SharedState::new()),
        }
    }

    /// Returns `true` if there is there is currently an active stream on this node.
    pub fn is_active(&self) -> bool {
        self.active_state.is_some() && self.shared_state.stream_active.load(Ordering::Relaxed)
    }

    /// Returns `true` if an underflow occured (due to the output stream
    /// running faster than the input stream).
    ///
    /// If this happens excessively in Release mode, you may want to consider
    /// increasing [`ResamplingChannelConfig::latency_seconds`].
    ///
    /// (Calling this will also reset the flag indicating whether an
    /// underflow occurred.)out
    pub fn underflow_occurred(&self) -> bool {
        self.shared_state
            .underflow_occurred
            .swap(false, Ordering::Relaxed)
    }

    /// Returns `true` if an overflow occured (due to the input stream
    /// running faster than the output stream).
    ///
    /// If this happens excessively in Release mode, you may want to consider
    /// increasing [`ResamplingChannelConfig::capacity_seconds`]. For
    /// example, if you are streaming data from a network, you may want to
    /// increase the capacity to several seconds.
    ///
    /// (Calling this will also reset the flag indicating whether an
    /// overflow occurred.)
    pub fn overflow_occurred(&self) -> bool {
        self.shared_state
            .overflow_occurred
            .swap(false, Ordering::Relaxed)
    }

    /// Begin the output audio stream on this node.
    ///
    /// The returned event must be sent to the node's processor for this to take effect.
    ///
    /// * `sample_rate` - The sample rate of this node.
    /// * `output_stream_sample_rate` - The sample rate of the active output audio stream.
    /// * `channel_config` - The configuration of the input to output channel.
    ///
    /// If there is already an active stream running on this node, then this will return
    /// an error.
    pub fn start_stream(
        &mut self,
        sample_rate: NonZeroU32,
        output_stream_sample_rate: NonZeroU32,
        channel_config: ResamplingChannelConfig,
    ) -> Result<NewOutputStreamEvent, ()> {
        if self.is_active() {
            return Err(());
        }

        self.shared_state.reset();

        let (prod, cons) = fixed_resample::resampling_channel::<f32, MAX_CHANNELS>(
            NonZeroUsize::new(self.channels.get().get() as usize).unwrap(),
            output_stream_sample_rate.get(),
            sample_rate.get(),
            channel_config,
        );

        self.active_state = Some(ActiveState {
            cons: Arc::new(Mutex::new(cons)),
            sample_rate,
        });
        self.shared_state
            .stream_active
            .store(true, Ordering::Relaxed);

        Ok(NewOutputStreamEvent { prod: Some(prod) })
    }

    /// The total number of frames (not samples) that can currently be read from
    /// the stream.
    ///
    /// If there is no active stream, the stream is paused, or the processor end
    /// is not ready to receive samples, then this will return `0`.
    pub fn available_frames(&self) -> usize {
        if self.is_ready() {
            self.active_state
                .as_ref()
                .map(|s| s.cons.lock().unwrap().available_frames())
                .unwrap_or(0)
        } else {
            0
        }
    }

    /// The amount of data in seconds that is currently occupied in the channel.
    ///
    /// This value will be in the range `[0.0, ResamplingChannelConfig::capacity_seconds]`.
    ///
    /// This can also be used to detect when an extra packet of data should be read or
    /// discarded to correct for jitter.
    ///
    /// If there is no active stream, then this will return `None`.
    pub fn occupied_seconds(&self) -> Option<f64> {
        self.active_state
            .as_ref()
            .map(|s| s.cons.lock().unwrap().occupied_seconds())
    }

    /// The number of channels in this node.
    pub fn num_channels(&self) -> NonZeroChannelCount {
        self.channels
    }

    /// The sample rate of the active stream.
    ///
    /// Returns `None` if there is no active stream.
    pub fn sample_rate(&self) -> Option<NonZeroU32> {
        self.active_state.as_ref().map(|s| s.sample_rate)
    }

    /// Read from the channel and write the results into the given output buffer
    /// in interleaved format.
    ///
    /// If there is no active stream, the stream is paused, or the processor end
    /// is not ready to send samples, then the output will be filled with zeros
    /// and `None` will be returned.
    pub fn read_interleaved(&mut self, output: &mut [f32]) -> Option<ReadStatus> {
        if !self.is_ready() {
            output.fill(0.0);
            return None;
        }

        Some(
            self.active_state
                .as_mut()
                .unwrap()
                .cons
                .lock()
                .unwrap()
                .read_interleaved(output),
        )
    }

    /// Read from the channel and write the results into the given output buffer in
    /// de-interleaved format.
    ///
    /// * `output` - The channels to write data to.
    /// * `range` - The range in each slice in `output` to write to.
    ///
    /// If there is no active stream, the stream is paused, or the processor end
    /// is not ready to send samples, then the output will be filled with zeros
    /// and `None` will be returned.
    pub fn read<Vin: AsMut<[f32]>>(
        &mut self,
        output: &mut [Vin],
        range: Range<usize>,
    ) -> Option<ReadStatus> {
        if !self.is_ready() {
            for ch in output.iter_mut() {
                ch.as_mut()[range.clone()].fill(0.0);
            }
            return None;
        }

        Some(
            self.active_state
                .as_mut()
                .unwrap()
                .cons
                .lock()
                .unwrap()
                .read(output, range),
        )
    }

    /// Discard a certian number of output frames from the buffer. This can be used to
    /// correct for jitter and avoid excessive overflows and reduce the percieved audible
    /// glitchiness.
    ///
    /// This will discard `frames.min(self.available_frames())` frames.
    ///
    /// Returns the number of output frames that were discarded.
    pub fn discard_frames(&mut self) -> usize {
        if let Some(state) = &mut self.active_state {
            state.cons.lock().unwrap().discard_frames(usize::MAX)
        } else {
            0
        }
    }

    /// Correct for any overflows.
    ///
    /// This returns the number of frames (samples in a single channel of audio) that were
    /// discarded due to an overflow occurring. If no overflow occured, then `None`
    /// is returned.
    ///
    /// Note, this method is already automatically called in [`StreamReaderState::read`] and
    /// [`StreamReaderState::read_interleaved`].
    ///
    /// This will have no effect if [`ResamplingChannelConfig::overflow_autocorrect_percent_threshold`]
    /// was set to `None`.
    ///
    /// This method is realtime-safe.
    pub fn autocorrect_overflows(&mut self) -> Option<usize> {
        if let Some(state) = &mut self.active_state {
            state.cons.lock().unwrap().autocorrect_overflows()
        } else {
            None
        }
    }

    /// Returns `true` if the processor end of the stream is ready to start sending
    /// data.
    pub fn is_ready(&self) -> bool {
        self.active_state.is_some()
            && self.shared_state.channel_started.load(Ordering::Relaxed)
            && !self.shared_state.paused.load(Ordering::Relaxed)
    }

    /// Pause any active audio streams.
    pub fn pause_stream(&mut self) {
        if self.is_active() {
            self.shared_state.paused.store(true, Ordering::Relaxed);
        }
    }

    /// Resume any active audio streams after pausing.
    pub fn resume(&mut self) {
        self.shared_state.paused.store(false, Ordering::Relaxed);
    }

    // Stop any active audio input streams.
    pub fn stop_stream(&mut self) {
        self.active_state = None;
        self.shared_state.reset();
    }

    pub fn handle(&self) -> Mutex<Self> {
        Mutex::new((*self).clone())
    }
}

impl Drop for StreamReaderState {
    fn drop(&mut self) {
        self.stop_stream();
    }
}

impl AudioNode for StreamReaderNode {
    type Configuration = StreamReaderConfig;

    fn info(&self, config: &Self::Configuration) -> AudioNodeInfo {
        AudioNodeInfo::new()
            .debug_name("stream_reader")
            .channel_config(ChannelConfig {
                num_inputs: config.channels.get(),
                num_outputs: ChannelCount::ZERO,
            })
            .uses_events(true)
            .custom_state(StreamReaderState::new(config.channels))
    }

    fn construct_processor(
        &self,
        _config: &Self::Configuration,
        cx: ConstructProcessorContext,
    ) -> impl AudioNodeProcessor {
        Processor {
            prod: None,
            shared_state: ArcGc::clone(
                &cx.custom_state::<StreamReaderState>().unwrap().shared_state,
            ),
        }
    }
}

#[derive(Clone)]
struct ActiveState {
    cons: Arc<Mutex<fixed_resample::ResamplingCons<f32>>>,
    sample_rate: NonZeroU32,
}

struct SharedState {
    stream_active: AtomicBool,
    channel_started: AtomicBool,
    paused: AtomicBool,
    underflow_occurred: AtomicBool,
    overflow_occurred: AtomicBool,
}

impl SharedState {
    fn new() -> Self {
        Self {
            stream_active: AtomicBool::new(false),
            channel_started: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            underflow_occurred: AtomicBool::new(false),
            overflow_occurred: AtomicBool::new(false),
        }
    }

    fn reset(&self) {
        self.stream_active.store(false, Ordering::Relaxed);
        self.channel_started.store(false, Ordering::Relaxed);
        self.paused.store(false, Ordering::Relaxed);
        self.underflow_occurred.store(false, Ordering::Relaxed);
        self.overflow_occurred.store(false, Ordering::Relaxed);
    }
}

struct Processor {
    prod: Option<fixed_resample::ResamplingProd<f32, MAX_CHANNELS>>,
    shared_state: ArcGc<SharedState>,
}

impl AudioNodeProcessor for Processor {
    fn process(
        &mut self,
        buffers: ProcBuffers,
        proc_info: &ProcInfo,
        mut events: NodeEventList,
    ) -> ProcessStatus {
        events.for_each(|event| {
            if let NodeEventType::Custom(event) = event.event {
                if let Some(out_stream_event) = event
                    .downcast_mut::<SyncWrapper<NewOutputStreamEvent>>()
                    .and_then(SyncWrapper::get_mut)
                {
                    // Swap the memory so that the old channel will be properly
                    // dropped outside of the audio thread.
                    core::mem::swap(&mut self.prod, &mut out_stream_event.prod);
                }
            }
        });

        if !self.shared_state.stream_active.load(Ordering::Relaxed)
            || self.shared_state.paused.load(Ordering::Relaxed)
        {
            return ProcessStatus::Bypass;
        }

        let Some(prod) = &mut self.prod else {
            return ProcessStatus::Bypass;
        };

        // Notify the input stream that the output stream has begun
        // reading data.
        self.shared_state
            .channel_started
            .store(true, Ordering::Relaxed);

        let status = prod.push(buffers.inputs, 0..proc_info.frames);

        match status {
            PushStatus::OverflowOccurred {
                num_frames_pushed: _,
            } => {
                self.shared_state
                    .overflow_occurred
                    .store(true, Ordering::Relaxed);
            }
            PushStatus::UnderflowCorrected {
                num_zero_frames_pushed: _,
            } => {
                self.shared_state
                    .underflow_occurred
                    .store(true, Ordering::Relaxed);
            }
            _ => {}
        }

        ProcessStatus::Bypass
    }

    fn stream_stopped(&mut self) {
        self.shared_state
            .stream_active
            .store(false, Ordering::Relaxed);
        self.prod = None;
    }
}

pub struct NewOutputStreamEvent {
    prod: Option<fixed_resample::ResamplingProd<f32, MAX_CHANNELS>>,
}

impl From<NewOutputStreamEvent> for NodeEventType {
    fn from(value: NewOutputStreamEvent) -> Self {
        NodeEventType::Custom(Box::new(SyncWrapper::new(value)))
    }
}
