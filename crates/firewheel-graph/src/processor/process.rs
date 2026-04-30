use audioadapter::{Adapter, AdapterMut};
use bevy_platform::sync::{atomic::Ordering, Arc};
use core::{num::NonZeroU32, time::Duration};

use arrayvec::ArrayVec;
use firewheel_core::{
    channel_config::MAX_CHANNELS,
    clock::{DurationSamples, InstantSamples},
    dsp::declick::{DeclickFadeCurve, Declicker},
    log::RealtimeLogger,
    mask::{ConnectedMask, ConstantMask, MaskType, SilenceMask},
    node::{ProcBuffers, ProcInfo, ProcessStatus, StreamStatus},
};

use crate::{
    backend::BackendProcessInfo,
    context::FirewheelBitFlags,
    graph::ProcessNodeInfo,
    processor::{event_scheduler::ProcessSubChunkInfo, FirewheelProcessorInner, SharedFlags},
};

#[cfg(feature = "scheduled_events")]
use crate::processor::SharedClock;
use bevy_platform::time::Instant;

#[cfg(feature = "musical_transport")]
use firewheel_core::clock::ProcTransportInfo;

/// A rough estimate of the amount of overhead occured by the OS's audio thread.
// TODO: Do research to find the optimal value.
const SYSTEM_OVERHEAD_DURATION_SECS: f64 = 1.0 / 1_000.0;
const UNDERFLOW_LOG_COOLDOWN: Duration = Duration::from_secs(3);

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

        let process_timestamp = process_timestamp.unwrap_or_else(|| Instant::now());

        let total_cpu_seconds_recip = ((frames as f64 * self.sample_rate_recip)
            - SYSTEM_OVERHEAD_DURATION_SECS)
            .max(SYSTEM_OVERHEAD_DURATION_SECS)
            .recip();

        self.profiler_tx
            .new_process_loop(process_timestamp, total_cpu_seconds_recip, &self.flags);

        let num_in_channels = input.channels();
        let num_out_channels = output.channels();

        if input_stream_status.contains(StreamStatus::INPUT_OVERFLOW) {
            let mut do_send = true;
            if let Some(instant) = self.last_input_overflow_log_instant {
                if let Some(duration) = process_timestamp.checked_duration_since(instant) {
                    do_send = duration >= UNDERFLOW_LOG_COOLDOWN;
                }
            }

            if do_send {
                self.last_input_overflow_log_instant = Some(process_timestamp);
                let _ = self.extra.logger.try_error("Firewheel input to output stream channel overflowed! Try increasing the capacity of the channel.");
            }
        }
        if input_stream_status.contains(StreamStatus::OUTPUT_UNDERFLOW) {
            let mut do_send = true;
            if let Some(instant) = self.last_output_underflow_log_instant {
                if let Some(duration) = process_timestamp.checked_duration_since(instant) {
                    do_send = duration >= UNDERFLOW_LOG_COOLDOWN;
                }
            }

            if do_send {
                self.last_output_underflow_log_instant = Some(process_timestamp);
                let _ = self.extra.logger.try_error("Firewheel input to output stream channel underflowed! Try increasing the latency of the channel.");
            }
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
                    self.flags.contains(FirewheelBitFlags::FORCE_CLEAR_BUFFERS),
                    |channels: &mut [&mut [f32]]| -> SilenceMask {
                        let mut silence_mask = SilenceMask::NONE_SILENT;

                        for (ch_i, ch) in channels.iter_mut().enumerate().take(num_in_channels) {
                            input.copy_from_channel_to_slice(
                                ch_i,
                                frames_processed,
                                &mut ch[..block_frames],
                            );

                            let mut input_is_silent = true;
                            if let Some(min_amp) = self.clamp_graph_inputs_below_amp {
                                for s in ch[..block_frames].iter() {
                                    if s.abs() >= min_amp {
                                        input_is_silent = false;
                                        break;
                                    }
                                }

                                if input_is_silent {
                                    ch[..block_frames].fill(0.0);
                                }
                            } else {
                                for s in ch[..block_frames].iter() {
                                    if *s != 0.0 {
                                        input_is_silent = false;
                                        break;
                                    }
                                }
                            }

                            silence_mask.set_channel(ch_i, input_is_silent);
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
                total_cpu_seconds_recip,
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
                            &self.shared_flags,
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

        self.profiler_tx.process_loop_completed();
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
        total_cpu_seconds_recip: f64,
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
            total_cpu_seconds_recip,
            duration_since_stream_start,
            stream_status,
            dropped_frames,
            process_to_playback_delay,
            did_just_unbypass: false,
            #[cfg(feature = "musical_transport")]
            transport_info,
        };

        let force_clear_buffers = self.flags.contains(FirewheelBitFlags::FORCE_CLEAR_BUFFERS);

        // -- Find scheduled events that have elapsed this block ------------------------------

        #[cfg(feature = "scheduled_events")]
        self.event_scheduler
            .prepare_process_block(&info, &mut self.nodes);

        // -- Audio graph node processing closure ---------------------------------------------

        self.profiler_tx.bookkeeping_part_completed();

        #[cfg(feature = "node_profiling")]
        self.profiler_tx.begin_node_profiling();

        schedule_data.schedule.process(
            block_frames,
            force_clear_buffers,
            |proc_node_info: ProcessNodeInfo| -> ProcessStatus {
                let ProcessNodeInfo {
                    node_id,
                    in_silence_mask,
                    out_silence_mask,
                    in_constant_mask,
                    out_constant_mask,
                    in_connected_mask,
                    out_connected_mask,
                    proc_buffers,
                    bypass_declick_buffer,
                } = proc_node_info;

                let node_entry = self.nodes.get_mut(node_id.0).unwrap();

                // Add the mask information to proc info.
                info.in_silence_mask = in_silence_mask;
                info.in_constant_mask = in_constant_mask;
                info.out_silence_mask = out_silence_mask;
                info.out_constant_mask = out_constant_mask;
                info.in_connected_mask = in_connected_mask;
                info.out_connected_mask = out_connected_mask;

                // Used to keep track of what status this closure should return.
                let mut prev_process_status = None;
                let mut final_mask = None;

                let mut is_bypassed = node_entry.bypass_declick == Declicker::SettledAt0;
                let mut is_bypass_declicking = !node_entry.bypass_declick.has_settled();
                let has_outputs = !proc_buffers.outputs.is_empty();

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
                    |proc_sub_chunk_info: ProcessSubChunkInfo| {
                        let ProcessSubChunkInfo {
                            sub_chunk_range,
                            sub_clock_samples,
                            node_entry,
                            info,
                            proc_buffers,
                            events,
                            extra,
                            set_bypassed,
                        } = proc_sub_chunk_info;

                        let sub_chunk_frames = sub_chunk_range.end - sub_chunk_range.start;

                        if let Some(bypassed) = set_bypassed {
                            if bypassed {
                                if node_entry.bypass_declick != Declicker::SettledAt0 {
                                    if has_outputs {
                                        node_entry.bypass_declick.fade_to_0(&extra.declick_values);
                                        is_bypass_declicking = true;
                                        is_bypassed = false;
                                    } else {
                                        node_entry.bypass_declick = Declicker::SettledAt0;
                                        is_bypass_declicking = false;
                                        is_bypassed = true;
                                    }
                                } // else already bypassed
                            } else {
                                if node_entry.bypass_declick != Declicker::SettledAt1 {
                                    is_bypassed = false;

                                    if has_outputs {
                                        node_entry.bypass_declick.fade_to_1(&extra.declick_values);
                                        is_bypass_declicking = true;
                                    } else {
                                        node_entry.bypass_declick = Declicker::SettledAt1;
                                        is_bypass_declicking = false;
                                    }
                                } // else already un-bypassed
                            }
                        }

                        // Set the timing information for the process info for this sub-chunk.
                        info.frames = sub_chunk_frames;
                        info.clock_samples = sub_clock_samples;
                        info.prev_output_was_silent = node_entry.prev_output_was_silent;
                        info.did_just_unbypass = false;

                        // Call the node's process method.
                        let process_status = if node_entry.bypass_declick == Declicker::SettledAt0 {
                            let did_just_bypass = !node_entry.is_bypassed;
                            if did_just_bypass {
                                node_entry.is_bypassed = true;
                                node_entry.processor.bypassed(true);
                            }

                            if !events.is_empty() || node_entry.is_first_process {
                                node_entry.processor.events(info, events, extra);
                                node_entry.is_first_process = false;
                            }

                            ProcessStatus::Bypass
                        } else {
                            let did_just_unbypass = node_entry.is_bypassed;
                            if did_just_unbypass {
                                node_entry.is_bypassed = false;
                                info.did_just_unbypass = true;
                                node_entry.processor.bypassed(false);
                            }

                            if !events.is_empty() || node_entry.is_first_process {
                                node_entry.processor.events(info, events, extra);
                                node_entry.is_first_process = false;
                            }

                            if is_bypass_declicking {
                                let mut tmp_buffers = bypass_declick_buffer
                                    .channels_mut::<MAX_CHANNELS>(
                                        proc_buffers.outputs.len(),
                                        sub_chunk_frames,
                                    );

                                if node_entry.in_place_buffers {
                                    for (out_ch, tmp_ch) in
                                        proc_buffers.outputs.iter().zip(tmp_buffers.iter_mut())
                                    {
                                        tmp_ch[..sub_chunk_frames]
                                            .copy_from_slice(&out_ch[sub_chunk_range.clone()]);
                                    }
                                } else {
                                    for (in_ch, tmp_ch) in
                                        proc_buffers.inputs.iter().zip(tmp_buffers.iter_mut())
                                    {
                                        tmp_ch[..sub_chunk_frames]
                                            .copy_from_slice(&in_ch[sub_chunk_range.clone()]);
                                    }

                                    for tmp_ch in
                                        tmp_buffers.iter_mut().skip(proc_buffers.inputs.len())
                                    {
                                        tmp_ch[..sub_chunk_frames].fill(0.0);
                                    }
                                }
                            }

                            if sub_chunk_frames == block_frames {
                                // If this is the only sub-chunk (because there are no scheduled
                                // events), there is no need to edit the buffer slices.
                                let sub_proc_buffers = ProcBuffers {
                                    inputs: proc_buffers.inputs,
                                    outputs: proc_buffers.outputs,
                                };

                                node_entry.processor.process(info, sub_proc_buffers, extra)
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

                                node_entry.processor.process(info, sub_proc_buffers, extra)
                            }
                        };

                        if is_bypass_declicking {
                            let tmp_buffers = bypass_declick_buffer.channels::<MAX_CHANNELS>(
                                proc_buffers.outputs.len(),
                                sub_chunk_frames,
                            );

                            node_entry.bypass_declick.process_crossfade(
                                &tmp_buffers,
                                proc_buffers.outputs,
                                0..sub_chunk_frames,
                                sub_chunk_range.clone(),
                                &extra.declick_values,
                                DeclickFadeCurve::Linear,
                            );
                        }

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

                #[cfg(feature = "node_profiling")]
                self.profiler_tx.node_completed();

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

        self.profiler_tx.begin_new_bookkeeping_part();

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
    flags: &FirewheelBitFlags,
    shared_flags: &Arc<SharedFlags>,
    logger: &mut RealtimeLogger,
) {
    if flags.contains(FirewheelBitFlags::VALIDATE_OUTPUT_IS_FINITE) {
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

    if flags.contains(FirewheelBitFlags::DETECT_CLIPPING_ON_OUTPUT) {
        let mut clipping_occurred = false;

        for ch in output.iter() {
            let max_peak = firewheel_core::dsp::algo::max_peak(ch);

            if max_peak > 1.0 {
                clipping_occurred = true;
                break;
            }
        }

        if clipping_occurred {
            shared_flags
                .clipping_occurred
                .store(true, Ordering::Relaxed);
        }
    }

    if flags.contains(FirewheelBitFlags::HARD_CLIP_OUTPUTS) {
        for ch in output.iter_mut() {
            for s in ch.iter_mut() {
                *s = s.clamp(-1.0, 1.0);
            }
        }
    }
}
