use firewheel_core::{
    channel_config::{ChannelConfig, ChannelCount},
    clock::{ClockSamples, ClockSeconds},
    diff::{Diff, Patch},
    dsp::volume::Volume,
    event::{NodeEventList, NodeEventType},
    node::{
        AudioNode, AudioNodeInfo, AudioNodeProcessor, ConstructProcessorContext, EmptyConfig,
        ProcBuffers, ProcInfo, ProcessStatus,
    },
    param::smoother::SmoothedParam,
    StreamInfo,
};
use std::{mem, num::NonZeroU32, ops::Range};

/// How an envelope responds to receiving [`TriggerEvent::On`] if it is not at rest.
#[derive(Diff, Patch, Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub enum RetriggerMode {
    /// Restart the cycle of the envelope from the `attack` section, but use the current value
    /// as the starting point.
    #[default]
    Normal,
    /// Restart the envelope from `lo`.
    Restart,
    /// Ignore any new [`TriggerEvent::On`] events unless the envelope is at rest.
    Ignore,
}

impl RetriggerMode {
    fn state_transition(&self) -> fn(f32, Option<AhdsrState>) -> (f32, AhdsrState) {
        match self {
            RetriggerMode::Ignore => |cur, state| (cur, state.unwrap_or(AhdsrState::Attack)),
            RetriggerMode::Normal => |cur, _| (cur, AhdsrState::Attack),
            RetriggerMode::Restart => |_, _| (0., AhdsrState::Attack),
        }
    }
}

#[derive(Diff, Patch, Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub enum TriggerMode {
    /// Start the envelope on [`TriggerEvent::On`], holding at `sustain` until [`TriggerEvent::Off`] is received.
    #[default]
    Normal,
    /// Ignore [`TriggerEvent::Off`] events, simply go through the full cycle without further input.
    Once,
}

impl TriggerMode {
    fn state_transition(&self) -> fn(AhdsrState) -> AhdsrState {
        match self {
            TriggerMode::Normal => |_| AhdsrState::Release,
            TriggerMode::Once => |state| state,
        }
    }
}

/// An event to send to an envelope.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Hash)]
pub enum TriggerEvent {
    /// Start the envelope.
    On,
    /// Return the envelope to its resting state.
    Off,
}

#[cfg_attr(feature = "bevy", derive(bevy_ecs::prelude::Component))]
#[derive(Diff, Patch, Debug, Clone, Copy, PartialEq)]
pub struct AhdsrVolumeNode {
    /// The low value, used when the envelope is not triggered.
    pub lo: Volume,
    /// The high value, used when the envelope is at its peak.
    pub hi: Volume,
    /// The amount of time to transition between `hi` and `lo`.
    pub attack: ClockSeconds,
    /// The amount of time to hold the peak before progressing to decay.
    pub hold: ClockSeconds,
    /// The amount of time to transition between `hi` and `sustain`.
    pub decay: ClockSeconds,
    /// The ratio between `lo` and `hi` to decay to.
    pub sustain_proportion: f32,
    /// The amount of time to transition between `sustain` and `lo`.
    pub release: ClockSeconds,
    /// How to respond to [`TriggerEvent`]s when the envelope is at rest.
    pub trigger_mode: TriggerMode,
    /// How to respond to [`TriggerEvent`]s when the envelope is already triggered.
    pub retrigger_mode: RetriggerMode,
}

impl Default for AhdsrVolumeNode {
    fn default() -> Self {
        Self {
            lo: Volume::SILENT,
            hi: Volume::UNITY_GAIN,
            attack: Default::default(),
            hold: Default::default(),
            decay: Default::default(),
            sustain_proportion: 1.,
            release: Default::default(),

            trigger_mode: Default::default(),
            retrigger_mode: Default::default(),
        }
    }
}

impl AudioNode for AhdsrVolumeNode {
    type Configuration = EmptyConfig;

    fn info(&self, _config: &Self::Configuration) -> AudioNodeInfo {
        AudioNodeInfo::new()
            .debug_name("ahdsr_volume")
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
        let lo = self.lo.linear();
        let hi = self.hi.linear();
        let sustain_proportion = self.sustain_proportion.clamp(0., 1.);

        AhdsrVolumeProcessor {
            state: None,
            current: SmoothedParam::new(lo, Default::default(), cx.stream_info.sample_rate),
            current_smoothed: lo,

            lo,
            hi,

            attack_rate: (self.attack.0 * cx.stream_info.sample_rate_recip) as _,
            hold_samples: ClockSamples::from_secs_f64(
                self.hold.0,
                cx.stream_info.sample_rate.get(),
            ),
            decay_rate: (1. - sustain_proportion)
                * (self.decay.0 * cx.stream_info.sample_rate_recip) as f32,
            sustain_proportion,
            release_rate: sustain_proportion
                * (self.release.0 * cx.stream_info.sample_rate_recip) as f32,

            attack: self.attack,
            hold: self.hold,
            decay: self.decay,
            release: self.release,
            sample_rate: cx.stream_info.sample_rate,
            sample_rate_recip: cx.stream_info.sample_rate_recip,

            off_state_transition: self.trigger_mode.state_transition(),
            on_state_transition: self.retrigger_mode.state_transition(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum AhdsrState {
    Attack,
    /// The inner value is the number of samples that the value has been held for.
    Hold(i64),
    Decay,
    Sustain,
    Release,
}

struct AhdsrVolumeProcessor {
    state: Option<AhdsrState>,

    /// Normalized 0..1, with 0 = lo, 1 = hi. This allows us to change `hi`/`lo`/`sustain` while the envelope
    /// is activated.
    current: SmoothedParam,
    current_smoothed: f32,

    lo: f32,
    hi: f32,

    attack_rate: f32,
    hold_samples: ClockSamples,
    decay_rate: f32,
    sustain_proportion: f32,
    release_rate: f32,

    // -- Information to reconstruct the above fields when the node params change --
    attack: ClockSeconds,
    hold: ClockSeconds,
    decay: ClockSeconds,
    release: ClockSeconds,
    sample_rate: NonZeroU32,
    sample_rate_recip: f64,

    off_state_transition: fn(AhdsrState) -> AhdsrState,
    on_state_transition: fn(f32, Option<AhdsrState>) -> (f32, AhdsrState),
}

#[inline(always)]
fn lerp(a: f32, b: f32, ratio: f32) -> f32 {
    let ratio = ratio.clamp(0., 1.);
    a * (1. - ratio) + b * ratio
}

pub struct TimedTriggerEvent {
    pub sample: ClockSamples,
    pub event: TriggerEvent,
}

impl TimedTriggerEvent {
    fn from_bytes(bytes: &[u8; 16]) -> Option<Self> {
        let mut clock_sample_bytes = [0u8; mem::size_of::<i64>()];
        clock_sample_bytes.copy_from_slice(&bytes[8..]);
        let clock_sample = i64::from_ne_bytes(clock_sample_bytes);
        let trigger_event = match bytes[..8] {
            [0, 0, 0, 0, 0, 0, 0, 0] => TriggerEvent::Off,
            [1, 0, 0, 0, 0, 0, 0, 0] => TriggerEvent::On,
            _ => return None,
        };

        Some(Self {
            sample: ClockSamples(clock_sample),
            event: trigger_event,
        })
    }

    pub fn into_bytes(&self) -> [u8; 16] {
        let mut out_bytes = [0u8; 16];
        out_bytes[8..].copy_from_slice(&i64::to_ne_bytes(self.sample.0));

        if self.event == TriggerEvent::On {
            out_bytes[0] = 1;
        }

        out_bytes
    }
}

fn close_enough(a: f32, b: f32) -> bool {
    (a - b).abs() <= f32::EPSILON
}

impl AhdsrVolumeProcessor {
    fn tick(&mut self) {
        match &mut self.state {
            Some(AhdsrState::Attack) => {
                self.current
                    .set_value((self.current.target_value() + self.attack_rate).min(1.));
                self.current_smoothed = self.current.next_smoothed();
                if close_enough(self.current_smoothed, 1.) {
                    self.state = Some(AhdsrState::Hold(0));
                }
            }
            Some(AhdsrState::Hold(val)) => {
                self.current_smoothed = self.current.next_smoothed();
                if *val >= self.hold_samples.0 {
                    self.state = Some(AhdsrState::Decay);
                } else {
                    *val += 1;
                }
            }
            Some(AhdsrState::Decay) => {
                self.current.set_value(
                    (self.current.target_value() - self.decay_rate).max(self.sustain_proportion),
                );
                self.current_smoothed = self.current.next_smoothed();
                if close_enough(self.current_smoothed, self.sustain_proportion) {
                    self.state = Some(AhdsrState::Sustain);
                }
            }
            Some(AhdsrState::Release) => {
                self.current
                    .set_value((self.current.target_value() - self.release_rate).max(0.));
                self.current_smoothed = self.current.next_smoothed();
                if close_enough(self.current_smoothed, 0.) {
                    self.state = None;
                }
            }
            None | Some(AhdsrState::Sustain) => {
                self.current_smoothed = self.current.next_smoothed();
            }
        }
    }
}

impl AudioNodeProcessor for AhdsrVolumeProcessor {
    fn process(
        &mut self,
        buffers: ProcBuffers,
        proc_info: &ProcInfo,
        mut events: NodeEventList,
    ) -> ProcessStatus {
        events.for_each_patch::<AhdsrVolumeNode>(|patch| match patch {
            AhdsrVolumeNodePatch::Lo(lo) => {
                self.lo = lo.linear();
            }
            AhdsrVolumeNodePatch::Hi(hi) => {
                self.hi = hi.linear();
            }
            AhdsrVolumeNodePatch::Attack(val) => {
                self.attack = val;
                self.attack_rate = (val.0 * self.sample_rate_recip) as _;
            }
            AhdsrVolumeNodePatch::Hold(val) => {
                self.hold = val;
                self.hold_samples =
                    ClockSamples::from_secs_f64(self.hold.0, self.sample_rate.get());
            }
            AhdsrVolumeNodePatch::Decay(val) => {
                self.decay = val;
                self.decay_rate =
                    (1. - self.sustain_proportion) * (val.0 * self.sample_rate_recip) as f32;
            }
            AhdsrVolumeNodePatch::SustainProportion(sustain_proportion) => {
                self.sustain_proportion = sustain_proportion;
                self.decay_rate =
                    (1. - self.sustain_proportion) * (self.decay.0 * self.sample_rate_recip) as f32;
                self.release_rate = (1. - self.sustain_proportion)
                    * (self.release.0 * self.sample_rate_recip) as f32;
            }
            AhdsrVolumeNodePatch::Release(val) => {
                self.release = val;
                self.release_rate =
                    self.sustain_proportion * (val.0 * self.sample_rate_recip) as f32;
            }
            AhdsrVolumeNodePatch::TriggerMode(val) => {
                self.off_state_transition = val.state_transition();
            }
            AhdsrVolumeNodePatch::RetriggerMode(val) => {
                self.on_state_transition = val.state_transition();
            }
        });

        if proc_info
            .in_silence_mask
            .all_channels_silent(buffers.inputs.len())
        {
            return ProcessStatus::ClearAllOutputs;
        }

        let mut process_range = |processor: &mut Self, range: Range<usize>| {
            // MVP per-sample interpolation. We could pre-calculate the number of samples per state as a run-length
            // encoded `ArrayVec` of states to allow for vectorization but this is good enough.
            for (input, output) in buffers.inputs.iter().zip(buffers.outputs.iter_mut()) {
                for (in_sample, out_sample) in
                    input[range.clone()].iter().zip(&mut output[range.clone()])
                {
                    *out_sample =
                        *in_sample * lerp(processor.lo, processor.hi, processor.current_smoothed);
                    processor.tick();
                }
            }
        };

        let mut event_index = 0;
        let mut buf_index = 0;

        while let Some(event) = events.get_event(event_index) {
            if let NodeEventType::CustomBytes(bytes) = &*event {
                if let Some(trig) = TimedTriggerEvent::from_bytes(bytes) {
                    let buf_range_end = (trig.sample - proc_info.clock_samples).0 as usize;

                    process_range(self, buf_index..buf_range_end);

                    match trig.event {
                        TriggerEvent::On => {
                            let (new_val, new_state) =
                                (self.on_state_transition)(self.current.target_value(), self.state);
                            self.current.set_value(new_val);
                            self.state = Some(new_state);
                        }
                        TriggerEvent::Off => {
                            if let Some(state) = &mut self.state {
                                *state = (self.off_state_transition)(*state);
                            }
                        }
                    }

                    buf_index = buf_range_end;
                }
            }

            event_index += 1;
        }

        process_range(self, buf_index..proc_info.frames);

        ProcessStatus::outputs_not_silent()
    }

    fn new_stream(&mut self, stream_info: &StreamInfo) {
        self.attack_rate = (self.attack.0 * stream_info.sample_rate_recip) as _;
        self.hold_samples = ClockSamples::from_secs_f64(self.hold.0, stream_info.sample_rate.get());
        self.decay_rate =
            (1. - self.sustain_proportion) * (self.decay.0 * stream_info.sample_rate_recip) as f32;
        self.release_rate =
            self.sustain_proportion * (self.release.0 * stream_info.sample_rate_recip) as f32;
        self.sample_rate = stream_info.sample_rate;
        self.sample_rate_recip = stream_info.sample_rate_recip;
    }
}
