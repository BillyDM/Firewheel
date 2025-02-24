//! This node applies a simple single-pole lowpass filter to a stereo signal.
//!
//! It also demonstrates how to make proper use of the parameter smoothers and
//! declickers from the dsp module, as well as how to make proper use of the
//! silence flags for optimization.

use std::f32::consts::PI;

use firewheel::{
    channel_config::{ChannelConfig, ChannelCount},
    diff::{Diff, Patch},
    dsp::{
        decibel::normalized_volume_to_raw_gain,
        declick::{Declicker, FadeType},
    },
    event::NodeEventList,
    node::{
        AudioNodeConstructor, AudioNodeInfo, AudioNodeProcessor, EmptyConfig, ProcInfo,
        ProcessStatus, NUM_SCRATCH_BUFFERS,
    },
    param::smoother::{SmoothedParam, SmoothedParamBuffer},
    SilenceMask, StreamInfo,
};

// The parameter struct holds all of the parameters of the node as plain values.
#[derive(Diff, Patch, Debug, Clone, Copy, PartialEq)]
pub struct FilterParams {
    /// The cutoff frequency in hertz in the range `[20.0, 20_000.0]`.
    pub cutoff_hz: f32,
    /// The normalized volume where `0.0` is mute and `1.0` is unity gain.
    pub normalized_volume: f32,
    /// Whether or not this node is enabled.
    pub enabled: bool,
}

impl Default for FilterParams {
    fn default() -> Self {
        Self {
            cutoff_hz: 1_000.0,
            normalized_volume: 1.0,
            enabled: true,
        }
    }
}

// Implement the AudioNodeConstructor type for your node.
impl AudioNodeConstructor for FilterParams {
    // Since this node doesnt't need any configuration, we'll just
    // default to `EmptyConfig`.
    type Configuration = EmptyConfig;

    // Return information about your node. This method is only ever called
    // once.
    fn info(&self, _config: &Self::Configuration) -> AudioNodeInfo {
        AudioNodeInfo {
            // A static name used for debugging purposes.
            debug_name: "example_filter",
            // The configuration of the input/output ports.
            channel_config: ChannelConfig {
                num_inputs: ChannelCount::STEREO,
                num_outputs: ChannelCount::STEREO,
            },
            // Wether or not our node uses events. If it does not, then setting
            // this to `false` will save a bit of memory by not allocating an
            // event buffer for this node.
            uses_events: true,
        }
    }

    // Construct the realtime processor counterpart using the given information
    // about the audio stream.
    //
    // This method is called before the node processor is sent to the realtime
    // thread, so it is safe to do non-realtime things here like allocating.
    fn processor(
        &self,
        _config: &Self::Configuration,
        stream_info: &StreamInfo,
    ) -> impl AudioNodeProcessor {
        // The reciprocal of the sample rate.
        let sample_rate_recip = stream_info.sample_rate_recip as f32;

        let cutoff_hz = self.cutoff_hz.clamp(20.0, 20_000.0);
        let gain = normalized_volume_to_raw_gain(self.normalized_volume);

        Processor {
            filter_l: OnePoleLPBiquad::new(cutoff_hz, sample_rate_recip),
            filter_r: OnePoleLPBiquad::new(cutoff_hz, sample_rate_recip),
            cutoff_hz: SmoothedParam::new(cutoff_hz, Default::default(), stream_info.sample_rate),
            gain: SmoothedParamBuffer::new(gain, Default::default(), stream_info),
            enable_declicker: Declicker::from_enabled(self.enabled),
            params: *self,
            sample_rate_recip,
        }
    }
}

// The realtime processor counterpart to your node.
struct Processor {
    filter_l: OnePoleLPBiquad,
    filter_r: OnePoleLPBiquad,
    params: FilterParams,
    // A helper struct to smooth a parameter.
    cutoff_hz: SmoothedParam,
    // This is similar to `SmoothedParam`, but it also contains an allocated buffer
    // for the smoothed values.
    gain: SmoothedParamBuffer,
    // This struct is used to declick when enabling/disabling this node.
    enable_declicker: Declicker,
    sample_rate_recip: f32,
}

impl AudioNodeProcessor for Processor {
    // The realtime process method.
    fn process(
        &mut self,
        // The list of input buffers. This will always be equal to the number we
        // gave in `info()`.`
        inputs: &[&[f32]],
        // The list of output buffers. This will always be equal to the number we
        // gave in `info()`.`
        outputs: &mut [&mut [f32]],
        // The list of events for our node to process.
        events: NodeEventList,
        // Additional information about the process.
        proc_info: &ProcInfo,
        // Optional scratch buffers that can be used for processing.
        _scratch_buffers: &mut [&mut [f32]; NUM_SCRATCH_BUFFERS],
    ) -> ProcessStatus {
        // Process the events.
        //
        // If a parameter was actually updated,
        // `patch_list` will return true.
        let enabled = self.params.enabled;
        if self.params.patch_list(events) {
            self.cutoff_hz
                .set_value(self.params.cutoff_hz.clamp(20.0, 20_000.0));
            self.gain
                .set_value(normalized_volume_to_raw_gain(self.params.normalized_volume));

            if enabled != self.params.enabled {
                // Tell the declicker to crossfade.
                self.enable_declicker
                    .fade_to_enabled(self.params.enabled, proc_info.declick_values);
            }
        }

        if self.enable_declicker.disabled() {
            // Disabled. Bypass this node.
            return ProcessStatus::Bypass;
        }

        // If the gain parameter is not currently smoothing and is silent, then
        // there is no need to process.
        let gain_is_silent = !self.gain.is_smoothing() && self.gain.target_value() < 0.00001;

        if proc_info.in_silence_mask.all_channels_silent(2) || gain_is_silent {
            // Outputs will be silent, so no need to process.

            // Reset the smoothers and filters since they don't need to smooth any
            // output.
            self.cutoff_hz.reset();
            self.gain.reset();
            self.filter_l.reset();
            self.filter_r.reset();
            self.enable_declicker.reset_to_target();

            return ProcessStatus::ClearAllOutputs;
        }

        // Get slices of the input and output buffers.
        //
        // Doing it this way allows the compiler to better optimize the processing
        // loops below.
        let in1 = &inputs[0][..proc_info.frames];
        let in2 = &inputs[1][..proc_info.frames];
        let (out1, out2) = outputs.split_first_mut().unwrap();
        let out1 = &mut out1[..proc_info.frames];
        let out2 = &mut out2[0][..proc_info.frames];

        // Retrieve a buffer of the smoothed gain values.
        //
        // The redundant slicing is not strictly necessary, but it may help make sure
        // the compiler properly optimizes the below processing loops.
        let gain = &self.gain.get_buffer(proc_info.frames)[..proc_info.frames];

        if self.cutoff_hz.is_smoothing() {
            for i in 0..proc_info.frames {
                let cutoff_hz = self.cutoff_hz.next_smoothed();

                // Because recalculating filter coefficients is expensive, a trick like
                // this can be use to only recalculate them every 16 frames.
                if i & 15 == 0 {
                    self.filter_l.set_cutoff(cutoff_hz, self.sample_rate_recip);
                    self.filter_r.copy_cutoff_from(&self.filter_l);
                }

                let fl = self.filter_l.process(in1[i]);
                let fr = self.filter_r.process(in2[i]);

                out1[i] = fl * gain[i];
                out2[i] = fr * gain[i];
            }
        } else {
            // The cutoff parameter is not currently smoothing, so we can optimize by
            // only updating the filter coefficients once.
            self.filter_l
                .set_cutoff(self.cutoff_hz.target_value(), self.sample_rate_recip);
            self.filter_r.copy_cutoff_from(&self.filter_l);

            for i in 0..proc_info.frames {
                let fl = self.filter_l.process(in1[i]);
                let fr = self.filter_r.process(in2[i]);

                out1[i] = fl * gain[i];
                out2[i] = fr * gain[i];
            }
        }

        // Crossfade between the wet and dry signals to declick enabling/disabling.
        self.enable_declicker.process_crossfade(
            inputs,
            outputs,
            proc_info.frames,
            proc_info.declick_values,
            FadeType::EqualPower3dB,
        );

        // Notify the engine that we have modified the output buffers.
        ProcessStatus::OutputsModified {
            out_silence_mask: SilenceMask::NONE_SILENT,
        }
    }

    // Called when a new stream has been created. Because the new stream may have a
    // different sample rate from the old one, make sure to update any calculations
    // that depend on the sample rate.
    //
    // This gets called outside of the audio thread, so it is safe to allocate and
    // deallocate here.
    fn new_stream(&mut self, stream_info: &StreamInfo) {
        self.sample_rate_recip = stream_info.sample_rate_recip as f32;

        self.cutoff_hz.update_sample_rate(stream_info.sample_rate);
        self.gain.update_stream(stream_info);

        self.filter_l
            .set_cutoff(self.cutoff_hz.target_value(), self.sample_rate_recip);
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
