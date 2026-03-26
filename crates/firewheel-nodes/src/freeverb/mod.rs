//! A Rust implementation of Freeverb by Ian Hobson.
//! The original repo can be found [here](https://github.com/irh/freeverb-rs).

#![allow(missing_docs)]
#![allow(clippy::module_inception)]

use firewheel_core::dsp::coeff_update::{CoeffUpdateFactor, CoeffUpdateMask};
use firewheel_core::node::NodeError;
use firewheel_core::{
    channel_config::{ChannelConfig, ChannelCount},
    diff::{Diff, Notify, Patch},
    dsp::{
        declick::{DeclickFadeCurve, DeclickValues, Declicker},
        volume::DEFAULT_MIN_AMP,
    },
    event::ProcEvents,
    node::{
        AudioNode, AudioNodeInfo, AudioNodeProcessor, ConstructProcessorContext, EmptyConfig,
        ProcBuffers, ProcExtra, ProcInfo, ProcStreamCtx, ProcessStatus,
    },
    param::smoother::{SmoothedParam, SmootherConfig},
};

use crate::freeverb::freeverb::Freeverb;

mod all_pass;
mod comb;
mod delay_line;
mod freeverb;

/// A simple, relatively cheap stereo reverb.
///
/// Freeverb tends to have a somewhat metallic sound, but
/// its minimal computational cost makes it highly versatile.
#[derive(Diff, Patch, Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "bevy", derive(bevy_ecs::component::Component))]
#[cfg_attr(feature = "bevy_reflect", derive(bevy_reflect::Reflect))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct FreeverbNode {
    /// Set the size of the emulated room, expressed from 0 to 1.
    ///
    /// Values near zero will sound like a small room, while values
    /// near one will reverberate almost continuously.
    pub room_size: f32,

    /// Set the high-frequency damping, expressed from 0 to 1.
    ///
    /// Values near zero will produce a dark or muffled sound,
    /// while values near one will sound bright or metallic.
    pub damping: f32,

    /// Set the left/right blending, expressed from 0 to 1.
    pub width: f32,

    /// Pause the reverb processing.
    ///
    /// This prevents a reverb tail from ringing out when you
    /// want all sound to momentarily pause.
    pub pause: bool,

    /// Reset the reverb, clearing its internal state.
    #[cfg_attr(feature = "serde", serde(skip))]
    pub reset: Notify<()>,

    /// Adjusts the time in seconds over which parameters are smoothed.
    ///
    /// By default this is set to `0.023` (23ms). This value is chosen to be
    /// roughly equal to a typical block size of 1024 samples (23 ms) to
    /// eliminate stair-stepping for most games.
    pub smooth_seconds: f32,

    /// An exponent representing the rate at which DSP coefficients are
    /// updated when parameters are being smoothed.
    ///
    /// Smaller values will produce less "stair-stepping" artifacts,
    /// but will also consume more CPU.
    ///
    /// The resulting number of frames (samples in a single channel of audio)
    /// that will elapse between each update is calculated as
    /// `2^coeff_update_factor`.
    ///
    /// By default this is set to `4`.
    pub coeff_update_factor: CoeffUpdateFactor,
}

impl Default for FreeverbNode {
    fn default() -> Self {
        FreeverbNode {
            room_size: 0.5,
            damping: 0.5,
            width: 0.5,
            pause: false,
            reset: Notify::new(()),
            smooth_seconds: 0.015,
            coeff_update_factor: CoeffUpdateFactor::default(),
        }
    }
}

impl AudioNode for FreeverbNode {
    type Configuration = EmptyConfig;

    fn info(&self, _: &Self::Configuration) -> Result<AudioNodeInfo, NodeError> {
        Ok(AudioNodeInfo::new()
            .debug_name("freeverb")
            .channel_config(ChannelConfig {
                num_inputs: ChannelCount::STEREO,
                num_outputs: ChannelCount::STEREO,
            }))
    }

    fn construct_processor(
        &self,
        _: &Self::Configuration,
        cx: ConstructProcessorContext,
    ) -> Result<impl AudioNodeProcessor, NodeError> {
        let freeverb = freeverb::Freeverb::new(cx.stream_info.sample_rate.get() as usize);
        let smoother_config = SmootherConfig {
            smooth_seconds: self.smooth_seconds,
            ..Default::default()
        };

        let mut processor = FreeverbProcessor {
            freeverb,
            damping: SmoothedParam::new(
                self.damping.clamp(0.0, 1.0),
                smoother_config,
                cx.stream_info.sample_rate,
            ),
            width: SmoothedParam::new(
                self.width.clamp(0.0, 1.0),
                smoother_config,
                cx.stream_info.sample_rate,
            ),
            room_size: SmoothedParam::new(
                self.room_size.clamp(0.0, 1.0),
                smoother_config,
                cx.stream_info.sample_rate,
            ),
            paused: self.pause,
            pause_declicker: if self.pause {
                Declicker::SettledAt0
            } else {
                Declicker::SettledAt1
            },
            values: DeclickValues::new(cx.stream_info.declick_frames),
            coeff_update_mask: self.coeff_update_factor.mask(),
        };

        processor.apply_parameters();

        Ok(processor)
    }
}

struct FreeverbProcessor {
    freeverb: freeverb::Freeverb,
    damping: SmoothedParam,
    width: SmoothedParam,
    room_size: SmoothedParam,
    paused: bool,
    pause_declicker: Declicker,
    values: DeclickValues,
    coeff_update_mask: CoeffUpdateMask,
}

impl FreeverbProcessor {
    fn reset(&mut self, reset_reverb: bool) {
        self.pause_declicker.reset_to_target();
        self.damping.reset_to_target();
        self.room_size.reset_to_target();
        self.width.reset_to_target();

        if reset_reverb {
            self.freeverb.reset();
        }
    }
}

impl AudioNodeProcessor for FreeverbProcessor {
    fn events(&mut self, info: &ProcInfo, events: &mut ProcEvents, _extra: &mut ProcExtra) {
        for patch in events.drain_patches::<FreeverbNode>() {
            match patch {
                FreeverbNodePatch::Damping(value) => {
                    self.damping.set_value(value.clamp(0.0, 1.0));
                }
                FreeverbNodePatch::RoomSize(value) => {
                    self.room_size.set_value(value.clamp(0.0, 1.0));
                }
                FreeverbNodePatch::Width(value) => {
                    self.width.set_value(value.clamp(0.0, 1.0));
                }
                FreeverbNodePatch::Reset(_) => {
                    self.freeverb.reset();
                }
                FreeverbNodePatch::Pause(value) => {
                    self.paused = value;

                    if value {
                        self.pause_declicker.fade_to_0(&self.values);
                    } else {
                        self.apply_parameters();
                        self.pause_declicker.fade_to_1(&self.values);
                    }
                }
                FreeverbNodePatch::SmoothSeconds(value) => {
                    self.room_size.set_smooth_seconds(value, info.sample_rate);
                    self.width.set_smooth_seconds(value, info.sample_rate);
                    self.damping.set_smooth_seconds(value, info.sample_rate);
                }
                FreeverbNodePatch::CoeffUpdateFactor(value) => {
                    self.coeff_update_mask = value.mask();
                }
            }
        }
    }

    fn bypassed(&mut self, bypassed: bool) {
        if !bypassed {
            self.reset(true);
        }
    }

    fn process(
        &mut self,
        info: &ProcInfo,
        buffers: ProcBuffers,
        _: &mut ProcExtra,
    ) -> ProcessStatus {
        let all_silent = info.in_silence_mask.all_channels_silent(2);

        if (self.paused && self.pause_declicker.has_settled())
            || (all_silent && info.prev_output_was_silent)
        {
            self.reset(false);

            return ProcessStatus::ClearAllOutputs;
        }

        if !all_silent && info.prev_output_was_silent {
            // re-apply the parameters
            self.apply_parameters();
        }

        assert!(buffers.inputs[0].len() >= info.frames);
        assert!(buffers.inputs[1].len() >= info.frames);
        assert!(buffers.outputs[0].len() >= info.frames);
        assert!(buffers.outputs[1].len() >= info.frames);

        // just take the slow path if any are smoothing
        if self.damping.is_smoothing() || self.room_size.is_smoothing() || self.width.is_smoothing()
        {
            for frame in 0..info.frames {
                let damping = self.damping.next_smoothed();
                let room_size = self.room_size.next_smoothed();
                let width = self.width.next_smoothed();

                // we assume setting these values is more expensive than
                // calculating their smoothing
                if self.coeff_update_mask.do_update(frame) {
                    calc_coeffs(&mut self.freeverb, damping, room_size, width);
                }

                let (left, right) = self.freeverb.tick((
                    buffers.inputs[0][frame] as f64,
                    buffers.inputs[1][frame] as f64,
                ));

                buffers.outputs[0][frame] = left as f32;
                buffers.outputs[1][frame] = right as f32;
            }

            self.damping.settle();
            self.room_size.settle();
            self.width.settle();
        } else {
            for frame in 0..info.frames {
                let (left, right) = self.freeverb.tick((
                    buffers.inputs[0][frame] as f64,
                    buffers.inputs[1][frame] as f64,
                ));

                buffers.outputs[0][frame] = left as f32;
                buffers.outputs[1][frame] = right as f32;
            }
        }

        // We do this before the declicking just to make sure we
        // finish declicking if we're paused simultaneously with the
        // input going silent.
        if all_silent && !info.prev_output_was_silent {
            // check the output buffers to see if they pass
            // the threshold for "completely silent"

            if matches!(
                buffers.check_for_silence_on_outputs(DEFAULT_MIN_AMP),
                ProcessStatus::ClearAllOutputs
            ) {
                return ProcessStatus::ClearAllOutputs;
            }
        }

        if !self.pause_declicker.has_settled() {
            self.pause_declicker.process(
                &mut buffers.outputs[..2],
                0..info.frames,
                &self.values,
                1.0,
                DeclickFadeCurve::EqualPower3dB,
            );
        }

        ProcessStatus::OutputsModified
    }

    fn new_stream(&mut self, stream_info: &firewheel_core::StreamInfo, _proc: &mut ProcStreamCtx) {
        self.freeverb.resize(stream_info.sample_rate.get() as usize);
        self.damping.update_sample_rate(stream_info.sample_rate);
        self.width.update_sample_rate(stream_info.sample_rate);
        self.room_size.update_sample_rate(stream_info.sample_rate);
        self.reset(true);
    }
}

impl FreeverbProcessor {
    fn apply_parameters(&mut self) {
        self.freeverb
            .set_dampening(self.damping.target_value() as f64);
        self.freeverb
            .set_room_size(self.room_size.target_value() as f64);
        self.freeverb.set_width(self.width.target_value() as f64);
        self.freeverb.update_combs();
    }
}

#[cold]
#[inline(never)]
fn calc_coeffs(freeverb: &mut Freeverb, damping: f32, room_size: f32, width: f32) {
    freeverb.set_dampening(damping as f64);
    freeverb.set_room_size(room_size as f64);
    freeverb.set_width(width as f64);

    freeverb.update_combs();
}
