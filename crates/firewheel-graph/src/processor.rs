use bevy_platform::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use core::num::NonZeroU32;
use ringbuf::traits::Producer;
use thunderdome::Arena;

#[cfg(not(feature = "std"))]
use bevy_platform::prelude::{Box, Vec};

#[cfg(feature = "scheduled_events")]
use bevy_platform::time::Instant;

use firewheel_core::{
    clock::InstantSamples,
    dsp::{buffer::ChannelBuffer, declick::DeclickValues},
    event::{NodeEvent, ProcEventsIndex},
    node::{AudioNodeProcessor, ProcExtra},
    StreamInfo,
};

use crate::{
    backend::BackendProcessInfo,
    context::{FirewheelFlags, ProcessorChannel},
    graph::ScheduleHeapData,
    processor::event_scheduler::{EventScheduler, NodeEventSchedulerData},
};

#[cfg(feature = "scheduled_events")]
use crate::context::ClearScheduledEventsType;
#[cfg(feature = "scheduled_events")]
use firewheel_core::node::NodeID;
#[cfg(feature = "scheduled_events")]
use smallvec::SmallVec;

#[cfg(feature = "musical_transport")]
use firewheel_core::clock::{InstantMusical, TransportState};

mod event_scheduler;
mod handle_messages;
mod process;

#[cfg(feature = "musical_transport")]
mod transport;
#[cfg(feature = "musical_transport")]
use transport::ProcTransportState;

pub struct FirewheelProcessor {
    inner: Option<FirewheelProcessorInner>,
    drop_tx: ringbuf::HeapProd<FirewheelProcessorInner>,
    drop_flag: Arc<AtomicBool>,
}

impl Drop for FirewheelProcessor {
    fn drop(&mut self) {
        self.drop_inner();
    }
}

impl FirewheelProcessor {
    pub(crate) fn new(
        processor: FirewheelProcessorInner,
        drop_tx: ringbuf::HeapProd<FirewheelProcessorInner>,
        drop_flag: Arc<AtomicBool>,
    ) -> Self {
        Self {
            inner: Some(processor),
            drop_tx,
            drop_flag,
        }
    }

    pub fn process_interleaved(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        info: BackendProcessInfo,
    ) {
        self.poll_drop_flag();

        if let Some(inner) = &mut self.inner {
            inner.process_interleaved(input, output, info);
        } else {
            output.fill(0.0);
        }
    }

    fn poll_drop_flag(&mut self) {
        if self.inner.is_some() && self.drop_flag.load(Ordering::Relaxed) {
            self.drop_inner();
        }
    }

    fn drop_inner(&mut self) {
        let Some(mut inner) = self.inner.take() else {
            return;
        };

        inner.stream_stopped();

        // TODO: Remove this feature gate if `bevy_platform` implements this.
        #[cfg(feature = "std")]
        if std::thread::panicking() {
            inner.poisoned = true;
        }

        let _ = self.drop_tx.try_push(inner);
    }
}

pub(crate) struct FirewheelProcessorInner {
    nodes: Arena<NodeEntry>,
    schedule_data: Option<Box<ScheduleHeapData>>,

    from_graph_rx: ringbuf::HeapCons<ContextToProcessorMsg>,
    to_graph_tx: ringbuf::HeapProd<ProcessorToContextMsg>,

    event_scheduler: EventScheduler,
    proc_event_queue: Vec<ProcEventsIndex>,

    sample_rate: NonZeroU32,
    sample_rate_recip: f64,
    max_block_frames: usize,

    clock_samples: InstantSamples,
    #[cfg(feature = "scheduled_events")]
    shared_clock_input: triple_buffer::Input<SharedClock>,

    #[cfg(feature = "musical_transport")]
    proc_transport_state: ProcTransportState,

    flags: FirewheelFlags,
    status_flags: Arc<StatusFlags>,

    pub(crate) extra: ProcExtra,

    /// If a panic occurs while processing, this flag is set to let the
    /// main thread know that it shouldn't try spawning a new audio stream
    /// with the shared `Arc<AtomicRefCell<FirewheelProcessorInner>>` object.
    pub(crate) poisoned: bool,
}

impl FirewheelProcessorInner {
    /// Note, this method gets called on the main thread, not the audio thread.
    pub(crate) fn new(
        proc_channel: ProcessorChannel,
        immediate_event_buffer_capacity: usize,
        #[cfg(feature = "scheduled_events")] scheduled_event_buffer_capacity: usize,
        node_event_buffer_capacity: usize,
        stream_info: &StreamInfo,
        flags: FirewheelFlags,
        status_flags: Arc<StatusFlags>,
        buffer_out_of_space_mode: BufferOutOfSpaceMode,
    ) -> Self {
        let ProcessorChannel {
            from_context_rx,
            to_context_tx,
            #[cfg(feature = "scheduled_events")]
            shared_clock_input,
            logger,
            store,
        } = proc_channel;
        Self {
            nodes: Arena::new(),
            schedule_data: None,
            from_graph_rx: from_context_rx,
            to_graph_tx: to_context_tx,
            event_scheduler: EventScheduler::new(
                immediate_event_buffer_capacity,
                #[cfg(feature = "scheduled_events")]
                scheduled_event_buffer_capacity,
                buffer_out_of_space_mode,
            ),
            proc_event_queue: Vec::with_capacity(node_event_buffer_capacity),
            sample_rate: stream_info.sample_rate,
            sample_rate_recip: stream_info.sample_rate_recip,
            max_block_frames: stream_info.max_block_frames.get() as usize,
            clock_samples: InstantSamples(0),
            #[cfg(feature = "scheduled_events")]
            shared_clock_input,
            #[cfg(feature = "musical_transport")]
            proc_transport_state: ProcTransportState::new(),
            flags,
            status_flags,
            extra: ProcExtra {
                scratch_buffers: ChannelBuffer::new(stream_info.max_block_frames.get() as usize),
                declick_values: DeclickValues::new(stream_info.declick_frames),
                logger,
                store,
            },
            poisoned: false,
        }
    }
}

pub(crate) struct NodeEntry {
    pub processor: Box<dyn AudioNodeProcessor>,
    pub prev_output_was_silent: bool,

    event_data: NodeEventSchedulerData,
}

pub(crate) enum ContextToProcessorMsg {
    EventGroup(Vec<NodeEvent>),
    NewSchedule(Box<ScheduleHeapData>),
    SetFlags(FirewheelFlags),
    #[cfg(feature = "musical_transport")]
    SetTransportState(Box<TransportState>),
    #[cfg(feature = "scheduled_events")]
    ClearScheduledEvents(SmallVec<[ClearScheduledEventsEvent; 1]>),
}

pub(crate) enum ProcessorToContextMsg {
    ReturnEventGroup(Vec<NodeEvent>),
    ReturnSchedule(Box<ScheduleHeapData>),
    #[cfg(feature = "musical_transport")]
    ReturnTransportState(Box<TransportState>),
    #[cfg(feature = "scheduled_events")]
    ReturnClearScheduledEvents(SmallVec<[ClearScheduledEventsEvent; 1]>),
}

#[cfg(feature = "scheduled_events")]
pub(crate) struct ClearScheduledEventsEvent {
    /// If `None`, then clear events for all nodes.
    pub node_id: Option<NodeID>,
    pub event_type: ClearScheduledEventsType,
}

#[cfg(feature = "scheduled_events")]
#[derive(Clone)]
pub(crate) struct SharedClock {
    pub clock_samples: InstantSamples,
    #[cfg(feature = "musical_transport")]
    pub current_playhead: Option<InstantMusical>,
    #[cfg(feature = "musical_transport")]
    pub speed_multiplier: f64,
    #[cfg(feature = "musical_transport")]
    pub transport_is_playing: bool,
    pub update_instant: Instant,
}

#[cfg(feature = "scheduled_events")]
impl Default for SharedClock {
    fn default() -> Self {
        Self {
            clock_samples: InstantSamples(0),
            #[cfg(feature = "musical_transport")]
            current_playhead: None,
            #[cfg(feature = "musical_transport")]
            speed_multiplier: 1.0,
            #[cfg(feature = "musical_transport")]
            transport_is_playing: false,
            update_instant: Instant::now(),
        }
    }
}

/// How to handle event buffers on the audio thread running out of space.
#[derive(Default, Debug, Clone, Copy, PartialEq, PartialOrd)]
#[cfg_attr(feature = "bevy_reflect", derive(bevy_reflect::Reflect))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
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

#[derive(Default)]
pub(crate) struct StatusFlags {
    pub clipping_occured: AtomicBool,
    pub non_finite_number_occured: AtomicBool,
}
