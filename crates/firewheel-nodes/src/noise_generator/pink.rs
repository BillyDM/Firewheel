//! A simple node that generates pink noise.
//!
//! Base on the algorithm from <https://www.musicdsp.org/en/latest/Synthesis/244-direct-pink-noise-synthesis-with-auto-correlated-generator.html>

use firewheel_core::{
    channel_config::{ChannelConfig, ChannelCount},
    diff::{Diff, Patch},
    dsp::volume::{Volume, DEFAULT_AMP_EPSILON},
    event::NodeEventList,
    node::{
        AudioNode, AudioNodeInfo, AudioNodeProcessor, ConstructProcessorContext, ProcBuffers,
        ProcInfo, ProcessStatus,
    },
    param::smoother::{SmoothedParam, SmootherConfig},
    SilenceMask,
};

const COEFF_A: [i32; 5] = [14055, 12759, 10733, 12273, 15716];
const COEFF_SUM: [i16; 5] = [22347, 27917, 29523, 29942, 30007];

/// A simple node that generates white noise. (Mono output only)
#[derive(Diff, Patch, Debug, Clone, Copy, PartialEq)]
pub struct PinkNoiseGenNode {
    /// The overall volume.
    ///
    /// Note, pink noise is really loud, so prefer to use a value like
    /// `Volume::Linear(0.4)` or `Volume::Decibels(-18.0)`.
    pub volume: Volume,
    /// Whether or not this node is enabled.
    pub enabled: bool,
}

impl Default for PinkNoiseGenNode {
    fn default() -> Self {
        Self {
            volume: Volume::Linear(0.4),
            enabled: true,
        }
    }
}

/// The configuration for a [`PinkNoiseGenNode`]
#[derive(Debug, Clone)]
pub struct PinkNoiseGenConfig {
    /// The starting seed. This cannot be zero.
    pub seed: i32,
    /// The time in seconds of the internal smoothing filter.
    ///
    /// By default this is set to `0.01` (10ms).
    pub smooth_secs: f32,
}

impl Default for PinkNoiseGenConfig {
    fn default() -> Self {
        Self {
            seed: 17,
            smooth_secs: 10.0 / 1_000.0,
        }
    }
}

impl AudioNode for PinkNoiseGenNode {
    type Configuration = PinkNoiseGenConfig;

    fn info(&self, _config: &Self::Configuration) -> AudioNodeInfo {
        AudioNodeInfo::new()
            .debug_name("pink_noise_gen")
            .channel_config(ChannelConfig {
                num_inputs: ChannelCount::ZERO,
                num_outputs: ChannelCount::MONO,
            })
    }

    fn construct_processor(
        &self,
        config: &Self::Configuration,
        cx: ConstructProcessorContext,
    ) -> impl AudioNodeProcessor {
        // Seed cannot be zero.
        let seed = if config.seed == 0 { 17 } else { config.seed };

        Processor {
            gain: SmoothedParam::new(
                self.volume.amp_clamped(DEFAULT_AMP_EPSILON),
                SmootherConfig {
                    smooth_secs: config.smooth_secs,
                    ..Default::default()
                },
                cx.stream_info.sample_rate,
            ),
            params: *self,
            fpd: seed,
            contrib: [0; 5],
            accum: 0,
        }
    }
}

// The realtime processor counterpart to your node.
struct Processor {
    params: PinkNoiseGenNode,
    gain: SmoothedParam,

    // white noise generator state
    fpd: i32,

    // filter stage contributions
    contrib: [i32; 5],
    accum: i32,
}

impl AudioNodeProcessor for Processor {
    fn process(
        &mut self,
        buffers: ProcBuffers,
        _proc_info: &ProcInfo,
        events: &mut NodeEventList,
    ) -> ProcessStatus {
        for patch in events.drain_patches::<PinkNoiseGenNode>() {
            if let PinkNoiseGenNodePatch::Volume(vol) = patch {
                self.gain.set_value(vol.amp_clamped(DEFAULT_AMP_EPSILON));
            }

            self.params.apply(patch);
        }

        if !self.params.enabled || (self.gain.target_value() == 0.0 && !self.gain.is_smoothing()) {
            self.gain.reset();
            return ProcessStatus::ClearAllOutputs;
        }

        for s in buffers.outputs[0].iter_mut() {
            // i16[0,32767]
            let randu: i16 = (rng(&mut self.fpd) & 0x7fff) as i16;

            // i32[-32768,32767]
            let r_bytes = rng(&mut self.fpd).to_ne_bytes();
            let randv: i32 = i16::from_ne_bytes([r_bytes[0], r_bytes[1]]) as i32;

            if randu < COEFF_SUM[0] {
                update_contrib::<0>(&mut self.accum, &mut self.contrib, randv);
            } else if randu < COEFF_SUM[1] {
                update_contrib::<1>(&mut self.accum, &mut self.contrib, randv);
            } else if randu < COEFF_SUM[2] {
                update_contrib::<2>(&mut self.accum, &mut self.contrib, randv);
            } else if randu < COEFF_SUM[3] {
                update_contrib::<3>(&mut self.accum, &mut self.contrib, randv);
            } else if randu < COEFF_SUM[4] {
                update_contrib::<4>(&mut self.accum, &mut self.contrib, randv);
            }

            // Get a random normalized value in the range `[-1.0, 1.0]`.
            let r = self.accum as f32 * (1.0 / 2_147_483_648.0);

            *s = r * self.gain.next_smoothed();
        }

        ProcessStatus::OutputsModified {
            out_silence_mask: SilenceMask::NONE_SILENT,
        }
    }
}

#[inline(always)]
fn rng(fpd: &mut i32) -> i32 {
    *fpd ^= *fpd << 13;
    *fpd ^= *fpd >> 17;
    *fpd ^= *fpd << 5;

    *fpd
}

#[inline(always)]
fn update_contrib<const I: usize>(accum: &mut i32, contrib: &mut [i32; 5], randv: i32) {
    *accum = accum.wrapping_sub(contrib[I]);
    contrib[I] = randv * COEFF_A[I];
    *accum = accum.wrapping_add(contrib[I]);
}
