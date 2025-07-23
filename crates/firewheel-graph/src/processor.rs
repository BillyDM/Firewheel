use core::{num::NonZeroU32, time::Duration};
use std::usize;

use ringbuf::traits::Producer;
use smallvec::SmallVec;
use thunderdome::Arena;

use crate::{
    backend::AudioBackend,
    context::ClearScheduledEventsType,
    graph::ScheduleHeapData,
    processor::{
        event_scheduler::{EventScheduler, NodeEventSchedulerData},
        transport::ProcTransportState,
    },
};
use firewheel_core::{
    clock::{InstantMusical, InstantSamples, TransportState},
    dsp::{buffer::ChannelBuffer, declick::DeclickValues},
    event::{NodeEvent, NodeEventListIndex},
    node::{AudioNodeProcessor, NodeID, StreamStatus, NUM_SCRATCH_BUFFERS},
    StreamInfo,
};

mod event_scheduler;
mod handle_messages;
mod process;
mod transport;

pub struct FirewheelProcessor<B: AudioBackend> {
    inner: Option<FirewheelProcessorInner<B>>,
    drop_tx: ringbuf::HeapProd<FirewheelProcessorInner<B>>,
}

impl<B: AudioBackend> Drop for FirewheelProcessor<B> {
    fn drop(&mut self) {
        let Some(mut inner) = self.inner.take() else {
            return;
        };

        inner.stream_stopped();

        // TODO: Either wait for `bevy_platform` to implement this method, or
        // hide this behind a "std" feature flag.
        if std::thread::panicking() {
            inner.poisoned = true;
        }

        let _ = self.drop_tx.try_push(inner);
    }
}

impl<B: AudioBackend> FirewheelProcessor<B> {
    pub(crate) fn new(
        processor: FirewheelProcessorInner<B>,
        drop_tx: ringbuf::HeapProd<FirewheelProcessorInner<B>>,
    ) -> Self {
        Self {
            inner: Some(processor),
            drop_tx,
        }
    }

    pub fn process_interleaved(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        num_in_channels: usize,
        num_out_channels: usize,
        frames: usize,
        process_timestamp: B::Instant,
        duration_since_stream_start: Duration,
        stream_status: StreamStatus,
        dropped_frames: u32,
    ) {
        if let Some(inner) = &mut self.inner {
            inner.process_interleaved(
                input,
                output,
                num_in_channels,
                num_out_channels,
                frames,
                process_timestamp,
                duration_since_stream_start,
                stream_status,
                dropped_frames,
            );
        }
    }
}

pub(crate) struct FirewheelProcessorInner<B: AudioBackend> {
    nodes: Arena<NodeEntry>,
    schedule_data: Option<Box<ScheduleHeapData>>,

    from_graph_rx: ringbuf::HeapCons<ContextToProcessorMsg>,
    to_graph_tx: ringbuf::HeapProd<ProcessorToContextMsg>,

    event_scheduler: EventScheduler,
    node_event_queue: Vec<NodeEventListIndex>,

    sample_rate: NonZeroU32,
    sample_rate_recip: f64,
    max_block_frames: usize,

    clock_samples: InstantSamples,
    shared_clock_input: triple_buffer::Input<SharedClock<B::Instant>>,

    proc_transport_state: ProcTransportState,

    hard_clip_outputs: bool,

    scratch_buffers: ChannelBuffer<f32, NUM_SCRATCH_BUFFERS>,
    declick_values: DeclickValues,

    /// If a panic occurs while processing, this flag is set to let the
    /// main thread know that it shouldn't try spawning a new audio stream
    /// with the shared `Arc<AtomicRefCell<FirewheelProcessorInner>>` object.
    pub(crate) poisoned: bool,
}

impl<B: AudioBackend> FirewheelProcessorInner<B> {
    /// Note, this method gets called on the main thread, not the audio thread.
    pub(crate) fn new(
        from_graph_rx: ringbuf::HeapCons<ContextToProcessorMsg>,
        to_graph_tx: ringbuf::HeapProd<ProcessorToContextMsg>,
        shared_clock_input: triple_buffer::Input<SharedClock<B::Instant>>,
        immediate_event_buffer_capacity: usize,
        scheduled_event_buffer_capacity: usize,
        node_event_buffer_capacity: usize,
        stream_info: &StreamInfo,
        hard_clip_outputs: bool,
        buffer_out_of_space_mode: BufferOutOfSpaceMode,
    ) -> Self {
        Self {
            nodes: Arena::new(),
            schedule_data: None,
            from_graph_rx,
            to_graph_tx,
            event_scheduler: EventScheduler::new(
                immediate_event_buffer_capacity,
                scheduled_event_buffer_capacity,
                buffer_out_of_space_mode,
            ),
            node_event_queue: Vec::with_capacity(node_event_buffer_capacity),
            sample_rate: stream_info.sample_rate,
            sample_rate_recip: stream_info.sample_rate_recip,
            max_block_frames: stream_info.max_block_frames.get() as usize,
            clock_samples: InstantSamples(0),
            shared_clock_input,
            proc_transport_state: ProcTransportState::new(),
            hard_clip_outputs,
            scratch_buffers: ChannelBuffer::new(stream_info.max_block_frames.get() as usize),
            declick_values: DeclickValues::new(stream_info.declick_frames),
            poisoned: false,
        }
    }
}

pub(crate) struct NodeEntry {
    pub processor: Box<dyn AudioNodeProcessor>,

    event_data: NodeEventSchedulerData,
}

pub(crate) enum ContextToProcessorMsg {
    EventGroup(Vec<NodeEvent>),
    NewSchedule(Box<ScheduleHeapData>),
    HardClipOutputs(bool),
    SetTransportState(Box<TransportState>),
    ClearScheduledEvents(SmallVec<[ClearScheduledEventsEvent; 1]>),
}

pub(crate) enum ProcessorToContextMsg {
    ReturnEventGroup(Vec<NodeEvent>),
    ReturnSchedule(Box<ScheduleHeapData>),
    ReturnTransportState(Box<TransportState>),
    ReturnClearScheduledEvents(SmallVec<[ClearScheduledEventsEvent; 1]>),
}

pub(crate) struct ClearScheduledEventsEvent {
    /// If `None`, then clear events for all nodes.
    pub node_id: Option<NodeID>,
    pub event_type: ClearScheduledEventsType,
}

#[derive(Clone)]
pub(crate) struct SharedClock<I: Clone> {
    pub clock_samples: InstantSamples,
    pub musical_time: Option<InstantMusical>,
    pub transport_is_playing: bool,
    pub process_timestamp: Option<I>,
}

impl<I: Clone> Default for SharedClock<I> {
    fn default() -> Self {
        Self {
            clock_samples: InstantSamples(0),
            musical_time: None,
            transport_is_playing: false,
            process_timestamp: None,
        }
    }
}

/// How to handle event buffers on the audio thread running out of space.
#[derive(Default, Debug, Clone, Copy, PartialEq, PartialOrd)]
pub enum BufferOutOfSpaceMode {
    #[default]
    /// If an event buffer on the audio thread ran out of space to fit new
    /// events, reallocate on the audio thread to fit the new items. If this
    /// happens, it may cause underruns (audio glitches), and a warning will
    /// be logged.
    AllocateOnAudioThread,
    /// If an event buffer on the audio thread ran out of space to fit new
    /// events, then panic.
    Panic,
    /// If an event buffer on the audio thread ran out of space to fit new
    /// events, drop those events to avoid allocating on the audio thread.
    /// If this happens, a warning will be logged.
    ///
    /// (Not generally recommended, but the option is here if you want it.)
    DropEvents,
}
