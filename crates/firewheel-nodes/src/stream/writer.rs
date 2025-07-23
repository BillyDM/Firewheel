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
    dsp::declick::{Declicker, FadeType},
    event::{NodeEventList, NodeEventType},
    node::{
        AudioNode, AudioNodeInfo, AudioNodeProcessor, ConstructProcessorContext, ProcBuffers,
        ProcInfo, ProcessStatus,
    },
    SilenceMask,
};
use fixed_resample::{ReadStatus, ResamplingChannelConfig};

pub use fixed_resample::PushStatus;

pub const MAX_CHANNELS: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "bevy", derive(bevy_ecs::prelude::Component))]
pub struct StreamWriterConfig {
    /// The number of channels.
    pub channels: NonZeroChannelCount,

    /// Whether or not to check for silence in the input stream. Highly
    /// recommened to set this to `true` to improve audio graph performance
    /// when there is no input on the microphone.
    ///
    /// By default this is set to `true`.
    pub check_for_silence: bool,
}

impl Default for StreamWriterConfig {
    fn default() -> Self {
        Self {
            channels: NonZeroChannelCount::STEREO,
            check_for_silence: true,
        }
    }
}

#[derive(Default, Debug, Clone, Copy)]
#[cfg_attr(feature = "bevy", derive(bevy_ecs::prelude::Component))]
pub struct StreamWriterNode;

#[derive(Clone)]
pub struct StreamWriterState {
    channels: NonZeroChannelCount,
    active_state: Option<ActiveState>,
    shared_state: ArcGc<SharedState>,
}

impl StreamWriterState {
    fn new(channels: NonZeroChannelCount) -> Self {
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
    /// underflow occurred.)
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

    /// The total number of frames (not samples) that can currently be pushed to the stream.
    ///
    /// If there is no active stream, the stream is paused, or the processor end
    /// is not ready to receive samples, then this will return `0`.
    pub fn available_frames(&self) -> usize {
        if self.is_ready() {
            self.active_state
                .as_ref()
                .map(|s| s.prod.lock().unwrap().available_frames())
                .unwrap_or(0)
        } else {
            0
        }
    }

    /// The amount of data in seconds that is currently occupied in the channel.
    ///
    /// This value will be in the range `[0.0, ResamplingChannelConfig::capacity_seconds]`.
    ///
    /// If there is no active stream, then this will return `None`.
    pub fn occupied_seconds(&self) -> Option<f64> {
        self.active_state
            .as_ref()
            .map(|s| s.prod.lock().unwrap().occupied_seconds())
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

    /// Begin the input audio stream on this node.
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
    ) -> Result<NewInputStreamEvent, ()> {
        if self.is_active() {
            return Err(());
        }

        self.shared_state.reset();

        let (prod, cons) = fixed_resample::resampling_channel::<f32, MAX_CHANNELS>(
            NonZeroUsize::new(self.channels.get().get() as usize).unwrap(),
            sample_rate.get(),
            output_stream_sample_rate.get(),
            channel_config,
        );

        self.active_state = Some(ActiveState {
            prod: Arc::new(Mutex::new(prod)),
            sample_rate,
        });
        self.shared_state
            .stream_active
            .store(true, Ordering::Relaxed);

        Ok(NewInputStreamEvent { cons: Some(cons) })
    }

    /// Push the given data in interleaved format.
    ///
    /// Returns the number of frames (not samples) that were successfully pushed.
    /// If this number is less than the number of frames in `data`, then it means
    /// an overflow has occured.
    ///
    /// If there is no active stream, the stream is paused, or the processor end
    /// is not ready to receive samples, then no data will be sent and this will
    /// return `0`.
    pub fn push_interleaved(&mut self, data: &[f32]) -> PushStatus {
        if !self.is_ready() {
            return PushStatus::OutputNotReady;
        }

        self.active_state
            .as_mut()
            .unwrap()
            .prod
            .lock()
            .unwrap()
            .push_interleaved(data)
    }

    /// Push the given data in de-interleaved format.
    ///
    /// * `data` - The channels of data to push to the channel.
    /// * `range` - The range in each slice in `input` to read data from.
    ///
    /// Returns the number of frames (not samples) that were successfully pushed.
    /// If this number is less than the number of frames in `data`, then it means
    /// an overflow has occured.
    ///b
    /// If there is no active stream, the stream is paused, or the processor end
    /// is not ready to receive samples, then no data will be sent and this will
    /// return `0`.
    pub fn push<Vin: AsRef<[f32]>>(&mut self, data: &[Vin], range: Range<usize>) -> PushStatus {
        if !self.is_ready() {
            return PushStatus::OutputNotReady;
        }

        self.active_state
            .as_mut()
            .unwrap()
            .prod
            .lock()
            .unwrap()
            .push(data, range)
    }

    /// Returns `true` if the processor end of the stream is ready to start receiving
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

    /// Correct for any underflows.
    ///
    /// This returns the number of extra zero frames (samples in a single channel of audio)
    /// that were added due to an underflow occurring. If no underflow occured, then `None`
    /// is returned.
    ///
    /// Note, this method is already automatically called in [`StreamWriterState::push`] and
    /// [`StreamWriterState::push_interleaved`].
    ///
    /// This will have no effect if [`ResamplingChannelConfig::underflow_autocorrect_percent_threshold`]
    /// was set to `None`.
    ///
    /// This method is realtime-safe.
    pub fn autocorrect_underflows(&mut self) -> Option<usize> {
        if let Some(state) = &mut self.active_state {
            state.prod.lock().unwrap().autocorrect_underflows()
        } else {
            None
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

impl Drop for StreamWriterState {
    fn drop(&mut self) {
        self.stop_stream();
    }
}

impl AudioNode for StreamWriterNode {
    type Configuration = StreamWriterConfig;

    fn info(&self, config: &Self::Configuration) -> AudioNodeInfo {
        AudioNodeInfo::new()
            .debug_name("stream_writer")
            .channel_config(ChannelConfig {
                num_inputs: ChannelCount::ZERO,
                num_outputs: config.channels.get(),
            })
            .custom_state(StreamWriterState::new(config.channels))
    }

    fn construct_processor(
        &self,
        config: &Self::Configuration,
        cx: ConstructProcessorContext,
    ) -> impl AudioNodeProcessor {
        Processor {
            cons: None,
            shared_state: ArcGc::clone(
                &cx.custom_state::<StreamWriterState>().unwrap().shared_state,
            ),
            check_for_silence: config.check_for_silence,
            pause_declicker: Declicker::SettledAt0,
        }
    }
}

#[derive(Clone)]
struct ActiveState {
    prod: Arc<Mutex<fixed_resample::ResamplingProd<f32, MAX_CHANNELS>>>,
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
    cons: Option<fixed_resample::ResamplingCons<f32>>,
    shared_state: ArcGc<SharedState>,
    check_for_silence: bool,
    pause_declicker: Declicker,
}

impl AudioNodeProcessor for Processor {
    fn process(
        &mut self,
        buffers: ProcBuffers,
        proc_info: &ProcInfo,
        events: &mut NodeEventList,
    ) -> ProcessStatus {
        for mut event in events.drain() {
            if let Some(in_stream_event) = event.downcast_mut::<NewInputStreamEvent>() {
                // Swap the values so that the old consumer gets dropped on
                // the main thread.
                core::mem::swap(&mut self.cons, &mut in_stream_event.cons);
            }
        }

        let enabled = self.shared_state.stream_active.load(Ordering::Relaxed)
            && !self.shared_state.paused.load(Ordering::Relaxed);

        self.pause_declicker
            .fade_to_enabled(enabled, proc_info.declick_values);

        if self.pause_declicker.disabled() {
            return ProcessStatus::ClearAllOutputs;
        }

        let Some(cons) = &mut self.cons else {
            self.pause_declicker.reset_to_0();
            return ProcessStatus::ClearAllOutputs;
        };

        // Notify the input stream that the output stream has begun
        // reading data.
        self.shared_state
            .channel_started
            .store(true, Ordering::Relaxed);

        let status = cons.read(buffers.outputs, 0..proc_info.frames);

        match status {
            ReadStatus::UnderflowOccurred { num_frames_read: _ } => {
                self.shared_state
                    .underflow_occurred
                    .store(true, Ordering::Relaxed);
            }
            ReadStatus::OverflowCorrected {
                num_frames_discarded: _,
            } => {
                self.shared_state
                    .overflow_occurred
                    .store(true, Ordering::Relaxed);
            }
            _ => {}
        }

        if !self.pause_declicker.is_settled() {
            self.pause_declicker.process(
                buffers.outputs,
                0..proc_info.frames,
                proc_info.declick_values,
                1.0,
                FadeType::EqualPower3dB,
            );
        }

        let mut silence_mask = SilenceMask::NONE_SILENT;
        if self.check_for_silence {
            let resampler_channels = cons.num_channels().get();

            for (ch_i, ch) in buffers.outputs.iter().enumerate() {
                if ch_i >= resampler_channels {
                    // `cons.read()` clears any extra channels
                    silence_mask.set_channel(ch_i, true);
                } else {
                    let mut all_silent = true;
                    for &s in ch[..proc_info.frames].iter() {
                        if s != 0.0 {
                            all_silent = false;
                            break;
                        }
                    }

                    if all_silent {
                        silence_mask.set_channel(ch_i, true);
                    }
                }
            }
        }

        ProcessStatus::outputs_modified(silence_mask)
    }

    fn stream_stopped(&mut self) {
        self.shared_state
            .stream_active
            .store(false, Ordering::Relaxed);
        self.cons = None;
        self.pause_declicker.reset_to_0();
    }
}

pub struct NewInputStreamEvent {
    cons: Option<fixed_resample::ResamplingCons<f32>>,
}

impl From<NewInputStreamEvent> for NodeEventType {
    fn from(value: NewInputStreamEvent) -> Self {
        NodeEventType::custom(value)
    }
}
