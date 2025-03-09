use atomic_float::AtomicF64;
use firewheel_core::{
    channel_config::{ChannelConfig, ChannelCount},
    clock::{ClockSamples, ClockSeconds, MusicalTime, MusicalTransport},
    collector::Collector,
    dsp::declick::DeclickValues,
    event::{NodeEvent, NodeEventType},
    node::{AudioNode, DynAudioNode, NodeID},
    StreamInfo,
};
use ringbuf::traits::{Consumer, Producer, Split};
use smallvec::SmallVec;
use std::{
    num::NonZeroU32,
    sync::{
        atomic::{AtomicI64, Ordering},
        Arc,
    },
};

use crate::{
    backend::{AudioBackend, DeviceInfo},
    error::{AddEdgeError, StartStreamError, UpdateError},
    graph::{AudioGraph, Edge, EdgeID, NodeEntry, PortIdx},
    processor::{
        ContextToProcessorMsg, FirewheelProcessor, FirewheelProcessorInner, ProcessorToContextMsg,
    },
};

/// The configuration of a Firewheel context.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FirewheelConfig {
    /// The number of input channels in the audio graph.
    pub num_graph_inputs: ChannelCount,
    /// The number of output channels in the audio graph.
    pub num_graph_outputs: ChannelCount,
    /// If `true`, then all outputs will be hard clipped at 0db to help
    /// protect the system's speakers.
    ///
    /// Note that most operating systems already hard clip the output,
    /// so this is usually not needed (TODO: Do research to see if this
    /// assumption is true.)
    ///
    /// By default this is set to `false`.
    pub hard_clip_outputs: bool,
    /// An initial capacity to allocate for the nodes in the audio graph.
    ///
    /// By default this is set to `64`.
    pub initial_node_capacity: u32,
    /// An initial capacity to allocate for the edges in the audio graph.
    ///
    /// By default this is set to `256`.
    pub initial_edge_capacity: u32,
    /// The amount of time in seconds to fade in/out when pausing/resuming
    /// to avoid clicks and pops.
    ///
    /// By default this is set to `10.0 / 1_000.0`.
    pub declick_seconds: f32,
    /// The initial capacity for a group of events.
    ///
    /// By default this is set to `128`.
    pub initial_event_group_capacity: u32,
    /// The capacity of the engine's internal message channel.
    ///
    /// By default this is set to `64`.
    pub channel_capacity: u32,
    /// The capacity of an event queue in the engine (one event queue per node).
    ///
    /// By default this is set to `128`.
    pub event_queue_capacity: u32,
}

impl Default for FirewheelConfig {
    fn default() -> Self {
        Self {
            num_graph_inputs: ChannelCount::ZERO,
            num_graph_outputs: ChannelCount::STEREO,
            hard_clip_outputs: false,
            initial_node_capacity: 128,
            initial_edge_capacity: 256,
            declick_seconds: DeclickValues::DEFAULT_FADE_SECONDS,
            initial_event_group_capacity: 128,
            channel_capacity: 64,
            event_queue_capacity: 128,
        }
    }
}

struct ActiveState<B: AudioBackend> {
    backend_handle: B,
    stream_info: StreamInfo,
}

/// A Firewheel context
pub struct FirewheelCtx<B: AudioBackend> {
    graph: AudioGraph,

    to_processor_tx: ringbuf::HeapProd<ContextToProcessorMsg>,
    from_processor_rx: ringbuf::HeapCons<ProcessorToContextMsg>,

    active_state: Option<ActiveState<B>>,

    processor_channel: Option<(
        ringbuf::HeapCons<ContextToProcessorMsg>,
        ringbuf::HeapProd<ProcessorToContextMsg>,
    )>,
    processor_drop_rx: Option<ringbuf::HeapCons<FirewheelProcessorInner>>,

    clock_shared: Arc<ClockValues>,

    // Re-use the allocations for groups of events.
    event_group_pool: Vec<Vec<NodeEvent>>,
    event_group: Vec<NodeEvent>,
    initial_event_group_capacity: usize,

    config: FirewheelConfig,
}

impl<B: AudioBackend> FirewheelCtx<B> {
    /// Create a new Firewheel context.
    pub fn new(config: FirewheelConfig) -> Self {
        let clock_shared = Arc::new(ClockValues {
            seconds: AtomicF64::new(0.0),
            samples: AtomicI64::new(0),
            musical: AtomicF64::new(0.0),
        });

        let (to_processor_tx, from_context_rx) =
            ringbuf::HeapRb::<ContextToProcessorMsg>::new(config.channel_capacity as usize).split();
        let (to_context_tx, from_processor_rx) =
            ringbuf::HeapRb::<ProcessorToContextMsg>::new(config.channel_capacity as usize * 2)
                .split();

        let initial_event_group_capacity = config.initial_event_group_capacity as usize;
        let mut event_group_pool = Vec::with_capacity(16);
        for _ in 0..3 {
            event_group_pool.push(Vec::with_capacity(initial_event_group_capacity));
        }

        Self {
            graph: AudioGraph::new(&config),
            to_processor_tx,
            from_processor_rx,
            active_state: None,
            processor_channel: Some((from_context_rx, to_context_tx)),
            processor_drop_rx: None,
            clock_shared: Arc::clone(&clock_shared),
            event_group_pool,
            event_group: Vec::with_capacity(initial_event_group_capacity),
            initial_event_group_capacity,
            config,
        }
    }

    /// Get a list of the available audio input devices.
    pub fn available_input_devices(&self) -> Vec<DeviceInfo> {
        B::available_input_devices()
    }

    /// Get a list of the available audio output devices.
    pub fn available_output_devices(&self) -> Vec<DeviceInfo> {
        B::available_output_devices()
    }

    /// Returns `true` if an audio stream can be started right now.
    ///
    /// When calling [`FirewheelCtx::stop_stream()`], it may take some time for the
    /// old stream to be fully stopped. This method is used to check if it has been
    /// dropped yet.
    ///
    /// Note, in rare cases where the audio thread crashes without cleanly dropping
    /// its contents, this may never return `true`. Consider adding a timeout to
    /// avoid deadlocking.
    pub fn can_start_stream(&self) -> bool {
        if self.is_audio_stream_running() {
            false
        } else if let Some(rx) = &self.processor_drop_rx {
            rx.try_peek().is_some()
        } else {
            true
        }
    }

    /// Start an audio stream for this context. Only one audio stream can exist on
    /// a context at a time.
    ///
    /// When calling [`FirewheelCtx::stop_stream()`], it may take some time for the
    /// old stream to be fully stopped. Use [`FirewheelCtx::can_start_stream`] to
    /// check if it has been dropped yet.
    ///
    /// Note, in rare cases where the audio thread crashes without cleanly dropping
    /// its contents, this may never succeed. Consider adding a timeout to avoid
    /// deadlocking.
    pub fn start_stream(
        &mut self,
        config: B::Config,
    ) -> Result<(), StartStreamError<B::StartStreamError>> {
        if self.is_audio_stream_running() {
            return Err(StartStreamError::AlreadyStarted);
        }

        if !self.can_start_stream() {
            return Err(StartStreamError::OldStreamNotFinishedStopping);
        }

        let (mut backend_handle, mut stream_info) =
            B::start_stream(config).map_err(|e| StartStreamError::BackendError(e))?;

        stream_info.sample_rate_recip = (stream_info.sample_rate.get() as f64).recip();
        stream_info.declick_frames = NonZeroU32::new(
            (self.config.declick_seconds * stream_info.sample_rate.get() as f32).round() as u32,
        )
        .unwrap_or(NonZeroU32::MIN);

        let schedule = self.graph.compile(&stream_info)?;

        let (drop_tx, drop_rx) = ringbuf::HeapRb::<FirewheelProcessorInner>::new(1).split();

        let processor =
            if let Some((from_context_rx, to_context_tx)) = self.processor_channel.take() {
                FirewheelProcessorInner::new(
                    from_context_rx,
                    to_context_tx,
                    Arc::clone(&self.clock_shared),
                    self.graph.node_capacity(),
                    &stream_info,
                    self.config.hard_clip_outputs,
                )
            } else {
                let mut processor = self.processor_drop_rx.as_mut().unwrap().try_pop().unwrap();

                if processor.poisoned {
                    panic!("The audio thread has panicked!");
                }

                processor.new_stream(&stream_info);

                processor
            };

        backend_handle.set_processor(FirewheelProcessor::new(processor, drop_tx));

        if let Err(_) = self.send_message_to_processor(ContextToProcessorMsg::NewSchedule(schedule))
        {
            panic!("Firewheel message channel is full!");
        }

        self.active_state = Some(ActiveState {
            backend_handle,
            stream_info,
        });
        self.processor_drop_rx = Some(drop_rx);

        Ok(())
    }

    /// Stop the audio stream in this context.
    pub fn stop_stream(&mut self) {
        // When the backend handle is dropped, the backend will automatically
        // stop its stream.
        self.active_state = None;
        self.graph.deactivate();
    }

    /// Returns `true` if there is currently a running audio stream.
    pub fn is_audio_stream_running(&self) -> bool {
        self.active_state.is_some()
    }

    /// Information about the running audio stream.
    ///
    /// Returns `None` if no audio stream is currently running.
    pub fn stream_info(&self) -> Option<&StreamInfo> {
        self.active_state.as_ref().map(|s| &s.stream_info)
    }

    /// The current time of the clock in the number of seconds since the stream
    /// was started.
    ///
    /// Note, this clock is not perfectly accurate, but it is good enough for
    /// most use cases. This clock also correctly accounts for any output
    /// underflows that may occur.
    pub fn clock_now(&self) -> ClockSeconds {
        ClockSeconds(self.clock_shared.seconds.load(Ordering::Relaxed))
    }

    /// The current time of the sample clock in the number of samples (of a single
    /// channel of audio) that have been processed since the beginning of the
    /// stream.
    ///
    /// This is more accurate than the seconds clock, and is ideal for syncing
    /// events to a musical transport. Though note that this clock does not
    /// account for any output underflows that may occur.
    pub fn clock_samples(&self) -> ClockSamples {
        ClockSamples(self.clock_shared.samples.load(Ordering::Relaxed))
    }

    /// The current musical time of the transport.
    ///
    /// If no transport is currently active, then this will have a value of `0`.
    pub fn clock_musical(&self) -> MusicalTime {
        MusicalTime(self.clock_shared.musical.load(Ordering::Relaxed))
    }

    /// Set the musical transport to use.
    ///
    /// If an existing musical transport is already running, then the new
    /// transport will pick up where the old one left off. This allows you
    /// to, for example, change the tempo dynamically at runtime.
    ///
    /// If the message channel is full, then this will return an error.
    pub fn set_transport(
        &mut self,
        transport: Option<MusicalTransport>,
    ) -> Result<(), UpdateError<B::StreamError>> {
        self.send_message_to_processor(ContextToProcessorMsg::SetTransport(transport))
            .map_err(|(_, e)| e)
    }

    /// Start or restart the musical transport.
    ///
    /// If the message channel is full, then this will return an error.
    pub fn start_or_restart_transport(&mut self) -> Result<(), UpdateError<B::StreamError>> {
        self.send_message_to_processor(ContextToProcessorMsg::StartOrRestartTransport)
            .map_err(|(_, e)| e)
    }

    /// Pause the musical transport.
    ///
    /// If the message channel is full, then this will return an error.
    pub fn pause_transport(&mut self) -> Result<(), UpdateError<B::StreamError>> {
        self.send_message_to_processor(ContextToProcessorMsg::PauseTransport)
            .map_err(|(_, e)| e)
    }

    /// Resume the musical transport.
    ///
    /// If the message channel is full, then this will return an error.
    pub fn resume_transport(&mut self) -> Result<(), UpdateError<B::StreamError>> {
        self.send_message_to_processor(ContextToProcessorMsg::ResumeTransport)
            .map_err(|(_, e)| e)
    }

    /// Stop the musical transport.
    ///
    /// If the message channel is full, then this will return an error.
    pub fn stop_transport(&mut self) -> Result<(), UpdateError<B::StreamError>> {
        self.send_message_to_processor(ContextToProcessorMsg::StopTransport)
            .map_err(|(_, e)| e)
    }

    /// Whether or not outputs are being hard clipped at 0dB.
    pub fn hard_clip_outputs(&self) -> bool {
        self.config.hard_clip_outputs
    }

    /// Set whether or not outputs should be hard clipped at 0dB to
    /// help protect the system's speakers.
    ///
    /// Note that most operating systems already hard clip the output,
    /// so this is usually not needed (TODO: Do research to see if this
    /// assumption is true.)
    ///
    /// If the message channel is full, then this will return an error.
    pub fn set_hard_clip_outputs(
        &mut self,
        hard_clip_outputs: bool,
    ) -> Result<(), UpdateError<B::StreamError>> {
        if self.config.hard_clip_outputs == hard_clip_outputs {
            return Ok(());
        }
        self.config.hard_clip_outputs = hard_clip_outputs;

        self.send_message_to_processor(ContextToProcessorMsg::HardClipOutputs(hard_clip_outputs))
            .map_err(|(_, e)| e)
    }

    /// Update the firewheel context.
    ///
    /// This must be called reguarly (i.e. once every frame).
    pub fn update(&mut self) -> Result<(), UpdateError<B::StreamError>> {
        firewheel_core::collector::GlobalCollector.collect();

        for msg in self.from_processor_rx.pop_iter() {
            match msg {
                ProcessorToContextMsg::ReturnEventGroup(mut event_group) => {
                    event_group.clear();
                    self.event_group_pool.push(event_group);
                }
                ProcessorToContextMsg::ReturnSchedule(schedule_data) => {
                    let _ = schedule_data;
                }
            }
        }

        self.graph.update(
            self.active_state.as_ref().map(|s| &s.stream_info),
            &mut self.event_group,
        );

        if let Some(active_state) = &mut self.active_state {
            if let Err(e) = active_state.backend_handle.poll_status() {
                self.active_state = None;
                self.graph.deactivate();

                return Err(UpdateError::StreamStoppedUnexpectedly(Some(e)));
            }

            if self
                .processor_drop_rx
                .as_ref()
                .unwrap()
                .try_peek()
                .is_some()
            {
                self.active_state = None;
                self.graph.deactivate();

                return Err(UpdateError::StreamStoppedUnexpectedly(None));
            }
        }

        if self.is_audio_stream_running() {
            if self.graph.needs_compile() {
                let schedule_data = self
                    .graph
                    .compile(&self.active_state.as_ref().unwrap().stream_info)?;

                if let Err((msg, e)) = self
                    .send_message_to_processor(ContextToProcessorMsg::NewSchedule(schedule_data))
                {
                    let ContextToProcessorMsg::NewSchedule(schedule) = msg else {
                        unreachable!();
                    };

                    self.graph.on_schedule_send_failed(schedule);

                    return Err(e);
                }
            }

            if !self.event_group.is_empty() {
                let mut next_event_group = self
                    .event_group_pool
                    .pop()
                    .unwrap_or_else(|| Vec::with_capacity(self.initial_event_group_capacity));
                std::mem::swap(&mut next_event_group, &mut self.event_group);

                if let Err((msg, e)) = self
                    .send_message_to_processor(ContextToProcessorMsg::EventGroup(next_event_group))
                {
                    let ContextToProcessorMsg::EventGroup(mut event_group) = msg else {
                        unreachable!();
                    };

                    std::mem::swap(&mut event_group, &mut self.event_group);
                    self.event_group_pool.push(event_group);

                    return Err(e);
                }
            }
        }

        Ok(())
    }

    /// The ID of the graph input node
    pub fn graph_in_node_id(&self) -> NodeID {
        self.graph.graph_in_node()
    }

    /// The ID of the graph output node
    pub fn graph_out_node_id(&self) -> NodeID {
        self.graph.graph_out_node()
    }

    /// Add a node to the audio graph.
    pub fn add_node<T: AudioNode + 'static>(
        &mut self,
        node: T,
        config: Option<T::Configuration>,
    ) -> NodeID {
        self.graph.add_node(node, config)
    }

    /// Add a node to the audio graph which implements the type-erased [`DynAudioNode`] trait.
    pub fn add_dyn_node<T: DynAudioNode + 'static>(&mut self, node: T) -> NodeID {
        self.graph.add_dyn_node(node)
    }

    /// Remove the given node from the audio graph.
    ///
    /// This will automatically remove all edges from the graph that
    /// were connected to this node.
    ///
    /// On success, this returns a list of all edges that were removed
    /// from the graph as a result of removing this node.
    ///
    /// This will return an error if a node with the given ID does not
    /// exist in the graph, or if the ID is of the graph input or graph
    /// output node.
    pub fn remove_node(&mut self, node_id: NodeID) -> Result<SmallVec<[EdgeID; 4]>, ()> {
        self.graph.remove_node(node_id)
    }

    /// Get information about a node in the graph.
    pub fn node_info(&self, id: NodeID) -> Option<&NodeEntry> {
        self.graph.node_info(id)
    }

    /// Get a list of all the existing nodes in the graph.
    pub fn nodes<'a>(&'a self) -> impl Iterator<Item = &'a NodeEntry> {
        self.graph.nodes()
    }

    /// Get a list of all the existing edges in the graph.
    pub fn edges<'a>(&'a self) -> impl Iterator<Item = &'a Edge> {
        self.graph.edges()
    }

    /// Set the number of input and output channels to and from the audio graph.
    ///
    /// Returns the list of edges that were removed.
    pub fn set_graph_channel_config(
        &mut self,
        channel_config: ChannelConfig,
    ) -> SmallVec<[EdgeID; 4]> {
        self.graph.set_graph_channel_config(channel_config)
    }

    /// Add connections (edges) between two nodes to the graph.
    ///
    /// * `src_node` - The ID of the source node.
    /// * `dst_node` - The ID of the destination node.
    /// * `ports_src_dst` - The port indices for each connection to make,
    /// where the first value in a tuple is the output port on `src_node`,
    /// and the second value in that tuple is the input port on `dst_node`.
    /// * `check_for_cycles` - If `true`, then this will run a check to
    /// see if adding these edges will create a cycle in the graph, and
    /// return an error if it does. Note, checking for cycles can be quite
    /// expensive, so avoid enabling this when calling this method many times
    /// in a row.
    ///
    /// If successful, then this returns a list of edge IDs in order.
    ///
    /// If this returns an error, then the audio graph has not been
    /// modified.
    pub fn connect(
        &mut self,
        src_node: NodeID,
        dst_node: NodeID,
        ports_src_dst: &[(PortIdx, PortIdx)],
        check_for_cycles: bool,
    ) -> Result<SmallVec<[EdgeID; 4]>, AddEdgeError> {
        self.graph
            .connect(src_node, dst_node, ports_src_dst, check_for_cycles)
    }

    /// Remove connections (edges) between two nodes from the graph.
    ///
    /// * `src_node` - The ID of the source node.
    /// * `dst_node` - The ID of the destination node.
    /// * `ports_src_dst` - The port indices for each connection to make,
    /// where the first value in a tuple is the output port on `src_node`,
    /// and the second value in that tuple is the input port on `dst_node`.
    ///
    /// If none of the edges existed in the graph, then `false` will be
    /// returned.
    pub fn disconnect(
        &mut self,
        src_node: NodeID,
        dst_node: NodeID,
        ports_src_dst: &[(PortIdx, PortIdx)],
    ) -> bool {
        self.graph.disconnect(src_node, dst_node, ports_src_dst)
    }

    /// Remove all connections (edges) between two nodes in the graph.
    ///
    /// * `src_node` - The ID of the source node.
    /// * `dst_node` - The ID of the destination node.
    pub fn disconnect_all_between(
        &mut self,
        src_node: NodeID,
        dst_node: NodeID,
    ) -> SmallVec<[EdgeID; 4]> {
        self.graph.disconnect_all_between(src_node, dst_node)
    }

    /// Remove a connection (edge) via the edge's unique ID.
    ///
    /// If the edge did not exist in this graph, then `false` will be returned.
    pub fn disconnect_by_edge_id(&mut self, edge_id: EdgeID) -> bool {
        self.graph.disconnect_by_edge_id(edge_id)
    }

    /// Get information about the given [Edge]
    pub fn edge(&self, edge_id: EdgeID) -> Option<&Edge> {
        self.graph.edge(edge_id)
    }

    /// Runs a check to see if a cycle exists in the audio graph.
    ///
    /// Note, this method is expensive.
    pub fn cycle_detected(&mut self) -> bool {
        self.graph.cycle_detected()
    }

    /// Queue an event to be sent to an audio node's processor.
    ///
    /// Note, this event will not be sent until the event queue is flushed
    /// in [`FirewheelCtx::update`].
    pub fn queue_event(&mut self, event: NodeEvent) {
        self.event_group.push(event);
    }

    /// Queue an event to be sent to an audio node's processor.
    ///
    /// Note, this event will not be sent until the event queue is flushed
    /// in [`FirewheelCtx::update`].
    pub fn queue_event_for(&mut self, node_id: NodeID, event: NodeEventType) {
        self.queue_event(NodeEvent { node_id, event });
    }

    fn send_message_to_processor(
        &mut self,
        msg: ContextToProcessorMsg,
    ) -> Result<(), (ContextToProcessorMsg, UpdateError<B::StreamError>)> {
        self.to_processor_tx
            .try_push(msg)
            .map_err(|msg| (msg, UpdateError::MsgChannelFull))
    }
}

impl<B: AudioBackend> Drop for FirewheelCtx<B> {
    fn drop(&mut self) {
        self.stop_stream();

        // Wait for the processor to be drop to avoid deallocating it on
        // the audio thread.
        #[cfg(not(target_family = "wasm"))]
        if let Some(drop_rx) = self.processor_drop_rx.take() {
            let now = std::time::Instant::now();

            while drop_rx.try_peek().is_none() {
                if now.elapsed() > std::time::Duration::from_secs(2) {
                    break;
                }

                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        }

        firewheel_core::collector::GlobalCollector.collect();
    }
}

pub(crate) struct ClockValues {
    pub seconds: AtomicF64,
    pub samples: AtomicI64,
    pub musical: AtomicF64,
}

impl<B: AudioBackend> FirewheelCtx<B> {
    /// Construct an [`ContextQueue`] for diffing.
    pub fn event_queue(&mut self, id: NodeID) -> ContextQueue<'_, B> {
        ContextQueue { context: self, id }
    }
}

/// An event queue acquired from [`FirewheelCtx::event_queue`].
///
/// This can help reduce event queue allocations
/// when you have direct access to the context.
///
/// ```
/// # use firewheel_core::{diff::{Diff, PathBuilder}, node::NodeID};
/// # use firewheel_graph::{backend::AudioBackend, FirewheelCtx, ContextQueue};
/// # fn context_queue<B: AudioBackend, D: Diff>(
/// #     context: &mut FirewheelCtx<B>,
/// #     node_id: NodeID,
/// #     params: &D,
/// #     baseline: &D,
/// # ) {
/// // Get a queue that will send events directly to the provided node.
/// let mut queue = context.event_queue(node_id);
/// // Perform diffing using this queue.
/// params.diff(baseline, PathBuilder::default(), &mut queue);
/// # }
/// ```
pub struct ContextQueue<'a, B: AudioBackend> {
    context: &'a mut FirewheelCtx<B>,
    id: NodeID,
}

impl<B: AudioBackend> firewheel_core::diff::EventQueue for ContextQueue<'_, B> {
    fn push(&mut self, data: NodeEventType) {
        self.context.queue_event(NodeEvent {
            event: data,
            node_id: self.id,
        });
    }
}
