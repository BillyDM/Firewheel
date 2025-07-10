use core::time::Duration;
use core::{any::Any, fmt::Debug, hash::Hash, num::NonZeroU32, ops::Range};

use crate::clock::EventInstant;
use crate::{
    channel_config::{ChannelConfig, ChannelCount},
    clock::{InstantMusical, InstantSamples, InstantSeconds, MusicalTransport},
    dsp::declick::DeclickValues,
    event::{NodeEvent, NodeEventList, NodeEventType},
    SilenceMask, StreamInfo,
};

pub mod dummy;

/// A globally unique identifier for a node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeID(pub thunderdome::Index);

impl NodeID {
    pub const DANGLING: Self = Self(thunderdome::Index::DANGLING);
}

impl Default for NodeID {
    fn default() -> Self {
        Self::DANGLING
    }
}

/// Information about an [`AudioNode`].
///
/// This struct enforces the use of the builder pattern for future-proofness, as
/// it is likely that more fields will be added in the future.
#[derive(Debug)]
pub struct AudioNodeInfo {
    debug_name: &'static str,
    channel_config: ChannelConfig,
    uses_events: bool,
    call_update_method: bool,
    custom_state: Option<Box<dyn Any>>,
}

impl AudioNodeInfo {
    /// Construct a new [`AudioNodeInfo`] builder struct.
    pub const fn new() -> Self {
        Self {
            debug_name: "unnamed",
            channel_config: ChannelConfig {
                num_inputs: ChannelCount::ZERO,
                num_outputs: ChannelCount::ZERO,
            },
            uses_events: false,
            call_update_method: false,
            custom_state: None,
        }
    }

    /// A unique name for this type of node, used for debugging purposes.
    pub const fn debug_name(mut self, debug_name: &'static str) -> Self {
        self.debug_name = debug_name;
        self
    }

    /// The channel configuration of this node.
    ///
    /// By default this has a channel configuration with zero input and output
    /// channels.
    pub const fn channel_config(mut self, channel_config: ChannelConfig) -> Self {
        self.channel_config = channel_config;
        self
    }

    /// Set to `true` if this node type uses events, `false` otherwise.
    ///
    /// Setting to `false` will help the system save some memory by not
    /// allocating an event buffer for this node.
    ///
    /// By default this is set to `false`.
    pub const fn uses_events(mut self, uses_events: bool) -> Self {
        self.uses_events = uses_events;
        self
    }

    /// Set to `true` if this node wishes to have the Firewheel context call
    /// [`AudioNode::update`] on every update cycle.
    ///
    /// By default this is set to `false`.
    pub const fn call_update_method(mut self, call_update_method: bool) -> Self {
        self.call_update_method = call_update_method;
        self
    }

    /// Custom `!Send` state that can be stored in the Firewheel context and accessed
    /// by the user.
    ///
    /// The user accesses this state via `FirewheelCtx::node_state` and
    /// `FirewheelCtx::node_state_mut`.
    pub fn custom_state<T: 'static>(mut self, custom_state: T) -> Self {
        self.custom_state = Some(Box::new(custom_state));
        self
    }
}

/// Information about an [`AudioNode`]. Used internally by the Firewheel context.
#[derive(Debug)]
pub struct AudioNodeInfoInner {
    pub debug_name: &'static str,
    pub channel_config: ChannelConfig,
    pub uses_events: bool,
    pub call_update_method: bool,
    pub custom_state: Option<Box<dyn Any>>,
}

impl Into<AudioNodeInfoInner> for AudioNodeInfo {
    fn into(self) -> AudioNodeInfoInner {
        AudioNodeInfoInner {
            debug_name: self.debug_name,
            channel_config: self.channel_config,
            uses_events: self.uses_events,
            call_update_method: self.call_update_method,
            custom_state: self.custom_state,
        }
    }
}

/// A trait representing a node in a Firewheel audio graph.
///
/// # Notes about ECS
///
/// In order to be friendlier to ECS's (entity component systems), it is encouraged
/// that any struct deriving this trait be POD (plain ol' data). If you want your
/// audio node to be usable in the Bevy game engine, also derive
/// `bevy_ecs::prelude::Component`. (You can hide this derive behind a feature flag
/// by using `#[cfg_attr(feature = "bevy", derive(bevy_ecs::prelude::Component))]`).
///
/// # Audio Node Lifecycle
///
/// 1. The user constructs the node as POD or from a custom constructor method for
/// that node.
/// 2. The user adds the node to the graph using `FirewheelCtx::add_node`. If the
/// node has any custom configuration, then the user passes that configuration to this
/// method as well. In this method, the Firewheel context calls [`AudioNode::info`] to
/// get information about the node. The node can also store any custom state in the
/// [`AudioNodeInfo`] struct.
/// 3. At this point the user may now call `FirewheelCtx::node_state` and
/// `FirewheelCtx::node_state_mut` to retrieve the node's custom state.
/// 4. If [`AudioNodeInfo::call_update_method`] was set to `true`, then
/// [`AudioNode::update`] will be called every time the Firewheel context updates.
/// The node's custom state is also accessible in this method.
/// 5. When the Firewheel context is ready for the node to start processing data,
/// it calls [`AudioNode::construct_processor`] to retrieve the realtime
/// [`AudioNodeProcessor`] counterpart of the node. This processor counterpart is
/// then sent to the audio thread.
/// 6. The Firewheel processor calls [`AudioNodeProcessor::process`] whenever there
/// is a new block of audio data to process.
/// 7. (Graceful shutdown)
///
///     7a. The Firewheel processor calls [`AudioNodeProcessor::stream_stopped`].
/// The processor is then sent back to the main thread.
///
///     7b. If a new audio stream is started, then the context will call
/// [`AudioNodeProcessor::new_stream`] on the main thread, and then send the
/// processor back to the audio thread for processing.
///
///     7c. If the Firewheel context is dropped before a new stream is started, then
/// both the node and the processor counterpart are dropped.
/// 8. (Audio thread crashes or stops unexpectedly) - The node's processor counterpart
/// may or may not be dropped. The user may try to create a new audio stream, in which
/// case [`AudioNode::construct_processor`] might be called again. If a second processor
/// instance is not able to be created, then the node may panic.
pub trait AudioNode {
    /// A type representing this constructor's configuration.
    ///
    /// This is intended as a one-time configuration to be used
    /// when constructing an audio node. When no configuration
    /// is required, [`EmptyConfig`] should be used.
    type Configuration: Default;

    /// Get information about this node.
    ///
    /// This method is only called once after the node is added to the audio graph.
    fn info(&self, configuration: &Self::Configuration) -> AudioNodeInfo;

    /// Construct a realtime processor for this node.
    ///
    /// * `configuration` - The custom configuration of this node.
    /// * `cx` - A context for interacting with the Firewheel context. This context
    /// also includes information about the audio stream.
    fn construct_processor(
        &self,
        configuration: &Self::Configuration,
        cx: ConstructProcessorContext,
    ) -> impl AudioNodeProcessor;

    /// If [`AudioNodeInfo::call_update_method`] was set to `true`, then the Firewheel
    /// context will call this method on every update cycle.
    ///
    /// * `configuration` - The custom configuration of this node.
    /// * `cx` - A context for interacting with the Firewheel context.
    fn update(&mut self, configuration: &Self::Configuration, cx: UpdateContext) {
        let _ = configuration;
        let _ = cx;
    }
}

/// A context for [`AudioNode::construct_processor`].
pub struct ConstructProcessorContext<'a> {
    /// The ID of this audio node.
    pub node_id: NodeID,
    /// Information about the running audio stream.
    pub stream_info: &'a StreamInfo,
    custom_state: &'a mut Option<Box<dyn Any>>,
}

impl<'a> ConstructProcessorContext<'a> {
    pub fn new(
        node_id: NodeID,
        stream_info: &'a StreamInfo,
        custom_state: &'a mut Option<Box<dyn Any>>,
    ) -> Self {
        Self {
            node_id,
            stream_info,
            custom_state,
        }
    }

    /// Get an immutable reference to the custom state that was created in
    /// [`AudioNodeInfo::custom_state`].
    pub fn custom_state<T: 'static>(&self) -> Option<&T> {
        self.custom_state
            .as_ref()
            .and_then(|s| s.downcast_ref::<T>())
    }

    /// Get a mutable reference to the custom state that was created in
    /// [`AudioNodeInfo::custom_state`].
    pub fn custom_state_mut<T: 'static>(&mut self) -> Option<&mut T> {
        self.custom_state
            .as_mut()
            .and_then(|s| s.downcast_mut::<T>())
    }
}

/// A context for [`AudioNode::update`].
pub struct UpdateContext<'a> {
    /// The ID of this audio node.
    pub node_id: NodeID,
    /// Information about the running audio stream. If no audio stream is running,
    /// then this will be `None`.
    pub stream_info: Option<&'a StreamInfo>,
    custom_state: &'a mut Option<Box<dyn Any>>,
    event_queue: &'a mut Vec<NodeEvent>,
}

impl<'a> UpdateContext<'a> {
    pub fn new(
        node_id: NodeID,
        stream_info: Option<&'a StreamInfo>,
        custom_state: &'a mut Option<Box<dyn Any>>,
        event_queue: &'a mut Vec<NodeEvent>,
    ) -> Self {
        Self {
            node_id,
            stream_info,
            custom_state,
            event_queue,
        }
    }

    /// Queue an event to send to this node's processor counterpart.
    pub fn queue_event(&mut self, event: NodeEventType) {
        self.event_queue.push(NodeEvent {
            node_id: self.node_id,
            time: None,
            event,
        });
    }

    /// Queue an event to send to this node's processor counterpart, at a certain time.
    pub fn schedule_event(&mut self, event: NodeEventType, time: EventInstant) {
        self.event_queue.push(NodeEvent {
            node_id: self.node_id,
            time: Some(time),
            event,
        });
    }

    /// Get an immutable reference to the custom state that was created in
    /// [`AudioNodeInfo::custom_state`].
    pub fn custom_state<T: 'static>(&self) -> Option<&T> {
        self.custom_state
            .as_ref()
            .and_then(|s| s.downcast_ref::<T>())
    }

    /// Get a mutable reference to the custom state that was created in
    /// [`AudioNodeInfo::custom_state`].
    pub fn custom_state_mut<T: 'static>(&mut self) -> Option<&mut T> {
        self.custom_state
            .as_mut()
            .and_then(|s| s.downcast_mut::<T>())
    }
}

/// An empty constructor configuration.
///
/// This should be preferred over `()` because it implements
/// Bevy's `Component` trait, making the
/// [`AudioNode`] implementor trivially Bevy-compatible.
#[derive(Debug, Default, Clone, Copy)]
#[cfg_attr(feature = "bevy", derive(bevy_ecs::prelude::Component))]
pub struct EmptyConfig;

/// A type-erased dyn-compatible [`AudioNode`].
pub trait DynAudioNode {
    /// Get information about this node.
    ///
    /// This method is only called once after the node is added to the audio graph.
    fn info(&self) -> AudioNodeInfo;

    /// Construct a realtime processor for this node.
    ///
    /// * `cx` - A context for interacting with the Firewheel context. This context
    /// also includes information about the audio stream.
    fn construct_processor(&self, cx: ConstructProcessorContext) -> Box<dyn AudioNodeProcessor>;

    /// If [`AudioNodeInfo::call_update_method`] was set to `true`, then the Firewheel
    /// context will call this method on every update cycle.
    ///
    /// * `cx` - A context for interacting with the Firewheel context.
    fn update(&mut self, cx: UpdateContext) {
        let _ = cx;
    }
}

/// Pairs constructors with their configurations.
///
/// This is useful for type-erasing an [`AudioNode`].
pub struct Constructor<T, C> {
    constructor: T,
    configuration: C,
}

impl<T: AudioNode> Constructor<T, T::Configuration> {
    pub fn new(constructor: T, configuration: Option<T::Configuration>) -> Self {
        Self {
            constructor,
            configuration: configuration.unwrap_or_default(),
        }
    }
}

impl<T: AudioNode> DynAudioNode for Constructor<T, T::Configuration> {
    fn info(&self) -> AudioNodeInfo {
        self.constructor.info(&self.configuration)
    }

    fn construct_processor(&self, cx: ConstructProcessorContext) -> Box<dyn AudioNodeProcessor> {
        Box::new(
            self.constructor
                .construct_processor(&self.configuration, cx),
        )
    }

    fn update(&mut self, cx: UpdateContext) {
        self.constructor.update(&self.configuration, cx);
    }
}

/// The trait describing the realtime processor counterpart to an
/// audio node.
pub trait AudioNodeProcessor: 'static + Send {
    /// Process the given block of audio. Only process data in the
    /// buffers up to `samples`.
    ///
    /// The node *MUST* either return `ProcessStatus::ClearAllOutputs`
    /// or fill all output buffers with data.
    ///
    /// If any output buffers contain all zeros up to `samples` (silent),
    /// then mark that buffer as silent in [`ProcInfo::out_silence_mask`].
    ///
    /// * `buffers` - The buffers of data to process.
    /// * `proc_info` - Additional information about the process.
    /// * `events` - A list of events for this node to process.
    fn process(
        &mut self,
        buffers: ProcBuffers,
        proc_info: &ProcInfo,
        events: NodeEventList,
    ) -> ProcessStatus;

    /// Called when the audio stream has been stopped.
    fn stream_stopped(&mut self) {}

    /// Called when a new audio stream has been started after a previous
    /// call to [`AudioNodeProcessor::stream_stopped`].
    ///
    /// Note, this method gets called on the main thread, not the audio
    /// thread. So it is safe to allocate/deallocate here.
    fn new_stream(&mut self, stream_info: &StreamInfo) {
        let _ = stream_info;
    }
}

pub const NUM_SCRATCH_BUFFERS: usize = 8;

/// The buffers used in [`AudioNodeProcessor::process`].
pub struct ProcBuffers<'a, 'b, 'c, 'd> {
    /// The audio input buffers.
    ///
    /// The number of channels will always equal the [`ChannelConfig::num_inputs`]
    /// value that was returned in [`AudioNode::info`].
    ///
    /// Each channel slice will have a length of [`ProcInfo::frames`].
    pub inputs: &'a [&'b [f32]],

    /// The audio input buffers.
    ///
    /// The number of channels will always equal the [`ChannelConfig::num_outputs`]
    /// value that was returned in [`AudioNode::info`].
    ///
    /// Each channel slice will have a length of [`ProcInfo::frames`].
    ///
    /// These buffers may contain junk data.
    pub outputs: &'a mut [&'b mut [f32]],

    /// A list of extra scratch buffers that can be used for processing.
    /// This removes the need for nodes to allocate their own scratch buffers.
    /// Each buffer has a length of [`StreamInfo::max_block_frames`]. These
    /// buffers are shared across all nodes, so assume that they contain junk
    /// data.
    pub scratch_buffers: &'c mut [&'d mut [f32]; NUM_SCRATCH_BUFFERS],
}

/// Additional information for processing audio
pub struct ProcInfo<'a> {
    /// The number of frames (samples in a single channel of audio) in
    /// this processing block.
    ///
    /// Not to be confused with video frames.
    pub frames: usize,

    /// An optional optimization hint on which input channels contain
    /// all zeros (silence). The first bit (`0b1`) is the first channel,
    /// the second bit is the second channel, and so on.
    pub in_silence_mask: SilenceMask,

    /// An optional optimization hint on which output channels contain
    /// all zeros (silence). The first bit (`0b1`) is the first channel,
    /// the second bit is the second channel, and so on.
    pub out_silence_mask: SilenceMask,

    /// The sample rate of the audio stream in samples per second.
    pub sample_rate: NonZeroU32,

    /// The reciprocal of the sample rate. This can be used to avoid a
    /// division and improve performance.
    pub sample_rate_recip: f64,

    /// The current time of the audio clock, equal to the total number of
    /// frames (samples in a single channel of audio) that have been
    /// processed since this Firewheel context was first started.
    ///
    /// The start of the range is the instant of time at the first frame
    /// in the block (inclusive), and the end of the range is the instant
    /// of time at the end of the block (exclusive).
    ///
    /// Note, this value does *NOT* account for any output underflows
    /// (underruns) that may have occured.
    ///
    /// Note, generally this value will always count up, but there may be
    /// a few edge cases that cause this value to be less than the previous
    /// block, such as when the sample rate of the stream has been changed.
    pub clock_samples: Range<InstantSamples>,

    /// The current time of the audio clock, equal to the total amount of
    /// data in seconds that have been processed since this Firewheel
    /// context was first started.
    ///
    /// The start of the range is the instant of time at the first frame
    /// in the block (inclusive), and the end of the range is the instant
    /// of time at the end of the block (exclusive).
    ///
    /// Note, this value does *NOT* account for any output underflows
    /// (underruns) that may have occured.
    pub clock_seconds: Range<InstantSeconds>,

    /// The duration between when the stream was started an when the
    /// Firewheel processor's `process` method was called.
    ///
    /// Note, this clock is not as accurate as the audio clock.
    pub duration_since_stream_start: Duration,

    /// Information about the musical transport.
    ///
    /// This will be `None` if no musical transport is currently active,
    /// or if the current transport is currently paused.
    pub transport_info: Option<TransportInfo<'a>>,

    /// Flags indicating the current status of the audio stream
    pub stream_status: StreamStatus,

    /// If an output underflow (underrun) occured, then this will contain
    /// an estimate for the number of frames (samples in a single channel
    /// of audio) that were dropped.
    ///
    /// This can be used to correct the timing of events if desired.
    ///
    /// Note, this is just an estimate, and may not always be perfectly
    /// accurate.
    ///
    /// If an underrun did not occur, then this will be `0`.
    pub dropped_frames: u32,

    /// A buffer of values that linearly ramp up/down between `0.0` and `1.0`
    /// which can be used to implement efficient declicking when
    /// pausing/resuming/stopping.
    pub declick_values: &'a DeclickValues,
}

#[derive(Clone)]
pub struct TransportInfo<'a> {
    /// The current transport.
    pub transport: &'a MusicalTransport,

    /// The current time of the musical transport.
    ///
    /// The start of the range is the instant of time at the first frame in
    /// the block (inclusive), and the end of the range is the instant of
    /// time at the end of the block (exclusive).
    ///
    /// This will be `None` if no musical clock is currently present.
    ///
    /// Note, this value does *NOT* account for any output underflows
    /// (underruns) that may have occured.
    pub clock_musical: Range<InstantMusical>,

    /// Whether or not the transport is currently playing (true) or paused
    /// (false).
    pub playing: bool,

    /// The instant that `MusicaltTime::ZERO` occured in units of
    /// `ClockSamples`.
    ///
    /// If the transport is not currently playing, then this will be `None`.
    pub start_clock_samples: Option<InstantSamples>,

    /// The beats per minute at the first frame of this process block.
    pub beats_per_minute: f64,

    /// The rate at which `beats_per_minute` changes each frame in this
    /// processing block.
    ///
    /// For example, if this value is `0.0`, then the bpm remains static for
    /// the entire duration of this processing block.
    ///
    /// And for example, if this is `0.1`, then the bpm increases by `0.1`
    /// each frame, and if this is `-0.1`, then the bpm decreased by `0.1`
    /// each frame.
    pub delta_bpm_per_frame: f64,
}

bitflags::bitflags! {
    /// Flags indicating the current status of the audio stream
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct StreamStatus: u32 {
        /// Some input data was discarded because of an overflow condition
        /// at the audio driver.
        const INPUT_OVERFLOW = 0b01;

        /// The output buffer ran low, likely producing a break in the
        /// output sound. (This is also known as an "underrun").
        const OUTPUT_UNDERFLOW = 0b10;
    }
}

/// The status of processing buffers in an audio node.
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessStatus {
    /// No output buffers were modified. If this is returned, then
    /// the engine will automatically clear all output buffers
    /// for you as efficiently as possible.
    #[default]
    ClearAllOutputs,
    /// No output buffers were modified. If this is returned, then
    /// the engine will automatically copy the input buffers to
    /// their corresponding output buffers for you as efficiently
    /// as possible.
    Bypass,
    /// All output buffers were filled with data.
    OutputsModified { out_silence_mask: SilenceMask },
}

impl ProcessStatus {
    /// All output buffers were filled with non-silence.
    pub const fn outputs_not_silent() -> Self {
        Self::OutputsModified {
            out_silence_mask: SilenceMask::NONE_SILENT,
        }
    }

    /// All output buffers were filled with data.
    pub const fn outputs_modified(out_silence_mask: SilenceMask) -> Self {
        Self::OutputsModified { out_silence_mask }
    }
}
