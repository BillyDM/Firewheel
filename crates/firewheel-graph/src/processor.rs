use core::{num::NonZeroU32, ops::Range, time::Duration};

use ringbuf::traits::{Consumer, Producer};
use thunderdome::Arena;

use crate::{
    backend::AudioBackend,
    graph::{NodeHeapData, ScheduleHeapData},
};
use firewheel_core::{
    clock::{
        DurationSamples, DurationSeconds, InstantMusical, InstantSamples, InstantSeconds,
        ProcTransportInfo, TransportState,
    },
    dsp::{buffer::ChannelBuffer, declick::DeclickValues},
    event::{NodeEvent, NodeEventList},
    node::{
        AudioNodeProcessor, NodeID, ProcBuffers, ProcInfo, ProcessStatus, StreamStatus,
        TransportInfo, NUM_SCRATCH_BUFFERS,
    },
    SilenceMask, StreamInfo,
};

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

    event_buffer: Vec<NodeEvent>,

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
            clock_samples: InstantSamples(0),
            shared_clock_input,
            proc_transport_state: ProcTransportState::new(),
            hard_clip_outputs,
            scratch_buffers: ChannelBuffer::new(stream_info.max_block_frames.get() as usize),
            declick_values: DeclickValues::new(stream_info.declick_frames),
            poisoned: false,
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
        process_timestamp: B::Instant,
        duration_since_stream_start: Duration,
        mut stream_status: StreamStatus,
        mut dropped_frames: u32,
    ) {
        // --- Poll messages ------------------------------------------------------------------

        self.poll_messages();

        // --- Increment the clock for the next process cycle ---------------------------------

        let mut clock_samples = self.clock_samples;

        // The sample clock is ultimately used as the "source of truth".
        let mut clock_seconds = clock_samples.to_seconds(self.sample_rate, self.sample_rate_recip);

        self.clock_samples += DurationSamples(frames as i64);

        self.sync_shared_clock(Some(process_timestamp));

        // --- Process the audio graph in blocks ----------------------------------------------

        if self.schedule_data.is_none() || frames == 0 {
            output.fill(0.0);
            return;
        };

        assert_eq!(input.len(), frames * num_in_channels);
        assert_eq!(output.len(), frames * num_out_channels);

        let mut frames_processed = 0;
        while frames_processed < frames {
            let mut block_frames = (frames - frames_processed).min(self.max_block_frames);

            let (playhead, proc_transport_info) = self.proc_transport_state.process_block(
                block_frames,
                clock_samples,
                self.sample_rate,
                self.sample_rate_recip,
            );

            // If the transport info changes this block, process up to that change.
            block_frames = proc_transport_info.frames;

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
                clock_seconds + DurationSeconds(block_frames as f64 * self.sample_rate_recip);

            self.process_block(
                block_frames,
                self.sample_rate,
                self.sample_rate_recip,
                self.clock_samples,
                clock_seconds..next_clock_seconds,
                duration_since_stream_start,
                stream_status,
                dropped_frames,
                playhead,
                proc_transport_info,
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
            clock_samples += DurationSamples(block_frames as i64);
            clock_seconds = next_clock_seconds;
            stream_status = StreamStatus::empty();
            dropped_frames = 0;
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
                    self.proc_transport_state.update(
                        &mut new_transport_state,
                        self.clock_samples,
                        self.sample_rate,
                        self.sample_rate_recip,
                    );

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
        sample_rate: NonZeroU32,
        sample_rate_recip: f64,
        clock_samples: InstantSamples,
        clock_seconds: Range<InstantSeconds>,
        duration_since_stream_start: Duration,
        stream_status: StreamStatus,
        dropped_frames: u32,
        playhead: InstantMusical,
        proc_transport_info: ProcTransportInfo,
    ) {
        if self.schedule_data.is_none() {
            return;
        }
        let schedule_data = self.schedule_data.as_mut().unwrap();

        let mut scratch_buffers = self.scratch_buffers.get_mut(self.max_block_frames);

        let transport_info = self
            .proc_transport_state
            .transport_state
            .transport
            .as_ref()
            .map(|transport| {
                let end_beat = if *self.proc_transport_state.transport_state.playing {
                    self.proc_transport_state
                        .playhead(
                            clock_samples + DurationSamples(block_frames as i64),
                            self.sample_rate,
                            self.sample_rate_recip,
                        )
                        .unwrap()
                } else {
                    playhead
                };

                TransportInfo {
                    clock_musical: playhead..end_beat,
                    transport,
                    playing: *self.proc_transport_state.transport_state.playing,
                    start_clock_samples: self
                        .proc_transport_state
                        .transport_state
                        .playing
                        .then(|| self.proc_transport_state.start_clock_samples),
                    beats_per_minute: proc_transport_info.beats_per_minute,
                    delta_bpm_per_frame: proc_transport_info.delta_beats_per_minute,
                }
            });

        let mut proc_info = ProcInfo {
            frames: block_frames,
            in_silence_mask: SilenceMask::default(),
            out_silence_mask: SilenceMask::default(),
            sample_rate,
            sample_rate_recip,
            clock_samples: clock_samples..(clock_samples + DurationSamples(block_frames as i64)),
            clock_seconds: clock_seconds.clone(),
            duration_since_stream_start,
            transport_info,
            stream_status,
            dropped_frames,
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

                // This should be done when pushing an event, but for the sake of an MVP it's done here.
                node_entry.event_indices.sort_unstable_by_key(|id| {
                    self.event_buffer
                        .get(*id as usize)
                        .and_then(|ev| ev.time)
                        .map(|time| time.to_samples(&proc_info))
                });

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

    fn sync_shared_clock(&mut self, process_timestamp: Option<B::Instant>) {
        let (musical_time, transport_is_playing) = if self
            .proc_transport_state
            .transport_state
            .transport
            .is_some()
        {
            if *self.proc_transport_state.transport_state.playing {
                (
                    self.proc_transport_state.playhead(
                        self.clock_samples,
                        self.sample_rate,
                        self.sample_rate_recip,
                    ),
                    true,
                )
            } else {
                (
                    Some(self.proc_transport_state.paused_at_musical_time),
                    false,
                )
            }
        } else {
            (None, false)
        };

        self.shared_clock_input.write(SharedClock {
            clock_samples: self.clock_samples,
            musical_time,
            transport_is_playing,
            process_timestamp,
        });
    }

    fn stream_stopped(&mut self) {
        self.sync_shared_clock(None);

        for (_, node) in self.nodes.iter_mut() {
            node.processor.stream_stopped();
        }
    }

    /// Called when a new audio stream has been started to replace the old one.
    ///
    /// Note, this method gets called on the main thread, not the audio thread.
    pub(crate) fn new_stream(&mut self, stream_info: &StreamInfo) {
        for (_, node) in self.nodes.iter_mut() {
            node.processor.new_stream(stream_info);
        }

        if self.sample_rate != stream_info.sample_rate {
            self.clock_samples = self
                .clock_samples
                .to_seconds(self.sample_rate, self.sample_rate_recip)
                .to_samples(stream_info.sample_rate);

            self.proc_transport_state.update_sample_rate(
                self.sample_rate,
                self.sample_rate_recip,
                stream_info.sample_rate,
            );

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

struct ProcTransportState {
    transport_state: Box<TransportState>,
    start_clock_samples: InstantSamples,
    paused_at_clock_samples: InstantSamples,
    paused_at_musical_time: InstantMusical,
}

impl ProcTransportState {
    fn new() -> Self {
        Self {
            transport_state: Box::new(TransportState::default()),
            start_clock_samples: InstantSamples(0),
            paused_at_clock_samples: InstantSamples(0),
            paused_at_musical_time: InstantMusical(0.0),
        }
    }

    fn playhead(
        &self,
        clock_samples: InstantSamples,
        sample_rate: NonZeroU32,
        sample_rate_recip: f64,
    ) -> Option<InstantMusical> {
        self.transport_state.transport.as_ref().map(|transport| {
            transport.samples_to_musical(
                clock_samples,
                self.start_clock_samples,
                sample_rate,
                sample_rate_recip,
            )
        })
    }

    fn update(
        &mut self,
        new_transport_state: &mut Box<TransportState>,
        clock_samples: InstantSamples,
        sample_rate: NonZeroU32,
        sample_rate_recip: f64,
    ) {
        let mut did_pause = false;

        if let Some(new_transport) = &new_transport_state.transport {
            if self.transport_state.playhead != new_transport_state.playhead
                || self.transport_state.transport.is_none()
            {
                self.start_clock_samples = new_transport.transport_start(
                    clock_samples,
                    *new_transport_state.playhead,
                    sample_rate,
                );
            } else {
                let old_transport = self.transport_state.transport.as_ref().unwrap();

                if *new_transport_state.playing {
                    if !*self.transport_state.playing {
                        // Resume
                        if old_transport == new_transport {
                            self.start_clock_samples +=
                                clock_samples - self.paused_at_clock_samples;
                        } else {
                            self.start_clock_samples = new_transport.transport_start(
                                clock_samples,
                                self.paused_at_musical_time,
                                sample_rate,
                            );
                        }
                    } else if old_transport != new_transport {
                        // Continue where the previous left off
                        let current_playhead = old_transport.samples_to_musical(
                            clock_samples,
                            self.start_clock_samples,
                            sample_rate,
                            sample_rate_recip,
                        );
                        self.start_clock_samples = new_transport.transport_start(
                            clock_samples,
                            current_playhead,
                            sample_rate,
                        );
                    }
                } else if *self.transport_state.playing {
                    // Pause
                    did_pause = true;

                    self.paused_at_clock_samples = clock_samples;
                    self.paused_at_musical_time = old_transport.samples_to_musical(
                        clock_samples,
                        self.start_clock_samples,
                        sample_rate,
                        sample_rate_recip,
                    );
                }
            }
        }

        if !did_pause {
            self.paused_at_clock_samples = clock_samples;
            self.paused_at_musical_time = *new_transport_state.playhead;
        }

        core::mem::swap(new_transport_state, &mut self.transport_state);
    }

    /// Update the transport
    ///
    /// Returns (playhead, proc_transport_info)
    fn process_block(
        &mut self,
        frames: usize,
        clock_samples: InstantSamples,
        sample_rate: NonZeroU32,
        sample_rate_recip: f64,
    ) -> (InstantMusical, ProcTransportInfo) {
        let Some(transport) = &self.transport_state.transport else {
            return (
                InstantMusical::ZERO,
                ProcTransportInfo {
                    frames,
                    beats_per_minute: 0.0,
                    delta_beats_per_minute: 0.0,
                },
            );
        };

        let mut playhead = transport.samples_to_musical(
            clock_samples,
            self.start_clock_samples,
            sample_rate,
            sample_rate_recip,
        );
        let beats_per_minute = transport.bpm_at_musical(playhead);

        if !*self.transport_state.playing {
            return (
                playhead,
                ProcTransportInfo {
                    frames,
                    beats_per_minute,
                    delta_beats_per_minute: 0.0,
                },
            );
        }

        let mut loop_end_clock_samples = InstantSamples::default();
        let mut stop_at_clock_samples = InstantSamples::default();

        if let Some(loop_range) = &self.transport_state.loop_range {
            loop_end_clock_samples =
                transport.musical_to_samples(loop_range.end, self.start_clock_samples, sample_rate);

            if clock_samples >= loop_end_clock_samples {
                // Loop back to start of loop.
                self.start_clock_samples =
                    transport.transport_start(clock_samples, loop_range.start, sample_rate);
                playhead = loop_range.start;
            }
        } else if let Some(stop_at) = self.transport_state.stop_at {
            stop_at_clock_samples =
                transport.musical_to_samples(stop_at, self.start_clock_samples, sample_rate);

            if clock_samples >= stop_at_clock_samples {
                // Stop the transport.
                *self.transport_state.playing = false;
                return (
                    stop_at,
                    ProcTransportInfo {
                        frames,
                        beats_per_minute,
                        delta_beats_per_minute: 0.0,
                    },
                );
            }
        }

        let mut info = transport.proc_transport_info(frames, playhead);

        let proc_end_samples = clock_samples + DurationSamples(info.frames as i64);

        if self.transport_state.loop_range.is_some() {
            if proc_end_samples > loop_end_clock_samples {
                // End of the loop reached.
                info.frames = (loop_end_clock_samples - clock_samples).0.max(0) as usize;
            }
        } else if self.transport_state.stop_at.is_some() {
            if proc_end_samples > stop_at_clock_samples {
                // End of the transport reached.
                info.frames = (stop_at_clock_samples - clock_samples).0.max(0) as usize;
            }
        }

        (playhead, info)
    }

    fn update_sample_rate(
        &mut self,
        old_sample_rate: NonZeroU32,
        old_sample_rate_recip: f64,
        new_sample_rate: NonZeroU32,
    ) {
        self.start_clock_samples = self
            .start_clock_samples
            .to_seconds(old_sample_rate, old_sample_rate_recip)
            .to_samples(new_sample_rate);
        self.paused_at_clock_samples = self
            .paused_at_clock_samples
            .to_seconds(old_sample_rate, old_sample_rate_recip)
            .to_samples(new_sample_rate);
    }
}
