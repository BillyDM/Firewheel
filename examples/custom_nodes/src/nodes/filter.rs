//! This node applies a simple single-pole lowpass filter to a stereo signal.
//!
//! It also demonstrates how to make proper use of the parameter smoothers from
//! the dsp module, as well as how to make proper use of the silence flags for
//! optimization.

use std::f32::consts::PI;

use firewheel::dsp::coeff_update::{CoeffUpdateFactor, CoeffUpdateMask};
use firewheel::node::NodeError;
use firewheel::{
    channel_config::{ChannelConfig, ChannelCount},
    diff::{Diff, Patch},
    dsp::volume::{Volume, DEFAULT_MIN_AMP},
    event::ProcEvents,
    node::{
        AudioNode, AudioNodeInfo, AudioNodeProcessor, ConstructProcessorContext, EmptyConfig,
        ProcBuffers, ProcExtra, ProcInfo, ProcStreamCtx, ProcessStatus,
    },
    param::smoother::{SmoothedParam, SmoothedParamBuffer},
    StreamInfo,
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
pub struct FilterNode {
    /// The cutoff frequency in hertz in the range `[20.0, 20_000.0]`.
    pub cutoff_hz: f32,

    /// The overall volume.
    pub volume: Volume,

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

impl Default for FilterNode {
    fn default() -> Self {
        Self {
            cutoff_hz: 1_000.0,
            volume: Volume::default(),
            coeff_update_factor: CoeffUpdateFactor::default(),
        }
    }
}

// Implement the AudioNode type for your node.
impl AudioNode for FilterNode {
    // Since this node doesnt't need any configuration, we'll just
    // default to `EmptyConfig`.
    type Configuration = EmptyConfig;

    // Return information about your node. This method is only ever called
    // once.
    fn info(&self, _config: &Self::Configuration) -> Result<AudioNodeInfo, NodeError> {
        // The builder pattern is used for future-proofness as it is likely that
        // more fields will be added in the future.
        Ok(AudioNodeInfo::new()
            // A static name used for debugging purposes.
            .debug_name("example_filter")
            // The configuration of the input/output ports.
            .channel_config(ChannelConfig {
                num_inputs: ChannelCount::STEREO,
                num_outputs: ChannelCount::STEREO,
            })
            // Since the number of inputs and outputs are equal and we don't have
            // a wet/dry mix parameter, we can take advantage of in-place buffer
            // optimizations.
            //
            // With this turned on, the number of inputs buffers in the process
            // method will be `0`.
            .in_place_buffers(true))
    }

    // Construct the realtime processor counterpart using the given information
    // about the audio stream.
    //
    // This method is called before the node processor is sent to the realtime
    // thread, so it is safe to do non-realtime things here like allocating.
    fn construct_processor(
        &self,
        _config: &Self::Configuration,
        cx: ConstructProcessorContext,
    ) -> Result<impl AudioNodeProcessor, NodeError> {
        // The reciprocal of the sample rate.
        let sample_rate_recip = cx.stream_info.sample_rate_recip as f32;

        let cutoff_hz = self.cutoff_hz.clamp(20.0, 20_000.0);
        let gain = self.volume.amp_clamped(DEFAULT_MIN_AMP);

        Ok(Processor {
            filter_l: OnePoleLPBiquad::new(cutoff_hz, sample_rate_recip),
            filter_r: OnePoleLPBiquad::new(cutoff_hz, sample_rate_recip),
            cutoff_hz: SmoothedParam::new(
                cutoff_hz,
                Default::default(),
                cx.stream_info.sample_rate,
            ),
            gain: SmoothedParamBuffer::new(gain, Default::default(), cx.stream_info),
            coeff_update_mask: self.coeff_update_factor.mask(),
        })
    }
}

// The realtime processor counterpart to your node.
struct Processor {
    filter_l: OnePoleLPBiquad,
    filter_r: OnePoleLPBiquad,
    // A helper struct to smooth a parameter.
    cutoff_hz: SmoothedParam,
    // This is similar to `SmoothedParam`, but it also contains an allocated buffer
    // for the smoothed values.
    gain: SmoothedParamBuffer,
    coeff_update_mask: CoeffUpdateMask,
}

impl Processor {
    fn reset(&mut self) {
        self.filter_l.reset();
        self.filter_r.reset();
        self.cutoff_hz.reset_to_target();
        self.gain.reset();
    }
}

impl AudioNodeProcessor for Processor {
    // Called when there are new events for this node to process.
    //
    // This is called once before the first call to `process`, and after that
    // it will be called whenever there are new events (including when the
    // node is bypassed).
    //
    // Unless this node is bypassed, then [`AudioNodeProcessor::process`] will be
    // called immediately after.
    //
    // This is always called in a realtime thread, so do not perform any
    // realtime-unsafe operations.
    fn events(&mut self, _info: &ProcInfo, events: &mut ProcEvents, _extra: &mut ProcExtra) {
        // Process the events.
        //
        // We don't need to keep around a `FilterNode` instance,
        // so we can just match on each event directly.
        for patch in events.drain_patches::<FilterNode>() {
            match patch {
                FilterNodePatch::CutoffHz(cutoff) => {
                    self.cutoff_hz.set_value(cutoff.clamp(20.0, 20_000.0));
                }
                FilterNodePatch::Volume(volume) => {
                    self.gain.set_value(volume.amp_clamped(DEFAULT_MIN_AMP));
                }
                FilterNodePatch::CoeffUpdateFactor(factor) => {
                    self.coeff_update_mask = factor.mask();
                }
            }
        }
    }

    // Called when the node has been fully bypassed/unbypassed.
    //
    // The Firewheel processor automatically handles bypass declicking, so
    // there is no need to handle that manually.
    //
    // This is always called in a realtime thread, so do not perform any
    // realtime-unsafe operations.
    fn bypassed(&mut self, _bypassed: bool) {
        self.reset();
    }

    // The realtime process method.
    //
    // This is always called in a realtime thread, so do not perform any
    // realtime-unsafe operations.
    fn process(
        &mut self,
        // Information about the process block.
        info: &ProcInfo,
        // The buffers of data to process.
        buffers: ProcBuffers,
        // Extra buffers and utilities.
        _extra: &mut ProcExtra,
    ) -> ProcessStatus {
        // If the gain parameter is not currently smoothing and is silent, then
        // there is no need to process.
        let gain_is_silent = self.gain.has_settled_at_or_below(DEFAULT_MIN_AMP);

        // Read the output silence mask since we are operating with in-place buffers.
        if info.out_silence_mask.all_channels_silent(2) || gain_is_silent {
            // Outputs will be silent, so no need to process.

            // Reset the smoothers and filters since they don't need to smooth any
            // output.
            self.reset();

            return ProcessStatus::ClearAllOutputs;
        }

        // Get slices of the output buffers.
        //
        // Doing it this way allows the compiler to better optimize the processing
        // loops below.
        let (out1, out2) = buffers.outputs.split_first_mut().unwrap();
        let out1 = &mut out1[..info.frames];
        let out2 = &mut out2[0][..info.frames];

        // Retrieve a buffer of the smoothed gain values.
        //
        // The redundant slicing is not strictly necessary, but it may help make sure
        // the compiler properly optimizes the below processing loops.
        let gain = &self.gain.get_buffer(info.frames).0[..info.frames];

        if self.cutoff_hz.is_smoothing() {
            for i in 0..info.frames {
                let cutoff_hz = self.cutoff_hz.next_smoothed();

                // Because recalculating filter coefficients is expensive, a trick like
                // this can be used to only recalculate them every 16 frames.
                if self.coeff_update_mask.do_update(i) {
                    self.filter_l
                        .set_cutoff(cutoff_hz, info.sample_rate_recip as f32);
                    self.filter_r.copy_cutoff_from(&self.filter_l);
                }

                let fl = self.filter_l.process(out1[i]);
                let fr = self.filter_r.process(out2[i]);

                out1[i] = fl * gain[i];
                out2[i] = fr * gain[i];
            }

            // Settle the filter if its state is close enough to the target value.
            // Otherwise `self.cutoff_hz.is_smoothing()` will always return `true`.
            self.cutoff_hz.settle();
        } else {
            // The cutoff parameter is not currently smoothing, so we can optimize by
            // only updating the filter coefficients once.
            self.filter_l
                .set_cutoff(self.cutoff_hz.target_value(), info.sample_rate_recip as f32);
            self.filter_r.copy_cutoff_from(&self.filter_l);

            for i in 0..info.frames {
                let fl = self.filter_l.process(out1[i]);
                let fr = self.filter_r.process(out2[i]);

                out1[i] = fl * gain[i];
                out2[i] = fr * gain[i];
            }
        }

        // Notify the engine that we have modified the output buffers.
        //
        // WARNING: The node must fill all audio audio output buffers
        // completely with data when returning this process status.
        // Failing to do so will result in audio glitches.
        ProcessStatus::OutputsModified
    }

    // Called when a new stream has been created. Because the new stream may have a
    // different sample rate from the old one, make sure to update any calculations
    // that depend on the sample rate.
    //
    // This gets called outside of the audio thread, so it is safe to allocate and
    // deallocate here.
    fn new_stream(&mut self, stream_info: &StreamInfo, _context: &mut ProcStreamCtx) {
        self.cutoff_hz.update_sample_rate(stream_info.sample_rate);
        self.gain.update_stream(stream_info);

        self.filter_l.set_cutoff(
            self.cutoff_hz.target_value(),
            stream_info.sample_rate_recip as f32,
        );
        self.filter_r.copy_cutoff_from(&self.filter_l);
    }
}

// A simple one pole lowpass biquad filter.
struct OnePoleLPBiquad {
    a0: f32,
    b1: f32,
    z1: f32,
}

impl OnePoleLPBiquad {
    pub fn new(cutoff_hz: f32, sample_rate_recip: f32) -> Self {
        let mut new_self = Self {
            a0: 0.0,
            b1: 0.0,
            z1: 0.0,
        };

        new_self.set_cutoff(cutoff_hz, sample_rate_recip);

        new_self
    }

    pub fn reset(&mut self) {
        self.z1 = 0.0;
    }

    #[inline]
    pub fn set_cutoff(&mut self, cutoff_hz: f32, sample_rate_recip: f32) {
        self.b1 = (-2.0 * PI * cutoff_hz * sample_rate_recip).exp();
        self.a0 = 1.0 - self.b1;
    }

    #[inline]
    pub fn copy_cutoff_from(&mut self, other: &Self) {
        self.a0 = other.a0;
        self.b1 = other.b1;
    }

    #[inline]
    pub fn process(&mut self, s: f32) -> f32 {
        self.z1 = (self.a0 * s) + (self.b1 * self.z1);
        self.z1
    }
}
