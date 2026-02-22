use bevy_platform::time::Instant;
use core::cell::RefCell;
use core::num::NonZeroU32;
use core::time::Duration;
use core::{any::Any, f64};
use firewheel_core::clock::DurationSeconds;
use firewheel_core::log::{RealtimeLogger, RealtimeLoggerConfig, RealtimeLoggerMainThread};
use firewheel_core::node::ProcStore;
use firewheel_core::{
    channel_config::{ChannelConfig, ChannelCount},
    clock::AudioClock,
    dsp::declick::DeclickValues,
    event::{NodeEvent, NodeEventType},
    node::{AudioNode, DynAudioNode, NodeID},
    StreamInfo,
};
use ringbuf::traits::{Consumer, Producer, Split};
use smallvec::SmallVec;

#[cfg(not(feature = "std"))]
use num_traits::Float;

#[cfg(all(not(feature = "std"), feature = "musical_transport"))]
use bevy_platform::prelude::Box;
#[cfg(not(feature = "std"))]
use bevy_platform::prelude::Vec;

use crate::error::RemoveNodeError;
use crate::processor::BufferOutOfSpaceMode;
use crate::{
    backend::AudioBackend,
    error::{AddEdgeError, StartStreamError, UpdateError},
    graph::{AudioGraph, Edge, EdgeID, NodeEntry, PortIdx},
    processor::{
        ContextToProcessorMsg, FirewheelProcessor, FirewheelProcessorInner, ProcessorToContextMsg,
        SharedClock,
    },
};

#[cfg(feature = "scheduled_events")]
use crate::processor::ClearScheduledEventsEvent;
#[cfg(feature = "scheduled_events")]
use firewheel_core::clock::EventInstant;

#[cfg(feature = "musical_transport")]
use firewheel_core::clock::TransportState;

/// The configuration of a Firewheel context.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "bevy_reflect", derive(bevy_reflect::Reflect))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
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
    /// The maximum number of events that can be sent in a single call
    /// to [`AudioNodeProcessor::process`].
    ///
    /// By default this is set to `128`.
    ///
    /// [`AudioNodeProcessor::process`]: firewheel_core::node::AudioNodeProcessor::process
    pub event_queue_capacity: usize,
    /// The maximum number of immediate events (events that do *NOT* have a
    /// scheduled time component) that can be stored at once in the audio
    /// thread.
    ///
    /// By default this is set to `512`.
    pub immediate_event_capacity: usize,
    /// The maximum number of scheduled events (events that have a scheduled
    /// time component) that can be stored at once in the audio thread.
    ///
    /// This can be set to `0` to save some memory if you do not plan on using
    /// scheduled events.
    ///
    /// By default this is set to `512`.
    #[cfg(feature = "scheduled_events")]
    pub scheduled_event_capacity: usize,
    /// How to handle event buffers on the audio thread running out of space.
    ///
    /// By default this is set to [`BufferOutOfSpaceMode::AllocateOnAudioThread`].
    pub buffer_out_of_space_mode: BufferOutOfSpaceMode,

    /// The configuration of the realtime safe logger.
    pub logger_config: RealtimeLoggerConfig,

    /// Force all buffers in the audio graph to be cleared when processing.
    ///
    /// This is only meant to be used when diagnosing a misbehaving audio node.
    /// Enabling this significantly increases processing overhead.
    ///
    /// If enabling this fixes audio glitching issues, then notify the node
    /// author of the misbehavior.
    ///
    /// By default this is set to `false`.
    pub debug_force_clear_buffers: bool,

    /// The initial number of slots to allocate for the [`ProcStore`].
    ///
    /// By default this is set to `8`.
    pub proc_store_capacity: usize,
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
            immediate_event_capacity: 512,
            #[cfg(feature = "scheduled_events")]
            scheduled_event_capacity: 512,
            buffer_out_of_space_mode: BufferOutOfSpaceMode::AllocateOnAudioThread,
            logger_config: RealtimeLoggerConfig::default(),
            debug_force_clear_buffers: false,
            proc_store_capacity: 8,
        }
    }
}

struct ActiveState<B: AudioBackend> {
    backend_handle: B,
    stream_info: StreamInfo,
}

pub(crate) struct ProcessorChannel<B: AudioBackend> {
    pub(crate) from_context_rx: ringbuf::HeapCons<ContextToProcessorMsg>,
    pub(crate) to_context_tx: ringbuf::HeapProd<ProcessorToContextMsg>,
    pub(crate) shared_clock_input: triple_buffer::Input<SharedClock<B::Instant>>,
    pub(crate) logger: RealtimeLogger,
    pub(crate) store: ProcStore,
}

/// A Firewheel context
pub struct FirewheelCtx<B: AudioBackend> {
    graph: AudioGraph,

    to_processor_tx: ringbuf::HeapProd<ContextToProcessorMsg>,
    from_processor_rx: ringbuf::HeapCons<ProcessorToContextMsg>,
    logger_rx: RealtimeLoggerMainThread,

    active_state: Option<ActiveState<B>>,

    processor_channel: Option<ProcessorChannel<B>>,
    processor_drop_rx: Option<ringbuf::HeapCons<FirewheelProcessorInner<B>>>,

    shared_clock_output: RefCell<triple_buffer::Output<SharedClock<B::Instant>>>,
    sample_rate: NonZeroU32,
    sample_rate_recip: f64,

    #[cfg(feature = "musical_transport")]
    transport_state: Box<TransportState>,
    #[cfg(feature = "musical_transport")]
    transport_state_alloc_reuse: Option<Box<TransportState>>,

    // Re-use the allocations for groups of events.
    event_group_pool: Vec<Vec<NodeEvent>>,
    event_group: Vec<NodeEvent>,
    initial_event_group_capacity: usize,

    #[cfg(feature = "scheduled_events")]
    queued_clear_scheduled_events: Vec<ClearScheduledEventsEvent>,

    config: FirewheelConfig,
}

impl<B: AudioBackend> FirewheelCtx<B> {
    /// Create a new Firewheel context.
    pub fn new(config: FirewheelConfig) -> Self {
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

        let (shared_clock_input, shared_clock_output) =
            triple_buffer::triple_buffer(&SharedClock::default());

        let (logger, logger_rx) = firewheel_core::log::realtime_logger(config.logger_config);

        let store = ProcStore::with_capacity(config.proc_store_capacity);

        Self {
            graph: AudioGraph::new(&config),
            to_processor_tx,
            from_processor_rx,
            logger_rx,
            active_state: None,
            processor_channel: Some(ProcessorChannel {
                from_context_rx,
                to_context_tx,
                shared_clock_input,
                logger,
                store,
            }),
            processor_drop_rx: None,
            shared_clock_output: RefCell::new(shared_clock_output),
            sample_rate: NonZeroU32::new(44100).unwrap(),
            sample_rate_recip: 44100.0f64.recip(),
            #[cfg(feature = "musical_transport")]
            transport_state: Box::new(TransportState::default()),
            #[cfg(feature = "musical_transport")]
            transport_state_alloc_reuse: None,
            event_group_pool,
            event_group: Vec::with_capacity(initial_event_group_capacity),
            initial_event_group_capacity,
            #[cfg(feature = "scheduled_events")]
            queued_clear_scheduled_events: Vec::new(),
            config,
        }
    }

    /// Get an immutable reference to the processor store.
    ///
    /// If an audio stream is currently running, this will return `None`.
    pub fn proc_store(&self) -> Option<&ProcStore> {
        if let Some(proc_channel) = &self.processor_channel {
            Some(&proc_channel.store)
        } else if let Some(processor) = self.processor_drop_rx.as_ref().unwrap().last() {
            if processor.poisoned {
                panic!("The audio thread has panicked!");
            }

            Some(&processor.extra.store)
        } else {
            None
        }
    }

    /// Get a mutable reference to the processor store.
    ///
    /// If an audio stream is currently running, this will return `None`.
    pub fn proc_store_mut(&mut self) -> Option<&mut ProcStore> {
        if let Some(proc_channel) = &mut self.processor_channel {
            Some(&mut proc_channel.store)
        } else if let Some(processor) = self.processor_drop_rx.as_mut().unwrap().last_mut() {
            if processor.poisoned {
                panic!("The audio thread has panicked!");
            }

            Some(&mut processor.extra.store)
        } else {
            None
        }
    }

    /// Get a reference to the currently active instance of the backend. Returns `None` if the backend has not
    /// yet been initialized with `start_stream`.
    pub fn active_backend(&self) -> Option<&B> {
        self.active_state
            .as_ref()
            .map(|state| &state.backend_handle)
    }

    /// Get a mutable reference to the currently active instance of the backend. Returns `None` if the backend has not
    /// yet been initialized with `start_stream`.
    pub fn active_backend_mut(&mut self) -> Option<&mut B> {
        self.active_state
            .as_mut()
            .map(|state| &mut state.backend_handle)
    }

    /// Get a struct used to retrieve the list of available audio devices
    /// on the system and their available ocnfigurations.
    pub fn device_enumerator(&self) -> B::Enumerator {
        B::enumerator()
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
            B::start_stream(config).map_err(StartStreamError::BackendError)?;

        stream_info.sample_rate_recip = (stream_info.sample_rate.get() as f64).recip();
        stream_info.declick_frames = NonZeroU32::new(
            (self.config.declick_seconds * stream_info.sample_rate.get() as f32).round() as u32,
        )
        .unwrap_or(NonZeroU32::MIN);

        let maybe_processor = self.processor_channel.take();

        stream_info.prev_sample_rate = if maybe_processor.is_some() {
            stream_info.sample_rate
        } else {
            self.sample_rate
        };

        self.sample_rate = stream_info.sample_rate;
        self.sample_rate_recip = stream_info.sample_rate_recip;

        let schedule = self.graph.compile(&stream_info)?;

        let (drop_tx, drop_rx) = ringbuf::HeapRb::<FirewheelProcessorInner<B>>::new(1).split();

        let processor = if let Some(proc_channel) = maybe_processor {
            FirewheelProcessorInner::new(
                proc_channel,
                self.config.immediate_event_capacity,
                #[cfg(feature = "scheduled_events")]
                self.config.scheduled_event_capacity,
                self.config.event_queue_capacity,
                &stream_info,
                self.config.hard_clip_outputs,
                self.config.buffer_out_of_space_mode,
                self.config.debug_force_clear_buffers,
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

        if self
            .send_message_to_processor(ContextToProcessorMsg::NewSchedule(schedule))
            .is_err()
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

    /// Get the current time of the audio clock, without accounting for the delay
    /// between when the clock was last updated and now.
    ///
    /// For most use cases you probably want to use [`FirewheelCtx::audio_clock_corrected`]
    /// instead, but this method is provided if needed.
    ///
    /// Note, due to the nature of audio processing, this clock is is *NOT* synced with
    /// the system's time (`Instant::now`). (Instead it is based on the amount of data
    /// that has been processed.) For applications where the timing of audio events is
    /// critical (i.e. a rythm game), sync the game to this audio clock instead of the
    /// OS's clock (`Instant::now()`).
    ///
    /// Note, calling this method is not super cheap, so avoid calling it many
    /// times within the same game loop iteration if possible.
    pub fn audio_clock(&self) -> AudioClock {
        // Reading the latest value of the clock doesn't meaningfully mutate
        // state, so treat it as an immutable operation with interior mutability.
        //
        // PANIC SAFETY: This struct is the only place this is ever borrowed, so this
        // will never panic.
        let mut clock_borrowed = self.shared_clock_output.borrow_mut();
        let clock = clock_borrowed.read();

        let update_instant = audio_clock_update_instant_and_delay(clock, &self.active_state)
            .map(|(update_instant, _delay)| update_instant);

        AudioClock {
            samples: clock.clock_samples,
            seconds: clock
                .clock_samples
                .to_seconds(self.sample_rate, self.sample_rate_recip),
            #[cfg(feature = "musical_transport")]
            musical: clock.current_playhead,
            #[cfg(feature = "musical_transport")]
            transport_is_playing: clock.transport_is_playing,
            update_instant,
        }
    }

    /// Get the current time of the audio clock.
    ///
    /// Unlike, [`FirewheelCtx::audio_clock`], this method accounts for the delay
    /// between when the audio clock was last updated and now, leading to a more
    /// accurate result for games and other applications.
    ///
    /// If the delay could not be determined (i.e. an audio stream is not currently
    /// running), then this will assume there was no delay between when the audio
    /// clock was last updated and now.
    ///
    /// Note, due to the nature of audio processing, this clock is is *NOT* synced with
    /// the system's time (`Instant::now`). (Instead it is based on the amount of data
    /// that has been processed.) For applications where the timing of audio events is
    /// critical (i.e. a rythm game), sync the game to this audio clock instead of the
    /// OS's clock (`Instant::now()`).
    ///
    /// Note, calling this method is not super cheap, so avoid calling it many
    /// times within the same game loop iteration if possible.
    pub fn audio_clock_corrected(&self) -> AudioClock {
        // Reading the latest value of the clock doesn't meaningfully mutate
        // state, so treat it as an immutable operation with interior mutability.
        //
        // PANIC SAFETY: This struct is the only place this is ever borrowed, so this
        // will never panic.
        let mut clock_borrowed = self.shared_clock_output.borrow_mut();
        let clock = clock_borrowed.read();

        let Some((update_instant, delay)) =
            audio_clock_update_instant_and_delay(clock, &self.active_state)
        else {
            // The audio thread is not currently running, so just return the
            // latest value of the clock.
            return AudioClock {
                samples: clock.clock_samples,
                seconds: clock
                    .clock_samples
                    .to_seconds(self.sample_rate, self.sample_rate_recip),
                #[cfg(feature = "musical_transport")]
                musical: clock.current_playhead,
                #[cfg(feature = "musical_transport")]
                transport_is_playing: clock.transport_is_playing,
                update_instant: None,
            };
        };

        // Account for the delay between when the clock was last updated and now.
        let delta_seconds = DurationSeconds(delay.as_secs_f64());

        let samples = clock.clock_samples + delta_seconds.to_samples(self.sample_rate);

        #[cfg(feature = "musical_transport")]
        let musical = clock.current_playhead.map(|musical_time| {
            if clock.transport_is_playing && self.transport_state.transport.is_some() {
                self.transport_state
                    .transport
                    .as_ref()
                    .unwrap()
                    .delta_seconds_from(musical_time, delta_seconds, clock.speed_multiplier)
            } else {
                musical_time
            }
        });

        AudioClock {
            samples,
            seconds: samples.to_seconds(self.sample_rate, self.sample_rate_recip),
            #[cfg(feature = "musical_transport")]
            musical,
            #[cfg(feature = "musical_transport")]
            transport_is_playing: clock.transport_is_playing,
            update_instant: Some(update_instant),
        }
    }

    /// Get the instant the audio clock was last updated.
    ///
    /// This method accounts for the delay between when the audio clock was last
    /// updated and now, leading to a more accurate result for games and other
    /// applications.
    ///
    /// If the audio thread is not currently running, or if the delay could not
    /// be determined for any other reason, then this will return `None`.
    ///
    /// Note, calling this method is not super cheap, so avoid calling it many
    /// times within the same game loop iteration if possible.
    pub fn audio_clock_instant(&self) -> Option<Instant> {
        // Reading the latest value of the clock doesn't meaningfully mutate
        // state, so treat it as an immutable operation with interior mutability.
        //
        // PANIC SAFETY: This struct is the only place this is ever borrowed, so this
        // will never panic.
        let mut clock_borrowed = self.shared_clock_output.borrow_mut();
        let clock = clock_borrowed.read();

        audio_clock_update_instant_and_delay(clock, &self.active_state)
            .map(|(update_instant, _delay)| update_instant)
    }

    /// Sync the state of the musical transport.
    ///
    /// If the message channel is full, then this will return an error.
    #[cfg(feature = "musical_transport")]
    pub fn sync_transport(
        &mut self,
        transport: &TransportState,
    ) -> Result<(), UpdateError<B::StreamError>> {
        if &*self.transport_state != transport {
            let transport_msg = if let Some(mut t) = self.transport_state_alloc_reuse.take() {
                *t = transport.clone();
                t
            } else {
                Box::new(transport.clone())
            };

            self.send_message_to_processor(ContextToProcessorMsg::SetTransportState(transport_msg))
                .map_err(|(_, e)| e)?;

            *self.transport_state = transport.clone();
        }

        Ok(())
    }

    /// Get the current transport state.
    #[cfg(feature = "musical_transport")]
    pub fn transport_state(&self) -> &TransportState {
        &self.transport_state
    }

    /// Get the current transport state.
    #[cfg(feature = "musical_transport")]
    pub fn transport(&self) -> &TransportState {
        &self.transport_state
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
        self.logger_rx.flush(
            |msg| {
                #[cfg(feature = "tracing")]
                tracing::error!("{}", msg);

                #[cfg(all(feature = "log", not(feature = "tracing")))]
                log::error!("{}", msg);

                let _ = msg;
            },
            #[cfg(debug_assertions)]
            |msg| {
                #[cfg(feature = "tracing")]
                tracing::debug!("{}", msg);

                #[cfg(all(feature = "log", not(feature = "tracing")))]
                log::debug!("{}", msg);

                let _ = msg;
            },
        );

        firewheel_core::collector::GlobalRtGc::collect();

        for msg in self.from_processor_rx.pop_iter() {
            match msg {
                ProcessorToContextMsg::ReturnEventGroup(mut event_group) => {
                    event_group.clear();
                    self.event_group_pool.push(event_group);
                }
                ProcessorToContextMsg::ReturnSchedule(schedule_data) => {
                    let _ = schedule_data;
                }
                #[cfg(feature = "musical_transport")]
                ProcessorToContextMsg::ReturnTransportState(transport_state) => {
                    if self.transport_state_alloc_reuse.is_none() {
                        self.transport_state_alloc_reuse = Some(transport_state);
                    }
                }
                #[cfg(feature = "scheduled_events")]
                ProcessorToContextMsg::ReturnClearScheduledEvents(msgs) => {
                    let _ = msgs;
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

            #[cfg(feature = "scheduled_events")]
            if !self.queued_clear_scheduled_events.is_empty() {
                let msgs: SmallVec<[ClearScheduledEventsEvent; 1]> =
                    self.queued_clear_scheduled_events.drain(..).collect();

                if let Err((msg, e)) = self
                    .send_message_to_processor(ContextToProcessorMsg::ClearScheduledEvents(msgs))
                {
                    let ContextToProcessorMsg::ClearScheduledEvents(mut msgs) = msg else {
                        unreachable!();
                    };

                    self.queued_clear_scheduled_events = msgs.drain(..).collect();

                    return Err(e);
                }
            }

            if !self.event_group.is_empty() {
                let mut next_event_group = self
                    .event_group_pool
                    .pop()
                    .unwrap_or_else(|| Vec::with_capacity(self.initial_event_group_capacity));
                core::mem::swap(&mut next_event_group, &mut self.event_group);

                if let Err((msg, e)) = self
                    .send_message_to_processor(ContextToProcessorMsg::EventGroup(next_event_group))
                {
                    let ContextToProcessorMsg::EventGroup(mut event_group) = msg else {
                        unreachable!();
                    };

                    core::mem::swap(&mut event_group, &mut self.event_group);
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
    /// This will return an error if the ID is of the graph input or graph
    /// output node.
    pub fn remove_node(
        &mut self,
        node_id: NodeID,
    ) -> Result<SmallVec<[EdgeID; 4]>, RemoveNodeError> {
        self.graph.remove_node(node_id)
    }

    /// Get information about a node in the graph.
    pub fn node_info(&self, id: NodeID) -> Option<&NodeEntry> {
        self.graph.node_info(id)
    }

    /// Get an immutable reference to the custom state of a node.
    pub fn node_state<T: 'static>(&self, id: NodeID) -> Option<&T> {
        self.graph.node_state(id)
    }

    /// Get a type-erased, immutable reference to the custom state of a node.
    pub fn node_state_dyn(&self, id: NodeID) -> Option<&dyn Any> {
        self.graph.node_state_dyn(id)
    }

    /// Get a mutable reference to the custom state of a node.
    pub fn node_state_mut<T: 'static>(&mut self, id: NodeID) -> Option<&mut T> {
        self.graph.node_state_mut(id)
    }

    pub fn node_state_dyn_mut(&mut self, id: NodeID) -> Option<&mut dyn Any> {
        self.graph.node_state_dyn_mut(id)
    }

    /// Get a list of all the existing nodes in the graph.
    pub fn nodes(&self) -> impl Iterator<Item = &NodeEntry> {
        self.graph.nodes()
    }

    /// Get a list of all the existing edges in the graph.
    pub fn edges(&self) -> impl Iterator<Item = &Edge> {
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
    ///   where the first value in a tuple is the output port on `src_node`,
    ///   and the second value in that tuple is the input port on `dst_node`.
    /// * `check_for_cycles` - If `true`, then this will run a check to
    ///   see if adding these edges will create a cycle in the graph, and
    ///   return an error if it does. Note, checking for cycles can be quite
    ///   expensive, so avoid enabling this when calling this method many times
    ///   in a row.
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
    ///   where the first value in a tuple is the output port on `src_node`,
    ///   and the second value in that tuple is the input port on `dst_node`.
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
        self.queue_event(NodeEvent {
            node_id,
            #[cfg(feature = "scheduled_events")]
            time: None,
            event,
        });
    }

    /// Queue an event at a certain time, to be sent to an audio node's processor.
    ///
    /// If `time` is `None`, then the event will occur as soon as the node's
    /// processor receives the event.
    ///
    /// Note, this event will not be sent until the event queue is flushed
    /// in [`FirewheelCtx::update`].
    #[cfg(feature = "scheduled_events")]
    pub fn schedule_event_for(
        &mut self,
        node_id: NodeID,
        event: NodeEventType,
        time: Option<EventInstant>,
    ) {
        self.queue_event(NodeEvent {
            node_id,
            time,
            event,
        });
    }

    /// Cancel scheduled events for all nodes.
    ///
    /// This will clear all events that have been scheduled since the last call to
    /// [`FirewheelCtx::update`]. Any events scheduled between then and the next call
    /// to [`FirewheelCtx::update`] will not be canceled.
    ///
    /// This only takes effect once [`FirewheelCtx::update`] is called.
    #[cfg(feature = "scheduled_events")]
    pub fn cancel_all_scheduled_events(&mut self, event_type: ClearScheduledEventsType) {
        self.queued_clear_scheduled_events
            .push(ClearScheduledEventsEvent {
                node_id: None,
                event_type,
            });
    }

    /// Cancel scheduled events for a specific node.
    ///
    /// This will clear all events that have been scheduled since the last call to
    /// [`FirewheelCtx::update`]. Any events scheduled between then and the next call
    /// to [`FirewheelCtx::update`] will not be canceled.
    ///
    /// This only takes effect once [`FirewheelCtx::update`] is called.
    #[cfg(feature = "scheduled_events")]
    pub fn cancel_scheduled_events_for(
        &mut self,
        node_id: NodeID,
        event_type: ClearScheduledEventsType,
    ) {
        self.queued_clear_scheduled_events
            .push(ClearScheduledEventsEvent {
                node_id: Some(node_id),
                event_type,
            });
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
            let now = bevy_platform::time::Instant::now();

            while drop_rx.try_peek().is_none() {
                if now.elapsed() > core::time::Duration::from_secs(2) {
                    break;
                }

                bevy_platform::thread::sleep(core::time::Duration::from_millis(2));
            }
        }

        firewheel_core::collector::GlobalRtGc::collect();
    }
}

impl<B: AudioBackend> FirewheelCtx<B> {
    /// Construct an [`ContextQueue`] for diffing.
    pub fn event_queue(&mut self, id: NodeID) -> ContextQueue<'_, B> {
        ContextQueue {
            context: self,
            id,
            #[cfg(feature = "scheduled_events")]
            time: None,
        }
    }

    #[cfg(feature = "scheduled_events")]
    pub fn event_queue_scheduled(
        &mut self,
        id: NodeID,
        time: Option<EventInstant>,
    ) -> ContextQueue<'_, B> {
        ContextQueue {
            context: self,
            id,
            time,
        }
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
    #[cfg(feature = "scheduled_events")]
    time: Option<EventInstant>,
}

#[cfg(feature = "scheduled_events")]
impl<'a, B: AudioBackend> ContextQueue<'a, B> {
    pub fn time(&self) -> Option<EventInstant> {
        self.time
    }
}

impl<B: AudioBackend> firewheel_core::diff::EventQueue for ContextQueue<'_, B> {
    fn push(&mut self, data: NodeEventType) {
        self.context.queue_event(NodeEvent {
            event: data,
            #[cfg(feature = "scheduled_events")]
            time: self.time,
            node_id: self.id,
        });
    }
}

/// The type of scheduled events to clear
#[cfg(feature = "scheduled_events")]
#[derive(Default, Debug, Clone, Copy, PartialEq)]
pub enum ClearScheduledEventsType {
    /// Clear both musical and non-musical scheduled events.
    #[default]
    All,
    /// Clear only non-musical scheduled events.
    NonMusicalOnly,
    /// Clear only musical scheduled events.
    MusicalOnly,
}

fn audio_clock_update_instant_and_delay<B: AudioBackend>(
    clock: &SharedClock<B::Instant>,
    active_state: &Option<ActiveState<B>>,
) -> Option<(Instant, Duration)> {
    active_state.as_ref().and_then(|active_state| {
        clock
            .process_timestamp
            .clone()
            .and_then(|process_timestamp| {
                active_state
                    .backend_handle
                    .delay_from_last_process(process_timestamp)
                    .and_then(|delay| {
                        Instant::now()
                            .checked_sub(delay)
                            .map(|instant| (instant, delay))
                    })
            })
    })
}
