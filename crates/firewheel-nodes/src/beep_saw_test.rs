use firewheel_core::{
    channel_config::{ChannelConfig, ChannelCount},
    diff::{Diff, Patch},
    dsp::volume::{Volume, DEFAULT_AMP_EPSILON},
    event::NodeEventList,
    node::{
        AudioNode, AudioNodeInfo, AudioNodeProcessor, ConstructProcessorContext, EmptyConfig,
        ProcBuffers, ProcInfo, ProcessStatus,
    },
};

/// A simple node that outputs a saw wave, used for testing purposes.
///
/// Note that because this node is for testing purposes, it does not
/// bother with parameter smoothing.
/// This was mostly copied from the beep_test node. Comments were removed for brevity.
#[derive(Diff, Patch, Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "bevy", derive(bevy_ecs::prelude::Component))]
pub struct BeepSawTestNode {
    pub freq_hz: f32,
    pub volume: Volume,
    pub enabled: bool,
}

impl Default for BeepSawTestNode {
    fn default() -> Self {
        Self {
            freq_hz: 440.0,
            volume: Volume::Linear(0.5),
            enabled: true,
        }
    }
}

impl AudioNode for BeepSawTestNode {
    type Configuration = EmptyConfig;

    fn info(&self, _config: &Self::Configuration) -> AudioNodeInfo {
        AudioNodeInfo::new()
            .debug_name("beep_saw_test")
            .channel_config(ChannelConfig {
                num_inputs: ChannelCount::ZERO,
                num_outputs: ChannelCount::MONO,
            })
            .uses_events(true)
    }

    fn construct_processor(
        &self,
        _config: &Self::Configuration,
        cx: ConstructProcessorContext,
    ) -> impl AudioNodeProcessor {
        Processor {
            phasor: 0.0,
            phasor_inc: self.freq_hz.clamp(20.0, 20_000.0)
                * cx.stream_info.sample_rate_recip as f32,
            gain: self.volume.amp_clamped(DEFAULT_AMP_EPSILON),
            sample_rate_recip: (cx.stream_info.sample_rate.get() as f32).recip(),
            enabled: self.enabled,
        }
    }
}

struct Processor {
    phasor: f32,
    phasor_inc: f32,
    gain: f32,
    sample_rate_recip: f32,
    enabled: bool,
}

impl AudioNodeProcessor for Processor {
    fn process(
        &mut self,
        buffers: ProcBuffers,
        _proc_info: &ProcInfo,
        mut events: NodeEventList,
    ) -> ProcessStatus {
        let Some(out) = buffers.outputs.first_mut() else {
            return ProcessStatus::ClearAllOutputs;
        };

        events.for_each_patch::<BeepSawTestNode>(|patch| match patch {
            BeepSawTestNodePatch::FreqHz(f) => {
                self.phasor_inc = f.clamp(20.0, 20_000.0) * self.sample_rate_recip;
            }
            BeepSawTestNodePatch::Volume(v) => {
                self.gain = v.amp_clamped(DEFAULT_AMP_EPSILON);
            }
            BeepSawTestNodePatch::Enabled(e) => self.enabled = e,
        });

        if !self.enabled {
            return ProcessStatus::ClearAllOutputs;
        }

        for s in out.iter_mut() {
            // replaced sine wave with saw wave
            *s = (self.phasor * 2. - 1.) * self.gain;
            self.phasor = (self.phasor + self.phasor_inc).fract();
        }

        ProcessStatus::outputs_not_silent()
    }
}
