//! A 3D spatial positioning node using a basic (and naive) algorithm. (It can also
//! be used for 2D audio.) It does not make use of any fancy binaural algorithms,
//! rather it just applies basic panning and filtering.

#[cfg(not(feature = "std"))]
use num_traits::Float;

use firewheel_core::{
    channel_config::{ChannelConfig, ChannelCount},
    diff::{Diff, Patch},
    dsp::{
        coeff_update::CoeffUpdateFactor,
        distance_attenuation::{
            DistanceAttenuation, DistanceAttenuatorStereoDsp, MUFFLE_CUTOFF_HZ_MAX,
        },
        fade::FadeCurve,
        filter::smoothing_filter::DEFAULT_SMOOTH_SECONDS,
        volume::Volume,
    },
    event::ProcEvents,
    mask::ConnectedMask,
    node::{
        AudioNode, AudioNodeInfo, AudioNodeProcessor, ConstructProcessorContext, EmptyConfig,
        ProcBuffers, ProcExtra, ProcInfo, ProcStreamCtx, ProcessStatus,
    },
    param::smoother::{SmoothedParam, SmootherConfig},
    vector::Vec3,
};

/// A 3D spatial positioning node using a basic but fast algorithm. (It can also be used
/// for 2D audio). It does not make use of any fancy binaural algorithms, rather it just
/// applies basic panning and filtering.
#[derive(Diff, Patch, Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "bevy", derive(bevy_ecs::prelude::Component))]
#[cfg_attr(feature = "bevy_reflect", derive(bevy_reflect::Reflect))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SpatialBasicNode {
    /// The overall volume. This is applied before the spatialization algorithm.
    pub volume: Volume,

    /// A 3D vector representing the offset between the listener and the
    /// sound source.
    ///
    /// The coordinates are `(x, y, z)`. (This node can also be used for 2D audio by
    /// setting the z value to `0.0`.)
    ///
    /// * `-x` is to the left of the listener, and `+x` is to the right of the listener
    /// * Larger absolute `y` and `z` values will make the signal sound farther away.
    /// (The algorithm used by this node makes no distinction between `-y`, `+y`, `-z`,
    /// and `+z`).
    ///
    /// By default this is set to `(0.0, 0.0, 0.0)`
    pub offset: Vec3,

    /// The threshold for the maximum amount of panning that can occur, in the range
    /// `[0.0, 1.0]`, where `0.0` is no panning and `1.0` is full panning (where one
    /// of the channels is fully silent when panned hard left or right).
    ///
    /// Setting this to a value less than `1.0` can help remove some of the
    /// jarringness of having a sound playing in only one ear.
    ///
    /// By default this is set to `0.6`.
    pub panning_threshold: f32,

    /// If `true`, then any stereo input signals will be downmixed to mono before
    /// going throught the spatialization algorithm. If `false` then the left and
    /// right channels will be processed independently.
    ///
    /// This has no effect if only one input channel is connected.
    ///
    /// By default this is set to `true`.
    pub downmix: bool,

    /// The amount of muffling (lowpass) in the range `[20.0, 20_480.0]`,
    /// where `20_480.0` is no muffling and `20.0` is maximum muffling.
    ///
    /// This can be used to give the effect of a sound being played behind a wall
    /// or underwater.
    ///
    /// By default this is set to `20_480.0`.
    ///
    /// See <https://www.desmos.com/calculator/jxp8t9ero4> for an interactive graph of
    /// how these parameters affect the final lowpass cuttoff frequency.
    pub muffle_cutoff_hz: f32,

    /// The parameters which describe how to attenuate a sound based on its distance from
    /// the listener.
    pub distance_attenuation: DistanceAttenuation,

    /// The time in seconds of the internal smoothing filter.
    ///
    /// By default this is set to `0.015` (15ms).
    pub smooth_seconds: f32,
    /// If the resutling gain (in raw amplitude, not decibels) is less than or equal
    /// to this value, the the gain will be clamped to `0` (silence).
    ///
    /// By default this is set to "0.0001" (-80 dB).
    pub min_gain: f32,
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
    /// By default this is set to `5`.
    pub coeff_update_factor: CoeffUpdateFactor,
}

impl Default for SpatialBasicNode {
    fn default() -> Self {
        Self {
            volume: Volume::default(),
            offset: Vec3::new(0.0, 0.0, 0.0),
            panning_threshold: 0.6,
            downmix: true,
            distance_attenuation: DistanceAttenuation::default(),
            muffle_cutoff_hz: MUFFLE_CUTOFF_HZ_MAX,
            smooth_seconds: DEFAULT_SMOOTH_SECONDS,
            min_gain: 0.0001,
            coeff_update_factor: CoeffUpdateFactor::default(),
        }
    }
}

impl SpatialBasicNode {
    pub fn from_volume_offset(volume: Volume, offset: impl Into<Vec3>) -> Self {
        Self {
            volume,
            offset: offset.into(),
            ..Default::default()
        }
    }

    /// Set the given volume in a linear scale, where `0.0` is silence and
    /// `1.0` is unity gain.
    ///
    /// These units are suitable for volume sliders (simply convert percent
    /// volume to linear volume by diving the percent volume by 100).
    pub const fn set_volume_linear(&mut self, linear: f32) {
        self.volume = Volume::Linear(linear);
    }

    /// Set the given volume in percentage, where `0.0` is silence and
    /// `100.0` is unity gain.
    ///
    /// These units are suitable for volume sliders.
    pub const fn set_volume_percent(&mut self, percent: f32) {
        self.volume = Volume::from_percent(percent);
    }

    /// Set the given volume in decibels, where `0.0` is unity gain and
    /// `f32::NEG_INFINITY` is silence.
    pub const fn set_volume_decibels(&mut self, decibels: f32) {
        self.volume = Volume::Decibels(decibels);
    }

    fn compute_values(&self) -> ComputedValues {
        let x2_z2 = (self.offset.x * self.offset.x) + (self.offset.z * self.offset.z);
        let xz_distance = x2_z2.sqrt();
        let distance = (x2_z2 + (self.offset.y * self.offset.y)).sqrt();

        let pan = if xz_distance > 0.0 {
            (self.offset.x / xz_distance) * self.panning_threshold.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let (pan_gain_l, pan_gain_r) = FadeCurve::EqualPower3dB.compute_gains_neg1_to_1(pan);

        let mut volume_gain = self.volume.amp();
        if volume_gain > 0.99999 && volume_gain < 1.00001 {
            volume_gain = 1.0;
        }

        let mut gain_l = pan_gain_l * volume_gain;
        let mut gain_r = pan_gain_r * volume_gain;

        if gain_l <= self.min_gain {
            gain_l = 0.0;
        }
        if gain_r <= self.min_gain {
            gain_r = 0.0;
        }

        ComputedValues {
            distance,
            gain_l,
            gain_r,
        }
    }
}

struct ComputedValues {
    distance: f32,
    gain_l: f32,
    gain_r: f32,
}

impl AudioNode for SpatialBasicNode {
    type Configuration = EmptyConfig;

    fn info(&self, _config: &Self::Configuration) -> AudioNodeInfo {
        AudioNodeInfo::new()
            .debug_name("spatial_basic")
            .channel_config(ChannelConfig {
                num_inputs: ChannelCount::STEREO,
                num_outputs: ChannelCount::STEREO,
            })
    }

    fn construct_processor(
        &self,
        _config: &Self::Configuration,
        cx: ConstructProcessorContext,
    ) -> impl AudioNodeProcessor {
        let computed_values = self.compute_values();

        Processor {
            gain_l: SmoothedParam::new(
                computed_values.gain_l,
                SmootherConfig {
                    smooth_seconds: self.smooth_seconds,
                    ..Default::default()
                },
                cx.stream_info.sample_rate,
            ),
            gain_r: SmoothedParam::new(
                computed_values.gain_r,
                SmootherConfig {
                    smooth_seconds: self.smooth_seconds,
                    ..Default::default()
                },
                cx.stream_info.sample_rate,
            ),
            distance_attenuator: DistanceAttenuatorStereoDsp::new(
                SmootherConfig {
                    smooth_seconds: self.smooth_seconds,
                    ..Default::default()
                },
                cx.stream_info.sample_rate,
                self.coeff_update_factor,
            ),
            params: *self,
        }
    }
}

struct Processor {
    gain_l: SmoothedParam,
    gain_r: SmoothedParam,

    distance_attenuator: DistanceAttenuatorStereoDsp,

    params: SpatialBasicNode,
}

impl AudioNodeProcessor for Processor {
    fn process(
        &mut self,
        info: &ProcInfo,
        buffers: ProcBuffers,
        events: &mut ProcEvents,
        extra: &mut ProcExtra,
    ) -> ProcessStatus {
        let mut updated = false;
        for mut patch in events.drain_patches::<SpatialBasicNode>() {
            match &mut patch {
                SpatialBasicNodePatch::Offset(offset) => {
                    if !(offset.x.is_finite() && offset.y.is_finite() && offset.z.is_finite()) {
                        *offset = Vec3::default();
                    }
                }
                SpatialBasicNodePatch::PanningThreshold(threshold) => {
                    *threshold = threshold.clamp(0.0, 1.0);
                }
                SpatialBasicNodePatch::SmoothSeconds(seconds) => {
                    self.gain_l.set_smooth_seconds(*seconds, info.sample_rate);
                    self.gain_r.set_smooth_seconds(*seconds, info.sample_rate);
                    self.distance_attenuator
                        .set_smooth_seconds(*seconds, info.sample_rate);
                }
                SpatialBasicNodePatch::MinGain(g) => {
                    *g = g.clamp(0.0, 1.0);
                }
                SpatialBasicNodePatch::CoeffUpdateFactor(f) => {
                    self.distance_attenuator.set_coeff_update_factor(*f);
                }
                _ => {}
            }

            self.params.apply(patch);
            updated = true;
        }

        if updated {
            let computed_values = self.params.compute_values();

            self.gain_l.set_value(computed_values.gain_l);
            self.gain_r.set_value(computed_values.gain_r);

            self.distance_attenuator.compute_values(
                computed_values.distance,
                &self.params.distance_attenuation,
                self.params.muffle_cutoff_hz,
                self.params.min_gain,
            );

            if info.prev_output_was_silent {
                // Previous block was silent, so no need to smooth.
                self.gain_l.reset_to_target();
                self.gain_r.reset_to_target();
                self.distance_attenuator.reset();
            }
        }

        if info.in_silence_mask.all_channels_silent(2) {
            self.gain_l.reset_to_target();
            self.gain_r.reset_to_target();
            self.distance_attenuator.reset();

            return ProcessStatus::ClearAllOutputs;
        }

        let scratch_buffer = extra.scratch_buffers.first_mut();

        let (in1, in2) = if info.in_connected_mask == ConnectedMask::STEREO_CONNECTED {
            if self.params.downmix {
                // Downmix the stereo signal to mono.
                for (scratch_s, (&in1, &in2)) in scratch_buffer[..info.frames].iter_mut().zip(
                    buffers.inputs[0][..info.frames]
                        .iter()
                        .zip(buffers.inputs[1][..info.frames].iter()),
                ) {
                    *scratch_s = (in1 + in2) * 0.5;
                }

                (
                    &scratch_buffer[..info.frames],
                    &scratch_buffer[..info.frames],
                )
            } else {
                (
                    &buffers.inputs[0][..info.frames],
                    &buffers.inputs[1][..info.frames],
                )
            }
        } else {
            // Only one (or none) channels are connected, so just use the first
            // channel as input.
            (
                &buffers.inputs[0][..info.frames],
                &buffers.inputs[0][..info.frames],
            )
        };

        // Make doubly sure that the compiler optimizes away the bounds checking
        // in the loop.
        let in1 = &in1[..info.frames];
        let in2 = &in2[..info.frames];

        let (out1, out2) = buffers.outputs.split_first_mut().unwrap();
        let out1 = &mut out1[..info.frames];
        let out2 = &mut out2[0][..info.frames];

        if self.gain_l.has_settled() && self.gain_r.has_settled() {
            if self.gain_l.target_value() <= self.params.min_gain
                && self.gain_r.target_value() <= self.params.min_gain
                && self.distance_attenuator.is_silent()
            {
                self.gain_l.reset_to_target();
                self.gain_r.reset_to_target();
                self.distance_attenuator.reset();

                return ProcessStatus::ClearAllOutputs;
            } else {
                for i in 0..info.frames {
                    out1[i] = in1[i] * self.gain_l.target_value();
                    out2[i] = in2[i] * self.gain_r.target_value();
                }
            }
        } else {
            for i in 0..info.frames {
                let gain_l = self.gain_l.next_smoothed();
                let gain_r = self.gain_r.next_smoothed();

                out1[i] = in1[i] * gain_l;
                out2[i] = in2[i] * gain_r;
            }

            self.gain_l.settle();
            self.gain_r.settle();
        }

        let clear_outputs =
            self.distance_attenuator
                .process(info.frames, out1, out2, info.sample_rate_recip);

        if clear_outputs {
            self.gain_l.reset_to_target();
            self.gain_r.reset_to_target();
            self.distance_attenuator.reset();

            return ProcessStatus::ClearAllOutputs;
        } else {
            ProcessStatus::OutputsModified
        }
    }

    fn new_stream(
        &mut self,
        stream_info: &firewheel_core::StreamInfo,
        _context: &mut ProcStreamCtx,
    ) {
        self.gain_l.update_sample_rate(stream_info.sample_rate);
        self.gain_r.update_sample_rate(stream_info.sample_rate);
        self.distance_attenuator
            .update_sample_rate(stream_info.sample_rate);
    }
}
