use bevy_platform::time::Instant;
use core::{num::NonZeroU32, ops::Range};

use ringbuf::traits::{Consumer, Producer};
use thunderdome::Arena;

use crate::graph::{NodeHeapData, ScheduleHeapData};
use firewheel_core::{
    clock::{ClockSamples, ClockSeconds, MusicalTime, TransportState},
    dsp::{buffer::ChannelBuffer, declick::DeclickValues},
    event::{NodeEvent, NodeEventList},
    node::{
        AudioNodeProcessor, NodeID, ProcBuffers, ProcInfo, ProcessStatus, StreamStatus,
        TransportInfo, NUM_SCRATCH_BUFFERS,
    },
    SilenceMask, StreamInfo,
};

pub struct FirewheelProcessor {
    inner: Option<FirewheelProcessorInner>,
    drop_tx: ringbuf::HeapProd<FirewheelProcessorInner>,
}

impl Drop for FirewheelProcessor {
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

impl FirewheelProcessor {
    pub(crate) fn new(
        processor: FirewheelProcessorInner,
        drop_tx: ringbuf::HeapProd<FirewheelProcessorInner>,
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
        clock_seconds: ClockSeconds,
        stream_status: StreamStatus,
    ) {
        if let Some(inner) = &mut self.inner {
            inner.process_interleaved(
                input,
                output,
                num_in_channels,
                num_out_channels,
                frames,
                clock_seconds,
                stream_status,
            );
        }
    }
}

pub(crate) struct FirewheelProcessorInner {
    nodes: Arena<NodeEntry>,
    schedule_data: Option<Box<ScheduleHeapData>>,

    from_graph_rx: ringbuf::HeapCons<ContextToProcessorMsg>,
    to_graph_tx: ringbuf::HeapProd<ProcessorToContextMsg>,

    event_buffer: Vec<NodeEvent>,

    sample_rate: NonZeroU32,
    sample_rate_recip: f64,
    max_block_frames: usize,

    clock_samples: ClockSamples,
    shared_clock_input: triple_buffer::Input<SharedClock>,
    clock_samples_stream_restart_offset: ClockSamples,

    hard_clip_outputs: bool,

    scratch_buffers: ChannelBuffer<f32, NUM_SCRATCH_BUFFERS>,
    declick_values: DeclickValues,

    transport_state: Box<TransportState>,
    transport_start_frame: ClockSamples,
    transport_paused_at_frame: ClockSamples,
    transport_paused_at_musical_time: MusicalTime,

    /// If a panic occurs while processing, this flag is set to let the
    /// main thread know that it shouldn't try spawning a new audio stream
    /// with the shared `Arc<AtomicRefCell<FirewheelProcessorInner>>` object.
    pub(crate) poisoned: bool,

    is_new_stream: bool,
}

impl FirewheelProcessorInner {
    /// Note, this method gets called on the main thread, not the audio thread.
    pub(crate) fn new(
        from_graph_rx: ringbuf::HeapCons<ContextToProcessorMsg>,
        to_graph_tx: ringbuf::HeapProd<ProcessorToContextMsg>,
        shared_clock_input: triple_buffer::Input<SharedClock>,
        node_capacity: usize,
        stream_info: &StreamInfo,
        hard_clip_outputs: bool,
    ) -> Self {
        Self {
            nodes: Arena::with_capacity(node_capacity * 2),
            schedule_data: None,
            from_graph_rx,
            to_graph_tx,
            event_buffer: Vec::new(),
            sample_rate: stream_info.sample_rate,
            sample_rate_recip: stream_info.sample_rate_recip,
            max_block_frames: stream_info.max_block_frames.get() as usize,
            clock_samples: ClockSamples(0),
            clock_samples_stream_restart_offset: ClockSamples(0),
            shared_clock_input,
            hard_clip_outputs,
            scratch_buffers: ChannelBuffer::new(stream_info.max_block_frames.get() as usize),
            declick_values: DeclickValues::new(stream_info.declick_frames),
            transport_state: Box::new(TransportState::default()),
            transport_start_frame: ClockSamples(0),
            transport_paused_at_frame: ClockSamples(0),
            transport_paused_at_musical_time: MusicalTime(0.0),
            poisoned: false,
            is_new_stream: true,
        }
    }

    // TODO: Add a `process_deinterleaved` method.

    /// Process the given buffers of audio data.
    fn process_interleaved(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        num_in_channels: usize,
        num_out_channels: usize,
        frames: usize,
        clock_seconds: ClockSeconds,
        stream_status: StreamStatus,
    ) {
        // --- Sync the clock -----------------------------------------------------------------

        if self.is_new_stream {
            self.is_new_stream = false;

            // The clock sent to us by the OS may not start at exactly 0, so account for that.
            self.clock_samples_stream_restart_offset -=
                clock_seconds.to_samples(self.sample_rate.get());
        }

        let mut clock_samples = if stream_status.contains(StreamStatus::OUTPUT_UNDERFLOW) {
            // If an output underflow occurred, correct for the missing frames by
            // syncing the sample clock to the OS's clock.
            clock_seconds.to_samples(self.sample_rate.get())
                + self.clock_samples_stream_restart_offset
        } else {
            self.clock_samples
        };

        // The sample clock is ultimately used as the "source of truth".
        let mut clock_seconds =
            clock_samples.to_seconds(self.sample_rate.get(), self.sample_rate_recip);

        self.clock_samples = clock_samples;

        // --- Poll messages ------------------------------------------------------------------

        self.poll_messages();

        // --- Increment the clock for the next process cycle ---------------------------------

        self.clock_samples += ClockSamples(frames as i64);

        self.sync_shared_clock(true);

        // --- Process the audio graph in blocks ----------------------------------------------

        if self.schedule_data.is_none() || frames == 0 {
            output.fill(0.0);
            return;
        };

        assert_eq!(input.len(), frames * num_in_channels);
        assert_eq!(output.len(), frames * num_out_channels);

        let mut frames_processed = 0;
        while frames_processed < frames {
            let block_frames = (frames - frames_processed).min(self.max_block_frames);

            // Prepare graph input buffers.
            self.schedule_data
                .as_mut()
                .unwrap()
                .schedule
                .prepare_graph_inputs(
                    block_frames,
                    num_in_channels,
                    |channels: &mut [&mut [f32]]| -> SilenceMask {
                        firewheel_core::dsp::interleave::deinterleave(
                            channels,
                            0,
                            &input[frames_processed * num_in_channels
                                ..(frames_processed + block_frames) * num_in_channels],
                            num_in_channels,
                            true,
                        )
                    },
                );

            let next_clock_seconds =
                clock_seconds + ClockSeconds(block_frames as f64 * self.sample_rate_recip);

            self.process_block(
                block_frames,
                self.clock_samples,
                clock_seconds..next_clock_seconds,
                stream_status,
            );

            // Copy the output of the graph to the output buffer.
            self.schedule_data
                .as_mut()
                .unwrap()
                .schedule
                .read_graph_outputs(
                    block_frames,
                    num_out_channels,
                    |channels: &[&[f32]], silence_mask| {
                        firewheel_core::dsp::interleave::interleave(
                            channels,
                            0,
                            &mut output[frames_processed * num_out_channels
                                ..(frames_processed + block_frames) * num_out_channels],
                            num_out_channels,
                            Some(silence_mask),
                        );
                    },
                );

            frames_processed += block_frames;
            clock_samples += ClockSamples(block_frames as i64);
            clock_seconds = next_clock_seconds;
        }

        // --- Hard clip outputs --------------------------------------------------------------

        if self.hard_clip_outputs {
            for s in output.iter_mut() {
                *s = s.fract();
            }
        }

        // --- Return the allocated event buffer to be reused ---------------------------------

        if self.event_buffer.capacity() > 0 {
            let mut event_group = Vec::new();
            core::mem::swap(&mut self.event_buffer, &mut event_group);

            let _ = self
                .to_graph_tx
                .try_push(ProcessorToContextMsg::ReturnEventGroup(event_group));
        }
    }

    fn poll_messages(&mut self) {
        for msg in self.from_graph_rx.pop_iter() {
            match msg {
                ContextToProcessorMsg::EventGroup(mut event_group) => {
                    let num_existing_events = self.event_buffer.len();

                    if self.event_buffer.capacity() == 0 {
                        core::mem::swap(&mut self.event_buffer, &mut event_group);
                    } else {
                        self.event_buffer.append(&mut event_group);

                        let _ = self
                            .to_graph_tx
                            .try_push(ProcessorToContextMsg::ReturnEventGroup(event_group));
                    }

                    for (i, event) in self.event_buffer[num_existing_events..].iter().enumerate() {
                        if let Some(node_entry) = self.nodes.get_mut(event.node_id.0) {
                            node_entry
                                .event_indices
                                .push((i + num_existing_events) as u32);
                        }
                    }
                }
                ContextToProcessorMsg::NewSchedule(mut new_schedule_data) => {
                    assert_eq!(
                        new_schedule_data.schedule.max_block_frames(),
                        self.max_block_frames
                    );

                    if let Some(mut old_schedule_data) = self.schedule_data.take() {
                        core::mem::swap(
                            &mut old_schedule_data.removed_nodes,
                            &mut new_schedule_data.removed_nodes,
                        );

                        for node_id in new_schedule_data.nodes_to_remove.iter() {
                            if let Some(node_entry) = self.nodes.remove(node_id.0) {
                                old_schedule_data.removed_nodes.push(NodeHeapData {
                                    id: *node_id,
                                    processor: node_entry.processor,
                                    event_buffer_indices: node_entry.event_indices,
                                });
                            }
                        }

                        let _ = self
                            .to_graph_tx
                            .try_push(ProcessorToContextMsg::ReturnSchedule(old_schedule_data));
                    }

                    for n in new_schedule_data.new_node_processors.drain(..) {
                        assert!(self
                            .nodes
                            .insert_at(
                                n.id.0,
                                NodeEntry {
                                    processor: n.processor,
                                    event_indices: n.event_buffer_indices,
                                }
                            )
                            .is_none());
                    }

                    self.schedule_data = Some(new_schedule_data);
                }
                ContextToProcessorMsg::HardClipOutputs(hard_clip_outputs) => {
                    self.hard_clip_outputs = hard_clip_outputs;
                }
                ContextToProcessorMsg::SetTransportState(mut new_transport_state) => {
                    let mut did_pause = false;

                    if let Some(new_transport) = &new_transport_state.transport {
                        if self.transport_state.playhead != new_transport_state.playhead
                            || self.transport_state.transport.is_none()
                        {
                            self.transport_start_frame = self.clock_samples
                                - new_transport.musical_to_sample(
                                    *new_transport_state.playhead,
                                    self.sample_rate.get(),
                                );
                        } else {
                            let old_transport = self.transport_state.transport.as_ref().unwrap();

                            if *new_transport_state.playing {
                                if !*self.transport_state.playing {
                                    // Resume
                                    if old_transport == new_transport {
                                        self.transport_start_frame +=
                                            self.clock_samples - self.transport_paused_at_frame;
                                    } else {
                                        self.transport_start_frame = self.clock_samples
                                            - new_transport.musical_to_sample(
                                                self.transport_paused_at_musical_time,
                                                self.sample_rate.get(),
                                            );
                                    }
                                } else if old_transport != new_transport {
                                    // Continue where the previous left off
                                    let current_musical = old_transport.sample_to_musical(
                                        self.clock_samples - self.transport_start_frame,
                                        self.sample_rate.get(),
                                        self.sample_rate_recip,
                                    );
                                    self.transport_start_frame = self.clock_samples
                                        - new_transport.musical_to_sample(
                                            current_musical,
                                            self.sample_rate.get(),
                                        );
                                }
                            } else if *self.transport_state.playing {
                                // Pause
                                did_pause = true;

                                self.transport_paused_at_frame = self.clock_samples;
                                self.transport_paused_at_musical_time = old_transport
                                    .sample_to_musical(
                                        self.clock_samples - self.transport_start_frame,
                                        self.sample_rate.get(),
                                        self.sample_rate_recip,
                                    );
                            }
                        }
                    }

                    if !did_pause {
                        self.transport_paused_at_frame = self.clock_samples;
                        self.transport_paused_at_musical_time = *new_transport_state.playhead;
                    }

                    std::mem::swap(&mut new_transport_state, &mut self.transport_state);

                    let _ = self
                        .to_graph_tx
                        .try_push(ProcessorToContextMsg::ReturnTransportState(
                            new_transport_state,
                        ));
                }
            }
        }
    }

    fn process_block(
        &mut self,
        block_frames: usize,
        clock_samples: ClockSamples,
        clock_seconds: Range<ClockSeconds>,
        stream_status: StreamStatus,
    ) {
        if self.schedule_data.is_none() {
            return;
        }
        let schedule_data = self.schedule_data.as_mut().unwrap();

        let mut scratch_buffers = self.scratch_buffers.get_mut(self.max_block_frames);

        let transport_info = self.transport_state.transport.as_ref().map(|transport| {
            let (start_beat, end_beat) = if *self.transport_state.playing {
                (
                    transport.sample_to_musical(
                        clock_samples - self.transport_start_frame,
                        self.sample_rate.get(),
                        self.sample_rate_recip,
                    ),
                    transport.sample_to_musical(
                        clock_samples - self.transport_start_frame
                            + ClockSamples(block_frames as i64),
                        self.sample_rate.get(),
                        self.sample_rate_recip,
                    ),
                )
            } else {
                (
                    self.transport_paused_at_musical_time,
                    self.transport_paused_at_musical_time,
                )
            };

            TransportInfo {
                musical_clock: start_beat..end_beat,
                transport,
                playing: *self.transport_state.playing,
            }
        });

        let mut proc_info = ProcInfo {
            frames: block_frames,
            in_silence_mask: SilenceMask::default(),
            out_silence_mask: SilenceMask::default(),
            clock_samples: clock_samples..(clock_samples + ClockSamples(block_frames as i64)),
            clock_seconds: clock_seconds.clone(),
            transport_info,
            stream_status,
            declick_values: &self.declick_values,
        };

        schedule_data.schedule.process(
            block_frames,
            &mut scratch_buffers,
            |node_id: NodeID,
             in_silence_mask: SilenceMask,
             out_silence_mask: SilenceMask,
             proc_buffers: ProcBuffers|
             -> ProcessStatus {
                let Some(node_entry) = self.nodes.get_mut(node_id.0) else {
                    return ProcessStatus::Bypass;
                };

                let events = NodeEventList::new(&mut self.event_buffer, &node_entry.event_indices);

                proc_info.in_silence_mask = in_silence_mask;
                proc_info.out_silence_mask = out_silence_mask;

                let status = node_entry
                    .processor
                    .process(proc_buffers, &proc_info, events);

                node_entry.event_indices.clear();

                status
            },
        );
    }

    fn sync_shared_clock(&mut self, stream_is_running: bool) {
        let (musical_time, transport_is_playing) =
            if let Some(transport) = &self.transport_state.transport {
                if *self.transport_state.playing {
                    (
                        Some(transport.sample_to_musical(
                            self.clock_samples - self.transport_start_frame,
                            self.sample_rate.get(),
                            self.sample_rate_recip,
                        )),
                        true,
                    )
                } else {
                    (Some(self.transport_paused_at_musical_time), false)
                }
            } else {
                (None, false)
            };

        self.shared_clock_input.write(SharedClock {
            clock_samples: self.clock_samples,
            musical_time,
            transport_is_playing,
            instant_of_update: stream_is_running.then(|| Instant::now()),
        });
    }

    fn stream_stopped(&mut self) {
        self.sync_shared_clock(false);

        self.clock_samples_stream_restart_offset = self.clock_samples;

        for (_, node) in self.nodes.iter_mut() {
            node.processor.stream_stopped();
        }
    }

    /// Called when a new audio stream has been started to replace the old one.
    ///
    /// Note, this method gets called on the main thread, not the audio thread.
    pub(crate) fn new_stream(&mut self, stream_info: &StreamInfo) {
        self.is_new_stream = true;

        for (_, node) in self.nodes.iter_mut() {
            node.processor.new_stream(stream_info);
        }

        if self.sample_rate != stream_info.sample_rate {
            self.clock_samples = self
                .clock_samples
                .to_seconds(self.sample_rate.get(), self.sample_rate_recip)
                .to_samples(stream_info.sample_rate.get());
            self.clock_samples_stream_restart_offset = self
                .clock_samples_stream_restart_offset
                .to_seconds(self.sample_rate.get(), self.sample_rate_recip)
                .to_samples(stream_info.sample_rate.get());

            self.transport_start_frame = self
                .transport_start_frame
                .to_seconds(self.sample_rate.get(), self.sample_rate_recip)
                .to_samples(stream_info.sample_rate.get());
            self.transport_paused_at_frame = self
                .transport_paused_at_frame
                .to_seconds(self.sample_rate.get(), self.sample_rate_recip)
                .to_samples(stream_info.sample_rate.get());

            self.sample_rate = stream_info.sample_rate;
            self.sample_rate_recip = stream_info.sample_rate_recip;

            self.declick_values = DeclickValues::new(stream_info.declick_frames);
        }

        if self.max_block_frames != stream_info.max_block_frames.get() as usize {
            self.max_block_frames = stream_info.max_block_frames.get() as usize;

            self.scratch_buffers = ChannelBuffer::new(stream_info.max_block_frames.get() as usize);
        }
    }
}

pub(crate) struct NodeEntry {
    pub processor: Box<dyn AudioNodeProcessor>,
    pub event_indices: Vec<u32>,
}

pub(crate) enum ContextToProcessorMsg {
    EventGroup(Vec<NodeEvent>),
    NewSchedule(Box<ScheduleHeapData>),
    HardClipOutputs(bool),
    SetTransportState(Box<TransportState>),
}

pub(crate) enum ProcessorToContextMsg {
    ReturnEventGroup(Vec<NodeEvent>),
    ReturnSchedule(Box<ScheduleHeapData>),
    ReturnTransportState(Box<TransportState>),
}

#[derive(Clone)]
pub(crate) struct SharedClock {
    pub clock_samples: ClockSamples,
    pub musical_time: Option<MusicalTime>,
    pub transport_is_playing: bool,
    pub instant_of_update: Option<Instant>,
}

impl Default for SharedClock {
    fn default() -> Self {
        Self {
            clock_samples: ClockSamples(0),
            musical_time: None,
            transport_is_playing: false,
            instant_of_update: None,
        }
    }
}
