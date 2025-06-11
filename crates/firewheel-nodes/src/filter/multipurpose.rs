use firewheel_core::{
    channel_config::{ChannelConfig, ChannelCount},
    diff::{Diff, Patch},
    dsp::filter::{
        cascade::FilterCascadeUpTo, filter_trait::Filter, multi_channel_filter::MultiChannelFilter,
    },
    event::NodeEventList,
    node::{
        AudioNode, AudioNodeInfo, AudioNodeProcessor, ConstructProcessorContext, ProcBuffers,
        ProcInfo, ProcessStatus,
    },
    SilenceMask,
};

use crate::filter::{
    allpass::AllpassFilterNode, bell::BellFilterNode, high_shelf::HighShelfFilterNode,
    highpass::HighpassFilterNode, low_shelf::LowShelfFilterNode, lowpass::LowpassFilterNode,
    notch::NotchFilterNode,
};

#[derive(Default, Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "bevy", derive(bevy_ecs::prelude::Component))]
pub struct MultipurposeFilterNodeConfig<const NUM_CHANNELS: usize>;

#[derive(Default, Diff, Patch, Debug, Clone, Copy, PartialEq)]
pub enum FilterType {
    #[default]
    Lowpass,
    Highpass,
    Notch,
    Bell,
    LowShelf,
    HighShelf,
    Allpass,
}

#[derive(Default, Diff, Patch, Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "bevy", derive(bevy_ecs::prelude::Component))]
pub struct MultipurposeFilterNode<const NUM_CHANNELS: usize, const MAX_ORDER: usize> {
    pub filter_type: FilterType,
    pub lowpass: LowpassFilterNode<NUM_CHANNELS, MAX_ORDER>,
    pub highpass: HighpassFilterNode<NUM_CHANNELS, MAX_ORDER>,
    pub notch: NotchFilterNode<NUM_CHANNELS>,
    pub bell: BellFilterNode<NUM_CHANNELS>,
    pub low_shelf: LowShelfFilterNode<NUM_CHANNELS>,
    pub high_shelf: HighShelfFilterNode<NUM_CHANNELS>,
    pub allpass: AllpassFilterNode<NUM_CHANNELS>,
}

impl<const NUM_CHANNELS: usize, const MAX_ORDER: usize> AudioNode
    for MultipurposeFilterNode<NUM_CHANNELS, MAX_ORDER>
{
    type Configuration = MultipurposeFilterNodeConfig<NUM_CHANNELS>;

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

        let mut result: MultipurposeFilterProcessor<NUM_CHANNELS, MAX_ORDER> =
            MultipurposeFilterProcessor {
                filter: Default::default(),
                params: Default::default(),
                prev_block_was_silent: true,
            };
        result.design();
        result
    }
}

struct MultipurposeFilterProcessor<const NUM_CHANNELS: usize, const MAX_ORDER: usize> {
    filter: MultiChannelFilter<NUM_CHANNELS, FilterCascadeUpTo<MAX_ORDER>>,
    params: MultipurposeFilterNode<NUM_CHANNELS, MAX_ORDER>,
    prev_block_was_silent: bool,
}

impl<const NUM_CHANNELS: usize, const MAX_ORDER: usize>
    MultipurposeFilterProcessor<NUM_CHANNELS, MAX_ORDER>
{
    fn design(&mut self) {
        match self.params.filter_type {
            FilterType::Lowpass => self.filter.lowpass(
                self.params.lowpass.order as usize,
                self.params.lowpass.cutoff_hz,
                self.params.lowpass.q,
            ),
            FilterType::Highpass => self.filter.highpass(
                self.params.highpass.order as usize,
                self.params.highpass.cutoff_hz,
                self.params.highpass.q,
            ),
            FilterType::Notch => self
                .filter
                .notch(self.params.notch.center_hz, self.params.notch.q),
            FilterType::Bell => self.filter.bell(
                self.params.bell.center_hz,
                self.params.bell.q,
                self.params.bell.gain_db,
            ),
            FilterType::LowShelf => self.filter.low_shelf(
                self.params.low_shelf.cutoff_hz,
                self.params.low_shelf.q,
                self.params.low_shelf.gain_db,
            ),
            FilterType::HighShelf => self.filter.high_shelf(
                self.params.high_shelf.cutoff_hz,
                self.params.high_shelf.q,
                self.params.high_shelf.gain_db,
            ),
            FilterType::Allpass => self
                .filter
                .allpass(self.params.allpass.cutoff_hz, self.params.allpass.q),
        }
    }
}

impl<const NUM_CHANNELS: usize, const MAX_ORDER: usize> AudioNodeProcessor
    for MultipurposeFilterProcessor<NUM_CHANNELS, MAX_ORDER>
{
    fn process(
        &mut self,
        buffers: ProcBuffers,
        proc_info: &ProcInfo,
        mut events: NodeEventList,
    ) -> ProcessStatus {
        let mut updated = false;
        events.for_each_patch::<MultipurposeFilterNode<NUM_CHANNELS, MAX_ORDER>>(|patch| {
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
