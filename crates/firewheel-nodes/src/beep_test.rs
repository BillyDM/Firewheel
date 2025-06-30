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

/// A simple node that outputs a sine wave, used for testing purposes.
///
/// Note that because this node is for testing purposes, it does not
/// bother with parameter smoothing.
#[derive(Diff, Patch, Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "bevy", derive(bevy_ecs::prelude::Component))]
pub struct BeepTestNode {
    /// The frequency of the sine wave in the range `[20.0, 20_000.0]`. A good
    /// value for testing is `440` (middle C).
    pub freq_hz: f32,

    /// The overall volume.
    ///
    /// NOTE, a sine wave at `Volume::Linear(1.0) or Volume::Decibels(0.0)` volume
    /// is *LOUD*, prefer to use a value `Volume::Linear(0.5) or
    /// Volume::Decibels(-12.0)`.
    pub volume: Volume,

    /// Whether or not the node is currently enabled.
    pub enabled: bool,
}

impl Default for BeepTestNode {
    fn default() -> Self {
        Self {
            freq_hz: 440.0,
            volume: Volume::Linear(0.5),
            enabled: true,
        }
    }
}

impl AudioNode for BeepTestNode {
    type Configuration = EmptyConfig;

    fn info(&self, _config: &Self::Configuration) -> AudioNodeInfo {
        AudioNodeInfo::new()
            .debug_name("beep_test")
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
            enabled: self.enabled,
        }
    }
}

struct Processor {
    phasor: f32,
    phasor_inc: f32,
    gain: f32,
    enabled: bool,
}

impl AudioNodeProcessor for Processor {
    fn process(
        &mut self,
        buffers: ProcBuffers,
        proc_info: &ProcInfo,
        mut events: NodeEventList,
    ) -> ProcessStatus {
        let Some(out) = buffers.outputs.first_mut() else {
            return ProcessStatus::ClearAllOutputs;
        };

        events.for_each_patch::<BeepTestNode>(|patch| match patch {
            BeepTestNodePatch::FreqHz(f) => {
                self.phasor_inc = f.clamp(20.0, 20_000.0) * proc_info.sample_rate_recip as f32;
            }
            BeepTestNodePatch::Volume(v) => {
                self.gain = v.amp_clamped(DEFAULT_AMP_EPSILON);
            }
            BeepTestNodePatch::Enabled(e) => self.enabled = e,
        });

        if !self.enabled {
            return ProcessStatus::ClearAllOutputs;
        }

        for s in out.iter_mut() {
            *s = (self.phasor * core::f32::consts::TAU).sin() * self.gain;
            self.phasor = (self.phasor + self.phasor_inc).fract();
        }

        ProcessStatus::outputs_not_silent()
    }
}
