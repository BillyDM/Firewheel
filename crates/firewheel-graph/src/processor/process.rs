use audioadapter::{Adapter, AdapterMut};
use bevy_platform::sync::{atomic::Ordering, Arc};
use core::{num::NonZeroU32, time::Duration};

use arrayvec::ArrayVec;
use firewheel_core::{
    channel_config::MAX_CHANNELS,
    clock::{DurationSamples, InstantSamples},
    event::ProcEvents,
    log::RealtimeLogger,
    mask::{ConnectedMask, ConstantMask, MaskType, SilenceMask},
    node::{NodeID, ProcBuffers, ProcExtra, ProcInfo, ProcessStatus, StreamStatus},
};

use crate::{
    backend::BackendProcessInfo,
    processor::{event_scheduler::SubChunkInfo, FirewheelProcessorInner, NodeEntry, StatusFlags},
    FirewheelFlags,
};

#[cfg(feature = "scheduled_events")]
use crate::processor::SharedClock;
#[cfg(feature = "scheduled_events")]
use bevy_platform::time::Instant;

#[cfg(feature = "musical_transport")]
use firewheel_core::clock::ProcTransportInfo;

impl FirewheelProcessorInner {
    /// Process the given buffers of audio data.
    pub fn process(
        &mut self,
        input: &dyn Adapter<'_, f32>,
        output: &mut dyn AdapterMut<'_, f32>,
        info: BackendProcessInfo,
    ) {
        let BackendProcessInfo {
            frames,
            process_timestamp,
            duration_since_stream_start,
            input_stream_status,
            mut output_stream_status,
            mut dropped_frames,
            process_to_playback_delay,
        } = info;

        let num_in_channels = input.channels();
        let num_out_channels = output.channels();

        #[cfg(feature = "scheduled_events")]
        let process_timestamp = process_timestamp.unwrap_or_else(|| Instant::now());
        #[cfg(not(feature = "scheduled_events"))]
        let _ = process_timestamp;

        if input_stream_status.contains(StreamStatus::INPUT_OVERFLOW) {
            let _ = self.extra.logger.try_error("Firewheel input to output stream channel overflowed! Try increasing the capacity of the channel.");
        }
        if input_stream_status.contains(StreamStatus::OUTPUT_UNDERFLOW) {
            let _ = self.extra.logger.try_error("Firewheel input to output stream channel underflowed! Try increasing the latency of the channel.");
        }

        // --- Poll messages ------------------------------------------------------------------

        self.poll_messages();

        // --- Increment the clock for the next process cycle ---------------------------------

        let mut clock_samples = self.clock_samples;

        self.clock_samples += DurationSamples(frames as i64);

        #[cfg(feature = "scheduled_events")]
        self.sync_shared_clock(process_timestamp);

        // --- Process the audio graph in blocks ----------------------------------------------

        if self.schedule_data.is_none() || frames == 0 {
            output.fill_frames_with(0, frames, &0.0);
            return;
        };

        #[cfg(feature = "unsafe_flush_denormals_to_zero")]
        let _ftz_gaurd = crate::ftz::ScopedFtz::enable();

        let mut frames_processed = 0;
        while frames_processed < frames {
            let block_frames = (frames - frames_processed).min(self.max_block_frames);

            // Get the transport info for this block.
            #[cfg(feature = "musical_transport")]
            let proc_transport_info = self.proc_transport_state.process_block(
                block_frames,
                clock_samples,
                self.sample_rate,
                self.sample_rate_recip,
            );

            // If the transport info changes this block, process up to that change.
            #[cfg(feature = "musical_transport")]
            let block_frames = proc_transport_info.frames;

            // If any pre-process node has a scheduled event this block, process up to
            // that change.
            #[cfg(feature = "scheduled_events")]
            let block_frames = self.num_pre_process_frames(block_frames, clock_samples);

            // Prepare graph input buffers.
            self.schedule_data
                .as_mut()
                .unwrap()
                .schedule
                .prepare_graph_inputs(
                    block_frames,
                    num_in_channels,
                    |channels: &mut [&mut [f32]]| -> SilenceMask {
                        let mut silence_mask = SilenceMask::NONE_SILENT;

                        for (ch_i, ch) in channels.iter_mut().enumerate().take(num_in_channels) {
                            input.copy_from_channel_to_slice(
                                ch_i,
                                frames_processed,
                                &mut ch[..block_frames],
                            );
                        }

                        for (ch_i, ch) in channels.iter_mut().enumerate().skip(num_in_channels) {
                            ch[..block_frames].fill(0.0);
                            silence_mask.set_channel(ch_i, true);
                        }

                        silence_mask
                    },
                );

            // Process the block.
            self.process_block(
                block_frames,
                self.sample_rate,
                self.sample_rate_recip,
                clock_samples,
                duration_since_stream_start,
                output_stream_status,
                dropped_frames,
                process_to_playback_delay,
                #[cfg(feature = "musical_transport")]
                &proc_transport_info,
            );

            // Copy the output of the audio graph to the output buffer.
            self.schedule_data
                .as_mut()
                .unwrap()
                .schedule
                .read_graph_outputs(
                    block_frames,
                    num_out_channels,
                    |channels: &mut [&mut [f32]], silence_mask| {
                        validate_output(
                            channels,
                            &self.flags,
                            &self.status_flags,
                            &mut self.extra.logger,
                        );

                        for (ch_i, ch) in channels.iter().enumerate().take(num_out_channels) {
                            if silence_mask.is_channel_silent(ch_i) {
                                output.fill_frames_with(frames_processed, block_frames, &0.0);
                            } else {
                                output.copy_from_slice_to_channel(
                                    ch_i,
                                    frames_processed,
                                    &ch[..block_frames],
                                );
                            }
                        }
                    },
                );

            // Advance to the next processing block.
            frames_processed += block_frames;
            clock_samples += DurationSamples(block_frames as i64);
            output_stream_status = StreamStatus::empty();
            dropped_frames = 0;
        }
    }

    #[cfg(feature = "scheduled_events")]
    fn num_pre_process_frames(
        &mut self,
        block_frames: usize,
        clock_samples: InstantSamples,
    ) -> usize {
        if self.schedule_data.is_none() {
            return block_frames;
        }
        let schedule_data = self.schedule_data.as_ref().unwrap();

        if !schedule_data.schedule.has_pre_proc_nodes() {
            return block_frames;
        }

        let clock_samples_range =
            clock_samples..clock_samples + DurationSamples(block_frames as i64);
        self.event_scheduler
            .num_pre_process_frames(block_frames, clock_samples_range)
    }

    #[expect(clippy::too_many_arguments, reason = "Function needs many arguments")]
    fn process_block(
        &mut self,
        block_frames: usize,
        sample_rate: NonZeroU32,
        sample_rate_recip: f64,
        clock_samples: InstantSamples,
        duration_since_stream_start: Duration,
        stream_status: StreamStatus,
        dropped_frames: u32,
        process_to_playback_delay: Option<Duration>,
        #[cfg(feature = "musical_transport")] proc_transport_info: &ProcTransportInfo,
    ) {
        if self.schedule_data.is_none() {
            return;
        }
        let schedule_data = self.schedule_data.as_mut().unwrap();

        // -- Prepare process info ------------------------------------------------------------

        #[cfg(feature = "musical_transport")]
        let transport_info = self
            .proc_transport_state
            .transport_info(&proc_transport_info);

        let mut info = ProcInfo {
            frames: block_frames,
            in_silence_mask: SilenceMask::default(),
            out_silence_mask: SilenceMask::default(),
            in_constant_mask: ConstantMask::default(),
            out_constant_mask: ConstantMask::default(),
            in_connected_mask: ConnectedMask::default(),
            out_connected_mask: ConnectedMask::default(),
            prev_output_was_silent: false,
            sample_rate,
            sample_rate_recip,
            clock_samples,
            duration_since_stream_start,
            stream_status,
            dropped_frames,
            process_to_playback_delay,
            #[cfg(feature = "musical_transport")]
            transport_info,
        };

        // -- Find scheduled events that have elapsed this block ------------------------------

        #[cfg(feature = "scheduled_events")]
        self.event_scheduler
            .prepare_process_block(&info, &mut self.nodes);

        // -- Audio graph node processing closure ---------------------------------------------

        schedule_data.schedule.process(
            block_frames,
            self.flags,
            |node_id: NodeID,
             in_silence_mask: SilenceMask,
             out_silence_mask: SilenceMask,
             in_constant_mask: ConstantMask,
             out_constant_mask: ConstantMask,
             in_connected_mask: ConnectedMask,
             out_connected_mask: ConnectedMask,
             proc_buffers|
             -> ProcessStatus {
                let node_entry = self.nodes.get_mut(node_id.0).unwrap();

                // Add the mask information to proc info.
                info.in_silence_mask = in_silence_mask;
                info.out_silence_mask = out_silence_mask;
                info.in_constant_mask = in_constant_mask;
                info.out_constant_mask = out_constant_mask;
                info.in_connected_mask = in_connected_mask;
                info.out_connected_mask = out_connected_mask;

                // Used to keep track of what status this closure should return.
                let mut prev_process_status = None;
                let mut final_mask = None;

                // Process in sub-chunks for each new scheduled event (or process a single
                // chunk if there are no scheduled events).
                self.event_scheduler.process_node(
                    node_id,
                    node_entry,
                    block_frames,
                    clock_samples,
                    &mut info,
                    &mut self.extra,
                    &mut self.proc_event_queue,
                    proc_buffers,
                    |sub_chunk_info: SubChunkInfo,
                     node_entry: &mut NodeEntry,
                     info: &mut ProcInfo,
                     proc_buffers: &mut ProcBuffers,
                     events: &mut ProcEvents,
                     extra: &mut ProcExtra| {
                        let SubChunkInfo {
                            sub_chunk_range,
                            sub_clock_samples,
                        } = sub_chunk_info;
                        let sub_chunk_frames = sub_chunk_range.end - sub_chunk_range.start;

                        // Set the timing information for the process info for this sub-chunk.
                        info.frames = sub_chunk_frames;
                        info.clock_samples = sub_clock_samples;
                        info.prev_output_was_silent = node_entry.prev_output_was_silent;

                        // Call the node's process method.
                        let process_status = {
                            if sub_chunk_frames == block_frames {
                                // If this is the only sub-chunk (because there are no scheduled
                                // events), there is no need to edit the buffer slices.
                                let sub_proc_buffers = ProcBuffers {
                                    inputs: proc_buffers.inputs,
                                    outputs: proc_buffers.outputs,
                                };

                                node_entry
                                    .processor
                                    .process(info, sub_proc_buffers, events, extra)
                            } else {
                                // Else if there are multiple sub-chunks, edit the range of each
                                // buffer slice to cover the range of this sub-chunk.

                                let mut sub_inputs: ArrayVec<&[f32], MAX_CHANNELS> =
                                    ArrayVec::new();
                                let mut sub_outputs: ArrayVec<&mut [f32], MAX_CHANNELS> =
                                    ArrayVec::new();

                                // TODO: We can use unsafe slicing here since we know the range is
                                // always valid.
                                for ch in proc_buffers.inputs.iter() {
                                    sub_inputs.push(&ch[sub_chunk_range.clone()]);
                                }
                                for ch in proc_buffers.outputs.iter_mut() {
                                    sub_outputs.push(&mut ch[sub_chunk_range.clone()]);
                                }

                                let sub_proc_buffers = ProcBuffers {
                                    inputs: sub_inputs.as_slice(),
                                    outputs: sub_outputs.as_mut_slice(),
                                };

                                node_entry
                                    .processor
                                    .process(info, sub_proc_buffers, events, extra)
                            }
                        };

                        node_entry.prev_output_was_silent = match process_status {
                            ProcessStatus::ClearAllOutputs => true,
                            ProcessStatus::Bypass => info
                                .in_silence_mask
                                .all_channels_silent(proc_buffers.inputs.len()),
                            ProcessStatus::OutputsModified => false,
                            ProcessStatus::OutputsModifiedWithMask(out_mask) => match out_mask {
                                MaskType::Silence(mask) => {
                                    mask.all_channels_silent(proc_buffers.outputs.len())
                                }
                                MaskType::Constant(_) => false,
                            },
                        };

                        // If there are multiple sub-chunks, and the node returned a different process
                        // status this sub-chunk than the previous sub-chunk, then we must manually
                        // handle the process statuses.
                        if final_mask.is_none() {
                            if let Some(prev_process_status) = prev_process_status {
                                if prev_process_status != process_status {
                                    // Handle the process status for the sub-chunk(s) before this
                                    // sub-chunk.
                                    match prev_process_status {
                                        ProcessStatus::ClearAllOutputs => {
                                            for out_ch in proc_buffers.outputs.iter_mut() {
                                                out_ch[0..sub_chunk_range.start].fill(0.0);
                                            }

                                            final_mask = Some(MaskType::Silence(
                                                SilenceMask::new_all_silent(
                                                    proc_buffers.outputs.len(),
                                                ),
                                            ));
                                        }
                                        ProcessStatus::Bypass => {
                                            for (out_ch, in_ch) in proc_buffers
                                                .outputs
                                                .iter_mut()
                                                .zip(proc_buffers.inputs.iter())
                                            {
                                                out_ch[0..sub_chunk_range.start].copy_from_slice(
                                                    &in_ch[0..sub_chunk_range.start],
                                                );
                                            }
                                            for out_ch in proc_buffers
                                                .outputs
                                                .iter_mut()
                                                .skip(proc_buffers.inputs.len())
                                            {
                                                out_ch[0..sub_chunk_range.start].fill(0.0);
                                            }

                                            final_mask = Some(MaskType::Silence(in_silence_mask));
                                        }
                                        ProcessStatus::OutputsModified => {
                                            final_mask =
                                                Some(MaskType::Silence(SilenceMask::NONE_SILENT));
                                        }
                                        ProcessStatus::OutputsModifiedWithMask(out_mask) => {
                                            final_mask = Some(out_mask);
                                        }
                                    }
                                }
                            }
                        }
                        prev_process_status = Some(process_status);

                        // If we are manually handling process statuses, handle the process status
                        // for this sub-chunk.
                        if let Some(final_mask) = &mut final_mask {
                            match process_status {
                                ProcessStatus::ClearAllOutputs => {
                                    for out_ch in proc_buffers.outputs.iter_mut() {
                                        out_ch[sub_chunk_range.clone()].fill(0.0);
                                    }
                                }
                                ProcessStatus::Bypass => {
                                    for (out_ch, in_ch) in proc_buffers
                                        .outputs
                                        .iter_mut()
                                        .zip(proc_buffers.inputs.iter())
                                    {
                                        out_ch[sub_chunk_range.clone()]
                                            .copy_from_slice(&in_ch[sub_chunk_range.clone()]);
                                    }
                                    for out_ch in proc_buffers
                                        .outputs
                                        .iter_mut()
                                        .skip(proc_buffers.inputs.len())
                                    {
                                        out_ch[sub_chunk_range.clone()].fill(0.0);
                                    }

                                    if let MaskType::Silence(s) = final_mask {
                                        s.union_with(in_silence_mask);
                                    } else {
                                        *final_mask = MaskType::Silence(SilenceMask::NONE_SILENT);
                                    }
                                }
                                ProcessStatus::OutputsModified => {
                                    *final_mask = MaskType::Silence(SilenceMask::NONE_SILENT);
                                }
                                ProcessStatus::OutputsModifiedWithMask(out_mask) => {
                                    match out_mask {
                                        MaskType::Silence(mask) => {
                                            if let MaskType::Silence(final_mask) = final_mask {
                                                final_mask.union_with(mask);
                                            } else {
                                                *final_mask =
                                                    MaskType::Silence(SilenceMask::NONE_SILENT);
                                            }
                                        }
                                        MaskType::Constant(mask) => {
                                            if let MaskType::Constant(final_mask) = final_mask {
                                                final_mask.union_with(mask);

                                                for (i, buf) in
                                                    proc_buffers.outputs.iter().enumerate()
                                                {
                                                    if final_mask.is_channel_constant(i)
                                                        && buf[0] != buf[sub_chunk_range.start]
                                                    {
                                                        final_mask.set_channel(i, false);
                                                    }
                                                }
                                            } else {
                                                *final_mask =
                                                    MaskType::Silence(SilenceMask::NONE_SILENT);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    },
                );

                // -- Done processing in sub-chunks. Return the final process status. ---------

                if let Some(final_mask) = final_mask {
                    // If we manually handled process statuses, return the calculated silence
                    // mask.
                    ProcessStatus::OutputsModifiedWithMask(final_mask)
                } else {
                    // Else return the process status returned by the node's proces method.
                    prev_process_status.unwrap()
                }
            },
        );

        // -- Clean up event buffers ----------------------------------------------------------

        self.event_scheduler.cleanup_process_block();
    }

    #[cfg(feature = "scheduled_events")]
    pub fn sync_shared_clock(&mut self, process_timestamp: Instant) {
        #[cfg(feature = "musical_transport")]
        let shared_clock_info = self.proc_transport_state.shared_clock_info(
            self.clock_samples,
            self.sample_rate,
            self.sample_rate_recip,
        );

        self.shared_clock_input.write(SharedClock {
            clock_samples: self.clock_samples,
            #[cfg(feature = "musical_transport")]
            current_playhead: shared_clock_info.current_playhead,
            #[cfg(feature = "musical_transport")]
            speed_multiplier: shared_clock_info.speed_multiplier,
            #[cfg(feature = "musical_transport")]
            transport_is_playing: shared_clock_info.transport_is_playing,
            update_instant: process_timestamp,
        });
    }
}

fn validate_output(
    output: &mut [&mut [f32]],
    flags: &FirewheelFlags,
    status_flags: &Arc<StatusFlags>,
    logger: &mut RealtimeLogger,
) {
    if flags.validate_output_is_finite {
        let mut non_finite_value = 0.0;

        for ch in output.iter_mut() {
            for s in ch.iter_mut() {
                // Try to optimize for auto-vectorization
                let is_finite = s.is_finite();

                non_finite_value = if is_finite { non_finite_value } else { *s };
                *s = if is_finite { *s } else { 0.0 };
            }
        }

        if non_finite_value != 0.0 {
            let _ = logger.try_error_with(|s| {
                #[cfg(feature = "std")]
                {
                    *s = format!(
                        "Non-finite number detected on audio output: {}",
                        non_finite_value
                    );
                }

                #[cfg(not(feature = "std"))]
                {
                    *s = bevy_platform::prelude::String::from(
                        "Non-finite number detected on audio output",
                    );
                }
            });
        }
    }

    if flags.detect_clipping_on_output {
        let mut clipping_occurred = false;

        if flags.hard_clip_outputs {
            for ch in output.iter_mut() {
                for s in ch.iter_mut() {
                    // Try to optimize for auto-vectorization
                    let ss = *s;
                    *s = s.clamp(-1.0, 1.0);

                    clipping_occurred |= ss != *s;
                }
            }
        } else {
            for ch in output.iter_mut() {
                for s in ch.iter() {
                    clipping_occurred |= *s < -1.0 || *s > 1.0;
                }
            }
        }

        if clipping_occurred {
            status_flags.clipping_occured.store(true, Ordering::Relaxed);
        }
    } else if flags.hard_clip_outputs {
        for ch in output.iter_mut() {
            for s in ch.iter_mut() {
                *s = s.clamp(-1.0, 1.0);
            }
        }
    }
}
