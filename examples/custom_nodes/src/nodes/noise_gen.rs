//! A simple node that generates white noise.

use firewheel::{
    channel_config::{ChannelConfig, ChannelCount},
    diff::{Diff, Patch},
    dsp::volume::{Volume, DEFAULT_AMP_EPSILON},
    event::NodeEventList,
    node::{
        AudioNode, AudioNodeInfo, AudioNodeProcessor, ConstructProcessorContext, ProcBuffers,
        ProcInfo, ProcessStatus,
    },
    SilenceMask,
};

// The node struct holds all of the parameters of the node as plain values.
///
/// # Notes about ECS
///
/// In order to be friendlier to ECS's (entity component systems), it is encouraged
/// that any struct deriving this trait be POD (plain ol' data). If you want your
/// audio node to be usable in the Bevy game engine, also derive
/// `bevy_ecs::prelude::Component`. (You can hide this derive behind a feature flag
/// by using `#[cfg_attr(feature = "bevy", derive(bevy_ecs::prelude::Component))]`).
#[derive(Diff, Patch, Debug, Clone, Copy, PartialEq)]
pub struct NoiseGenNode {
    /// The overall volume.
    ///
    /// Note, white noise is really loud, so prefer to use a value like
    /// `Volume::Linear(0.4)` or `Volume::Decibels(-18.0)`.
    pub volume: Volume,
    /// Whether or not this node is enabled.
    pub enabled: bool,
}

impl Default for NoiseGenNode {
    fn default() -> Self {
        Self {
            volume: Volume::Linear(0.4),
            enabled: true,
        }
    }
}

// The configuration allows users to provide
// one-time initialization settings for your
// processors.
//
// Here we provide a "seed" for the random number generator
#[derive(Debug, Clone, Copy)]
pub struct NoiseGenConfig {
    pub seed: u32,
}

impl Default for NoiseGenConfig {
    fn default() -> Self {
        Self { seed: 17 }
    }
}

// Implement the AudioNode type for your node.
impl AudioNode for NoiseGenNode {
    type Configuration = NoiseGenConfig;

    // Return information about your node. This method is only ever called
    // once.
    fn info(&self, _config: &Self::Configuration) -> AudioNodeInfo {
        // The builder pattern is used for future-proofness as it is likely that
        // more fields will be added in the future.
        AudioNodeInfo::new()
            // A static name used for debugging purposes.
            .debug_name("example_noise_gen")
            // The configuration of the input/output ports.
            .channel_config(ChannelConfig {
                num_inputs: ChannelCount::ZERO,
                num_outputs: ChannelCount::MONO,
            })
    }

    // Construct the realtime processor counterpart using the given information
    // about the audio stream.
    //
    // This method is called before the node processor is sent to the realtime
    // thread, so it is safe to do non-realtime things here like allocating.
    fn construct_processor(
        &self,
        config: &Self::Configuration,
        _cx: ConstructProcessorContext,
    ) -> impl AudioNodeProcessor {
        // Seed cannot be zero.
        let seed = if config.seed == 0 { 17 } else { config.seed };

        Processor {
            fpd: seed,
            gain: self.volume.amp_clamped(DEFAULT_AMP_EPSILON),
            params: *self,
        }
    }
}

// The realtime processor counterpart to your node.
struct Processor {
    fpd: u32,
    params: NoiseGenNode,
    gain: f32,
}

impl AudioNodeProcessor for Processor {
    // The realtime process method.
    fn process(
        &mut self,
        // The buffers of data to process.
        buffers: ProcBuffers,
        // Additional information about the process.
        _proc_info: &ProcInfo,
        // The list of events for our node to process.
        events: &mut NodeEventList,
    ) -> ProcessStatus {
        // Process the events.
        for patch in events.drain_patches::<NoiseGenNode>() {
            // Since we want to clamp the volume event, we can
            // grab it here and perform the processing only when required.
            if let NoiseGenNodePatch::Volume(vol) = &patch {
                self.gain = vol.amp_clamped(DEFAULT_AMP_EPSILON);
            }

            self.params.apply(patch);
        }

        if !self.params.enabled || self.gain == 0.0 {
            // Tell the engine to automatically and efficiently clear the output buffers
            // for us. This is equivalent to doing:
            // ```
            // for (i, out_ch) in buffers.outputs.iter_mut().enumerate() {
            //    if !proc_info.out_silence_mask.is_channel_silent(i) {
            //        out_ch.fill(0.0);
            //    } // otherwise buffer is already silent
            // }
            //
            // return ProcessStatus::OutputsModified { out_silence_mask: SilenceMask::new_all_silent(buffers.outputs.len()) };
            // ```
            return ProcessStatus::ClearAllOutputs;
        }

        for s in buffers.outputs[0].iter_mut() {
            // Tick the random number generator.
            self.fpd ^= self.fpd << 13;
            self.fpd ^= self.fpd >> 17;
            self.fpd ^= self.fpd << 5;

            // Get a random normalized value in the range `[-1.0, 1.0]`.
            let r = self.fpd as f32 * (2.0 / 4_294_967_295.0) - 1.0;

            *s = r * self.gain;
        }

        // Notify the engine that we have modified the output buffers.
        ProcessStatus::OutputsModified {
            out_silence_mask: SilenceMask::NONE_SILENT,
        }
    }
}
