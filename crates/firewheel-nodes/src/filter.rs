use firewheel_core::{
    channel_config::{ChannelConfig, ChannelCount},
    diff::{Diff, Patch},
    dsp::declick::{Declicker, FadeType},
    event::NodeEventList,
    node::{
        AudioNode, AudioNodeInfo, AudioNodeProcessor, ConstructProcessorContext, EmptyConfig,
        ProcBuffers, ProcInfo, ProcessStatus,
    },
    param::smoother::SmoothedParam,
    SilenceMask, StreamInfo,
};

use std::f32::consts::PI;

/// Filter type enumeration
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterType {
    LowPass = 0,
    HighPass = 1,
    BandPass = 2,
}

impl From<u32> for FilterType {
    fn from(value: u32) -> Self {
        match value {
            0 => FilterType::LowPass,
            1 => FilterType::HighPass,
            2 => FilterType::BandPass,
            _ => FilterType::LowPass, // Default fallback
        }
    }
}

impl From<FilterType> for u32 {
    fn from(filter_type: FilterType) -> Self {
        filter_type as u32
    }
}

/// A multi-purpose filter node that can apply low-pass, high-pass, or band-pass filtering
/// to a stereo signal with configurable wet/dry mixing.
#[derive(Diff, Patch, Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "bevy", derive(bevy_ecs::prelude::Component))]
pub struct FilterNode {
    /// The cutoff frequency in hertz in the range `[20.0, 20_000.0]`.
    pub cutoff_hz: f32,
    /// Filter type (0 = LowPass, 1 = HighPass, 2 = BandPass)
    pub filter_type: u32,
    /// Wet/dry mix (0.0 = fully dry, 1.0 = fully wet)
    pub mix: f32,
    /// Whether or not this node is enabled.
    pub enabled: bool,
}

impl Default for FilterNode {
    fn default() -> Self {
        Self {
            cutoff_hz: 1000.0,
            filter_type: FilterType::LowPass as u32,
            mix: 1.0,
            enabled: true,
        }
    }
}

impl AudioNode for FilterNode {
    type Configuration = EmptyConfig;

    fn info(&self, _config: &Self::Configuration) -> AudioNodeInfo {
        AudioNodeInfo::new()
            .debug_name("multi_filter")
            .channel_config(ChannelConfig {
                num_inputs: ChannelCount::STEREO,
                num_outputs: ChannelCount::STEREO,
            })
            .uses_events(true)
    }
    
    fn construct_processor(
        &self,
        _config: &Self::Configuration,
        cx: ConstructProcessorContext,
    ) -> impl AudioNodeProcessor {
        let sample_rate_recip = cx.stream_info.sample_rate_recip as f32;
        
        let cutoff_hz = self.cutoff_hz.clamp(20.0, 20_000.0);

        Processor {
            filter_l: OnePoleFilter::new(cutoff_hz, sample_rate_recip, FilterType::from(self.filter_type)),
            filter_r: OnePoleFilter::new(cutoff_hz, sample_rate_recip, FilterType::from(self.filter_type)),
            cutoff_hz: SmoothedParam::new(
                cutoff_hz,
                Default::default(),
                cx.stream_info.sample_rate,
            ),
            mix: SmoothedParam::new(
                self.mix.clamp(0.0, 1.0),
                Default::default(),
                cx.stream_info.sample_rate,
            ),
            filter_type: FilterType::from(self.filter_type),
            enable_declicker: Declicker::from_enabled(self.enabled),
            sample_rate_recip,
        }
    }
}

struct Processor {
    filter_l: OnePoleFilter,
    filter_r: OnePoleFilter,
    cutoff_hz: SmoothedParam,
    mix: SmoothedParam,
    filter_type: FilterType,
    enable_declicker: Declicker,
    sample_rate_recip: f32,
}

impl AudioNodeProcessor for Processor {
    fn process(
        &mut self,
        buffers: ProcBuffers,
        proc_info: &ProcInfo,
        mut events: NodeEventList,
    ) -> ProcessStatus {
        events.for_each_patch::<FilterNode>(|patch| match patch {
            FilterNodePatch::CutoffHz(cutoff) => {
                self.cutoff_hz.set_value(cutoff.clamp(20.0, 20_000.0));
            }
            FilterNodePatch::FilterType(filter_type) => {
                self.filter_type = FilterType::from(filter_type);
                self.filter_l.set_filter_type(self.filter_type);
                self.filter_r.set_filter_type(self.filter_type);
            }
            FilterNodePatch::Mix(mix) => {
                self.mix.set_value(mix.clamp(0.0, 1.0));
            }
            FilterNodePatch::Enabled(enabled) => {
                self.enable_declicker
                    .fade_to_enabled(enabled, proc_info.declick_values);
            }
        });

        if self.enable_declicker.disabled() {
            return ProcessStatus::Bypass;
        }

        if proc_info.in_silence_mask.all_channels_silent(2) {

            self.cutoff_hz.reset();
            self.mix.reset();
            self.filter_l.reset();
            self.filter_r.reset();
            self.enable_declicker.reset_to_target();

            return ProcessStatus::ClearAllOutputs;
        }
        
        let in1 = &buffers.inputs[0][..proc_info.frames];
        let in2 = &buffers.inputs[1][..proc_info.frames];
        let (out1, out2) = buffers.outputs.split_first_mut().unwrap();
        let out1 = &mut out1[..proc_info.frames];
        let out2 = &mut out2[0][..proc_info.frames];

        if self.cutoff_hz.is_smoothing() || self.mix.is_smoothing() {
            for i in 0..proc_info.frames {
                let cutoff_hz = self.cutoff_hz.next_smoothed();
                let mix = self.mix.next_smoothed();

                if i & (16 - 1) == 0 {
                    self.filter_l.set_cutoff(cutoff_hz, self.sample_rate_recip);
                    self.filter_r.copy_cutoff_from(&self.filter_l);
                }

                let fl = self.filter_l.process(in1[i]);
                let fr = self.filter_r.process(in2[i]);

                out1[i] = in1[i] * (1.0 - mix) + fl * mix;
                out2[i] = in2[i] * (1.0 - mix) + fr * mix;
            }

            self.cutoff_hz.settle();
            self.mix.settle();
        } else {
            let mix = self.mix.target_value();

            self.filter_l
                .set_cutoff(self.cutoff_hz.target_value(), self.sample_rate_recip);
            self.filter_r.copy_cutoff_from(&self.filter_l);

            if mix == 0.0 {
                return ProcessStatus::Bypass;
            } else if mix == 1.0 {
                for i in 0..proc_info.frames {
                    let fl = self.filter_l.process(in1[i]);
                    let fr = self.filter_r.process(in2[i]);

                    out1[i] = fl;
                    out2[i] = fr;
                }
            } else {
                for i in 0..proc_info.frames {
                    let fl = self.filter_l.process(in1[i]);
                    let fr = self.filter_r.process(in2[i]);

                    out1[i] = in1[i] * (1.0 - mix) + fl * mix;
                    out2[i] = in2[i] * (1.0 - mix) + fr * mix;
                }
            }
        }

        self.enable_declicker.process_crossfade(
            buffers.inputs,
            buffers.outputs,
            proc_info.frames,
            proc_info.declick_values,
            FadeType::EqualPower3dB,
        );

        ProcessStatus::OutputsModified {
            out_silence_mask: SilenceMask::NONE_SILENT,
        }
    }
    
    fn new_stream(&mut self, stream_info: &StreamInfo) {
        self.sample_rate_recip = stream_info.sample_rate_recip as f32;

        self.cutoff_hz.update_sample_rate(stream_info.sample_rate);
        self.mix.update_sample_rate(stream_info.sample_rate);

        self.filter_l
            .set_cutoff(self.cutoff_hz.target_value(), self.sample_rate_recip);
        self.filter_r.copy_cutoff_from(&self.filter_l);
    }
}

#[derive(Debug, Clone, Copy)]
struct OnePoleFilter {
    // Low-pass state
    z1: f32,
    // High-pass state  
    x1: f32,
    y1: f32,
    // Band-pass state
    bp_z1: f32,
    bp_z2: f32,
    // Coefficients
    a0: f32,
    b1: f32,
    filter_type: FilterType,
}

impl OnePoleFilter {
    pub fn new(cutoff_hz: f32, sample_rate_recip: f32, filter_type: FilterType) -> Self {
        let mut new_self = Self {
            z1: 0.0,
            x1: 0.0,
            y1: 0.0,
            bp_z1: 0.0,
            bp_z2: 0.0,
            a0: 0.0,
            b1: 0.0,
            filter_type,
        };

        new_self.set_cutoff(cutoff_hz, sample_rate_recip);

        new_self
    }

    pub fn reset(&mut self) {
        self.z1 = 0.0;
        self.x1 = 0.0;
        self.y1 = 0.0;
        self.bp_z1 = 0.0;
        self.bp_z2 = 0.0;
    }

    pub fn set_filter_type(&mut self, filter_type: FilterType) {
        if self.filter_type != filter_type {
            self.filter_type = filter_type;
            self.reset();
        }
    }

    #[inline]
    pub fn set_cutoff(&mut self, cutoff_hz: f32, sample_rate_recip: f32) {
        match self.filter_type {
            FilterType::LowPass | FilterType::HighPass => {
                self.b1 = (-2.0 * PI * cutoff_hz * sample_rate_recip).exp();
                self.a0 = 1.0 - self.b1;
            }
            FilterType::BandPass => {
                let omega = 2.0 * PI * cutoff_hz * sample_rate_recip;
                self.a0 = omega / (1.0 + omega);
                self.b1 = (1.0 - omega) / (1.0 + omega);
            }
        }
    }

    #[inline]
    pub fn copy_cutoff_from(&mut self, other: &Self) {
        self.a0 = other.a0;
        self.b1 = other.b1;
    }

    #[inline]
    pub fn process(&mut self, s: f32) -> f32 {
        match self.filter_type {
            FilterType::LowPass => {
                self.z1 = (self.a0 * s) + (self.b1 * self.z1);
                self.z1
            }
            FilterType::HighPass => {
                let output = self.a0 * (s - self.x1) + self.b1 * self.y1;
                self.x1 = s;
                self.y1 = output;
                output
            }
            FilterType::BandPass => {
                let temp = s - self.bp_z2;
                let output = self.a0 * temp + self.bp_z1;
                self.bp_z2 = self.bp_z2 + self.a0 * temp;
                self.bp_z1 = output;
                output
            }
        }
    }
}