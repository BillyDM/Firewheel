use std::f32::consts::FRAC_1_SQRT_2;

use firewheel_core::{
    channel_config::{ChannelConfig, ChannelCount},
    diff::{Diff, Patch},
    dsp::filter::{
        filter_trait::Filter, multi_channel_filter::MultiChannelFilter, primitives::svf::SvfState,
    },
    event::NodeEventList,
    node::{
        AudioNode, AudioNodeInfo, AudioNodeProcessor, ConstructProcessorContext, ProcBuffers,
        ProcInfo, ProcessStatus,
    },
    SilenceMask,
};

#[derive(Default, Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "bevy", derive(bevy_ecs::prelude::Component))]
pub struct AllpassFilterNodeConfig<const NUM_CHANNELS: usize>;

#[derive(Diff, Patch, Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "bevy", derive(bevy_ecs::prelude::Component))]
pub struct AllpassFilterNode<const NUM_CHANNELS: usize> {
    /// The cutoff frequency in Hz
    pub cutoff_hz: f32,
    /// The q factor
    pub q: f32,
}

impl<const NUM_CHANNELS: usize> Default for AllpassFilterNode<NUM_CHANNELS> {
    fn default() -> Self {
        Self {
            cutoff_hz: 440.,
            q: FRAC_1_SQRT_2,
        }
    }
}

impl<const NUM_CHANNELS: usize> AudioNode for AllpassFilterNode<NUM_CHANNELS> {
    type Configuration = AllpassFilterNodeConfig<NUM_CHANNELS>;

    fn info(&self, _config: &Self::Configuration) -> AudioNodeInfo {
        let num_inputs = ChannelCount::new(NUM_CHANNELS as u32).unwrap();
        let num_outputs = num_inputs;
        AudioNodeInfo::new()
            .debug_name("volume")
            .channel_config(ChannelConfig {
                num_inputs,
                num_outputs,
            })
            .uses_events(true)
    }

    fn construct_processor(
        &self,
        _config: &Self::Configuration,
        cx: ConstructProcessorContext,
    ) -> impl AudioNodeProcessor {
        assert!((cx.stream_info.num_stream_in_channels as usize) < NUM_CHANNELS);

        let mut result: AllpassFilterProcessor<NUM_CHANNELS> = AllpassFilterProcessor {
            filter: Default::default(),
            params: Default::default(),
            prev_block_was_silent: true,
        };
        result.design();
        result
    }
}

struct AllpassFilterProcessor<const NUM_CHANNELS: usize> {
    filter: MultiChannelFilter<NUM_CHANNELS, [SvfState; 1]>,
    params: AllpassFilterNode<NUM_CHANNELS>,
    prev_block_was_silent: bool,
}

impl<const NUM_CHANNELS: usize> AllpassFilterProcessor<NUM_CHANNELS> {
    fn design(&mut self) {
        self.filter.allpass(self.params.cutoff_hz, self.params.q);
    }
}

impl<const NUM_CHANNELS: usize> AudioNodeProcessor for AllpassFilterProcessor<NUM_CHANNELS> {
    fn process(
        &mut self,
        buffers: ProcBuffers,
        proc_info: &ProcInfo,
        mut events: NodeEventList,
    ) -> ProcessStatus {
        let mut updated = false;
        events.for_each_patch::<AllpassFilterNode<NUM_CHANNELS>>(|patch| {
            self.params.apply(patch);
            updated = true;
        });
        if updated {
            self.design();
        }

        self.prev_block_was_silent = false;

        if proc_info
            .in_silence_mask
            .all_channels_silent(buffers.inputs.len())
            && self.filter.is_silent()
        {
            self.prev_block_was_silent = true;

            return ProcessStatus::ClearAllOutputs;
        }

        let mut output_silence_mask = SilenceMask::new_all_silent(buffers.inputs.len());
        for (ch_i, (out_ch, in_ch)) in buffers
            .outputs
            .iter_mut()
            .zip(buffers.inputs.iter())
            .enumerate()
        {
            if proc_info.in_silence_mask.is_channel_silent(ch_i)
                && self.filter.filters[ch_i].is_silent()
            {
                if !proc_info.out_silence_mask.is_channel_silent(ch_i) {
                    out_ch.fill(0.0);
                }
            } else {
                output_silence_mask.set_channel(ch_i, false);
                for (os, &is) in out_ch.iter_mut().zip(in_ch.iter()) {
                    *os = self.filter.process(is, ch_i);
                }
            }
        }

        return ProcessStatus::OutputsModified {
            out_silence_mask: output_silence_mask,
        };
    }

    fn new_stream(&mut self, stream_info: &firewheel_core::StreamInfo) {
        self.filter.sample_rate_recip = stream_info.sample_rate_recip as f32;
    }
}
