use std::{num::NonZeroU32, time::Duration};

use arrayvec::ArrayVec;
use firewheel_core::{
    channel_config::MAX_CHANNELS,
    clock::{DurationSamples, InstantSamples, ProcTransportInfo},
    event::NodeEventList,
    node::{NodeID, ProcBuffers, ProcInfo, ProcessStatus, StreamStatus},
    SilenceMask,
};

use crate::{
    backend::AudioBackend,
    processor::{event_scheduler::SubChunkInfo, FirewheelProcessorInner, NodeEntry, SharedClock},
};

impl<B: AudioBackend> FirewheelProcessorInner<B> {
    // TODO: Add a `process_deinterleaved` method.

    /// Process the given buffers of audio data.
    pub fn process_interleaved(
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

            // Get the transport info for this block.
            let proc_transport_info = self.proc_transport_state.process_block(
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

            // Process the block.
            self.process_block(
                block_frames,
                self.sample_rate,
                self.sample_rate_recip,
                clock_samples,
                duration_since_stream_start,
                stream_status,
                dropped_frames,
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

            // Advance to the next processing block.
            frames_processed += block_frames;
            clock_samples += DurationSamples(block_frames as i64);
            stream_status = StreamStatus::empty();
            dropped_frames = 0;
        }

        // --- Hard clip outputs --------------------------------------------------------------

        if self.hard_clip_outputs {
            for s in output.iter_mut() {
                *s = s.fract();
            }
        }
    }

    fn process_block(
        &mut self,
        block_frames: usize,
        sample_rate: NonZeroU32,
        sample_rate_recip: f64,
        clock_samples: InstantSamples,
        duration_since_stream_start: Duration,
        stream_status: StreamStatus,
        dropped_frames: u32,
        proc_transport_info: &ProcTransportInfo,
    ) {
        if self.schedule_data.is_none() {
            return;
        }
        let schedule_data = self.schedule_data.as_mut().unwrap();

        // -- Prepare process info ------------------------------------------------------------

        let mut scratch_buffers = self.scratch_buffers.get_mut(self.max_block_frames);

        let transport_info = self
            .proc_transport_state
            .transport_info(&proc_transport_info);

        let mut proc_info = ProcInfo {
            frames: block_frames,
            in_silence_mask: SilenceMask::default(),
            out_silence_mask: SilenceMask::default(),
            sample_rate,
            sample_rate_recip,
            clock_samples,
            duration_since_stream_start,
            transport_info,
            stream_status,
            dropped_frames,
            declick_values: &self.declick_values,
        };

        // -- Find scheduled events that have elapsed this block ------------------------------

        self.event_scheduler
            .prepare_process_block(&proc_info, &mut self.nodes);

        // -- Audio graph node processing closure ---------------------------------------------

        schedule_data.schedule.process(
            block_frames,
            &mut scratch_buffers,
            |node_id: NodeID,
             in_silence_mask: SilenceMask,
             out_silence_mask: SilenceMask,
             proc_buffers|
             -> ProcessStatus {
                let node_entry = self.nodes.get_mut(node_id.0).unwrap();

                // Add the silence mask information to proc info.
                proc_info.in_silence_mask = in_silence_mask;
                proc_info.out_silence_mask = out_silence_mask;

                // Used to keep track of what status this closure should return.
                let mut prev_process_status = None;
                let mut final_silence_mask = None;

                // Process in sub-chunks for each new scheduled event (or process a single
                // chunk if there are no scheduled events).
                self.event_scheduler.process_node(
                    node_id,
                    node_entry,
                    block_frames,
                    clock_samples,
                    &mut proc_info,
                    &mut self.node_event_queue,
                    proc_buffers,
                    |sub_chunk_info: SubChunkInfo,
                     node_entry: &mut NodeEntry,
                     proc_info: &mut ProcInfo,
                     events: &mut NodeEventList,
                     proc_buffers: &mut ProcBuffers| {
                        let SubChunkInfo {
                            sub_chunk_range,
                            sub_clock_samples,
                        } = sub_chunk_info;
                        let sub_chunk_frames = sub_chunk_range.end - sub_chunk_range.start;

                        // Set the timing information for the process info for this sub-chunk.
                        proc_info.frames = sub_chunk_frames;
                        proc_info.clock_samples = sub_clock_samples;
                        if let Some(transport) = &mut proc_info.transport_info {
                            // For now this isn't really necessary, but it will be once support
                            // for linearly automated tempo is added.
                            transport.beats_per_minute = proc_transport_info
                                .bpm_at_frame(sub_chunk_range.start)
                                .unwrap();
                        }

                        // Call the node's process method.
                        let process_status = {
                            if sub_chunk_frames == block_frames {
                                // If this is the only sub-chunk (because there are no scheduled
                                // events), there is no need to edit the buffer slices.
                                let sub_proc_buffers = ProcBuffers {
                                    inputs: proc_buffers.inputs,
                                    outputs: proc_buffers.outputs,
                                    scratch_buffers: proc_buffers.scratch_buffers,
                                };

                                node_entry
                                    .processor
                                    .process(sub_proc_buffers, &proc_info, events)
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
                                    // Scratch buffers don't need to change length.
                                    scratch_buffers: proc_buffers.scratch_buffers,
                                };

                                node_entry
                                    .processor
                                    .process(sub_proc_buffers, &proc_info, events)
                            }
                        };

                        // If there are multiple sub-chunks, and the node returned a different process
                        // status this sub-chunk than the previous sub-chunk, then we must manually
                        // handle the process statuses.
                        if final_silence_mask.is_none() {
                            if let Some(prev_process_status) = prev_process_status {
                                if prev_process_status != process_status {
                                    // Handle the process status for the sub-chunk(s) before this
                                    // sub-chunk.
                                    match prev_process_status {
                                        ProcessStatus::ClearAllOutputs => {
                                            for out_ch in proc_buffers.outputs.iter_mut() {
                                                out_ch[0..sub_chunk_range.start].fill(0.0);
                                            }

                                            final_silence_mask = Some(SilenceMask::new_all_silent(
                                                proc_buffers.outputs.len(),
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

                                            final_silence_mask = Some(in_silence_mask);
                                        }
                                        ProcessStatus::OutputsModified { out_silence_mask } => {
                                            final_silence_mask = Some(out_silence_mask);
                                        }
                                    }
                                }
                            }
                        }
                        prev_process_status = Some(process_status);

                        // If we are manually handling process statuses, handle the process status
                        // for this sub-chunk.
                        if let Some(final_silence_mask) = &mut final_silence_mask {
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

                                    final_silence_mask.union_with(in_silence_mask);
                                }
                                ProcessStatus::OutputsModified { out_silence_mask } => {
                                    final_silence_mask.union_with(out_silence_mask);
                                }
                            }
                        }
                    },
                );

                // -- Done processing in sub-chunks. Return the final process status. ---------

                if let Some(final_silence_mask) = final_silence_mask {
                    // If we manually handled process statuses, return the calculated silence
                    // mask.
                    ProcessStatus::OutputsModified {
                        out_silence_mask: final_silence_mask,
                    }
                } else {
                    // Else return the process status returned by the node's proces method.
                    prev_process_status.unwrap()
                }
            },
        );

        // -- Clean up event buffers ----------------------------------------------------------

        self.event_scheduler.cleanup_process_block();
    }

    pub fn sync_shared_clock(&mut self, process_timestamp: Option<B::Instant>) {
        let (musical_time, transport_is_playing) = self.proc_transport_state.shared_clock_info(
            self.clock_samples,
            self.sample_rate,
            self.sample_rate_recip,
        );

        self.shared_clock_input.write(SharedClock {
            clock_samples: self.clock_samples,
            musical_time,
            transport_is_playing,
            process_timestamp,
        });
    }
}
