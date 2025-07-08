use bevy_platform::time::Instant;
use core::num::NonZeroU32;
use core::ops::{Add, AddAssign, Range, Sub, SubAssign};

use crate::{
    diff::{Diff, Patch, Notify}, node::ProcInfo,
    event::ParamData,
};

pub const MAX_PROC_TRANSPORT_KEYFRAMES: usize = 16;

/// When a particular audio event should occur.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EventDelay {
    /// The event should happen when the clock reaches the given time in
    /// seconds.
    ///
    /// The value is an absolute time, *NOT* a delta time. Use
    /// `FirewheelCtx::audio_clock` to get the current time of the clock.
    DelayUntilSeconds(ClockSeconds),

    /// The event should happen when the clock reaches the given time in
    /// samples (of a single channel of audio).
    ///
    /// The value is an absolute time, *NOT* a delta time. Use
    /// `FirewheelCtx::audio_clock` to get the current time of the clock.
    DelayUntilSamples(ClockSamples),

    /// The event should happen when the musical clock reaches the given
    /// musical time.
    DelayUntilMusical(MusicalTime),
}

impl EventDelay {
    pub fn elapsed_before_this_block(&self, proc_info: &ProcInfo) -> bool {
        match self {
            EventDelay::DelayUntilSeconds(seconds) => {
                *seconds < proc_info.audio_clock_seconds.start
            }
            EventDelay::DelayUntilSamples(samples) => {
                *samples < proc_info.audio_clock_samples.start
            }
            EventDelay::DelayUntilMusical(musical) => {
                if let Some(transport) = &proc_info.transport_info {
                    transport.playing && *musical < transport.clock_musical.start
                } else {
                    false
                }
            }
        }
    }

    pub fn elapsed_this_block(&self, proc_info: &ProcInfo) -> bool {
        match self {
            EventDelay::DelayUntilSeconds(seconds) => *seconds < proc_info.audio_clock_seconds.end,
            EventDelay::DelayUntilSamples(samples) => *samples < proc_info.audio_clock_samples.end,
            EventDelay::DelayUntilMusical(musical) => {
                if let Some(transport) = &proc_info.transport_info {
                    transport.playing && *musical < transport.clock_musical.end
                } else {
                    false
                }
            }
        }
    }

    pub fn elapsed_on_frame(&self, proc_info: &ProcInfo, sample_rate: NonZeroU32) -> Option<usize> {
        match self {
            EventDelay::DelayUntilSeconds(seconds) => {
                if *seconds <= proc_info.audio_clock_seconds.start {
                    Some(0)
                } else if *seconds >= proc_info.audio_clock_seconds.end {
                    None
                } else {
                    let frame = ((seconds.0 - proc_info.audio_clock_seconds.start.0)
                        * f64::from(sample_rate.get()))
                    .round() as usize;

                    if frame >= proc_info.frames {
                        None
                    } else {
                        Some(frame)
                    }
                }
            }
            EventDelay::DelayUntilSamples(samples) => {
                if *samples <= proc_info.audio_clock_samples.start {
                    Some(0)
                } else if *samples >= proc_info.audio_clock_samples.end {
                    None
                } else {
                    Some((*samples - proc_info.audio_clock_samples.start).0 as usize)
                }
            }
            EventDelay::DelayUntilMusical(musical) => {
                if let Some(transport) = &proc_info.transport_info {
                    if !transport.playing || *musical >= transport.clock_musical.end {
                        None
                    } else if *musical <= transport.clock_musical.start {
                        Some(0)
                    } else {
                        let frame = transport
                            .transport
                            .musical_to_samples(*musical, sample_rate)
                            - proc_info.audio_clock_samples.start;

                        if frame.0 >= proc_info.frames as i64 {
                            None
                        } else {
                            Some(frame.0 as usize)
                        }
                    }
                } else {
                    None
                }
            }
        }
    }
}

/// An absolute clock time in units of seconds.
#[repr(transparent)]
#[derive(Default, Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct ClockSeconds(pub f64);

impl ClockSeconds {
    pub fn to_samples(self, sample_rate: NonZeroU32) -> ClockSamples {
        let seconds_i64 = self.0.floor() as i64;
        let fract_samples_i64 = (self.0.fract() * f64::from(sample_rate.get())).round() as i64;

        ClockSamples((seconds_i64 * i64::from(sample_rate.get())) + fract_samples_i64)
    }

    /// Convert to the corresponding musical time.
    pub fn to_musical(self, transport: &MusicalTransport) -> MusicalTime {
        transport.seconds_to_musical(self)
    }
}

impl Add for ClockSeconds {
    type Output = Self;
    fn add(self, rhs: Self) -> Self::Output {
        Self(self.0 + rhs.0)
    }
}

impl Sub for ClockSeconds {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self::Output {
        Self(self.0 - rhs.0)
    }
}

impl AddAssign for ClockSeconds {
    fn add_assign(&mut self, rhs: Self) {
        self.0 += rhs.0;
    }
}

impl SubAssign for ClockSeconds {
    fn sub_assign(&mut self, rhs: Self) {
        self.0 -= rhs.0;
    }
}

impl From<f64> for ClockSeconds {
    fn from(value: f64) -> Self {
        Self(value)
    }
}

impl Into<f64> for ClockSeconds {
    fn into(self) -> f64 {
        self.0
    }
}

impl TryFrom<&ParamData> for ClockSeconds {
    type Error = crate::diff::PatchError;

    fn try_from(value: &ParamData) -> Result<Self, Self::Error> {
        value
            .try_into()
            .map_err(|_| crate::diff::PatchError::InvalidData)
            .map(ClockSeconds)
    }
}

impl From<ClockSeconds> for ParamData {
    fn from(value: ClockSeconds) -> Self {
        value.0.into()
    }
}

impl Diff for ClockSeconds {
    fn diff<E: crate::diff::EventQueue>(
        &self,
        baseline: &Self,
        path: crate::diff::PathBuilder,
        event_queue: &mut E,
    ) {
        if self != baseline {
            event_queue.push_param(*self, path);
        }
    }
}

impl Patch for ClockSeconds {
    type Patch = Self;

    fn patch(data: &ParamData, _path: &[u32]) -> Result<Self::Patch, crate::diff::PatchError> {
        data.try_into()
    }

    fn apply(&mut self, patch: Self::Patch) {
        *self = patch;
    }
}

/// An absolute clock time in units of samples (in a single channel of audio).
#[repr(transparent)]
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ClockSamples(pub i64);

impl ClockSamples {
    pub const fn new(samples: i64) -> Self {
        Self(samples)
    }

    /// (whole seconds, samples *after* whole seconds)
    pub fn whole_seconds_and_fract(&self, sample_rate: NonZeroU32) -> (i64, u32) {
        // Provide optimized implementations for common sample rates.
        let (whole_seconds, fract_samples) = match sample_rate.get() {
            44100 => (self.0 / 44100, self.0 % 44100),
            48000 => (self.0 / 48000, self.0 % 48000),
            sample_rate => (
                self.0 / i64::from(sample_rate),
                self.0 % i64::from(sample_rate),
            ),
        };

        if fract_samples < 0 {
            (
                whole_seconds - 1,
                sample_rate.get() - (fract_samples.abs() as u32),
            )
        } else {
            (whole_seconds, fract_samples as u32)
        }
    }

    pub fn fract_second_samples(&self, sample_rate: NonZeroU32) -> u32 {
        match sample_rate.get() {
            44100 => (self.0 % 44100) as u32,
            48000 => (self.0 % 48000) as u32,
            sample_rate => (self.0 % i64::from(sample_rate)) as u32,
        }
    }

    pub fn to_seconds(self, sample_rate: NonZeroU32, sample_rate_recip: f64) -> ClockSeconds {
        // Provide optimized implementations for common sample rates.
        let (whole_seconds, fract_samples) = self.whole_seconds_and_fract(sample_rate);

        ClockSeconds(whole_seconds as f64 + (fract_samples as f64 * sample_rate_recip))
    }

    /// Convert to the corresponding musical time.
    pub fn to_musical(
        self,
        transport: &MusicalTransport,
        sample_rate: NonZeroU32,
        sample_rate_recip: f64,
    ) -> MusicalTime {
        transport.samples_to_musical(self, sample_rate, sample_rate_recip)
    }
}

impl Add for ClockSamples {
    type Output = Self;
    fn add(self, rhs: Self) -> Self::Output {
        Self(self.0 + rhs.0)
    }
}

impl Sub for ClockSamples {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self::Output {
        Self(self.0 - rhs.0)
    }
}

impl AddAssign for ClockSamples {
    fn add_assign(&mut self, rhs: Self) {
        self.0 += rhs.0;
    }
}

impl SubAssign for ClockSamples {
    fn sub_assign(&mut self, rhs: Self) {
        self.0 -= rhs.0;
    }
}

impl From<i64> for ClockSamples {
    fn from(value: i64) -> Self {
        Self(value)
    }
}

impl Into<i64> for ClockSamples {
    fn into(self) -> i64 {
        self.0
    }
}

impl TryFrom<&ParamData> for ClockSamples {
    type Error = crate::diff::PatchError;

    fn try_from(value: &ParamData) -> Result<Self, Self::Error> {
        value
            .try_into()
            .map_err(|_| crate::diff::PatchError::InvalidData)
            .map(ClockSamples)
    }
}

impl From<ClockSamples> for ParamData {
    fn from(value: ClockSamples) -> Self {
        value.0.into()
    }
}

impl Diff for ClockSamples {
    fn diff<E: crate::diff::EventQueue>(
        &self,
        baseline: &Self,
        path: crate::diff::PathBuilder,
        event_queue: &mut E,
    ) {
        if self != baseline {
            event_queue.push_param(*self, path);
        }
    }
}

impl Patch for ClockSamples {
    type Patch = Self;

    fn patch(data: &ParamData, _path: &[u32]) -> Result<Self::Patch, crate::diff::PatchError> {
        data.try_into()
    }

    fn apply(&mut self, patch: Self::Patch) {
        *self = patch;
    }
}

/// Musical time in units of beats.
#[derive(Default, Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct MusicalTime(pub f64);

impl MusicalTime {
    pub const ZERO: Self = Self(0.0);

    pub const fn new(beats: f64) -> Self {
        Self(beats)
    }

    /// Convert to the corresponding time in seconds.
    pub fn to_seconds(&self, beats_per_minute: f64) -> ClockSeconds {
        ClockSeconds(self.0 * 60.0 / beats_per_minute)
    }

    /// Convert to the corresponding time in samples.
    pub fn to_sample_time(&self, beats_per_minute: f64, sample_rate: NonZeroU32) -> ClockSamples {
        self.to_seconds(beats_per_minute).to_samples(sample_rate)
    }

    /// Convert to the corresponding time in seconds.
    pub fn to_seconds_with_spb(&self, seconds_per_beat: f64) -> ClockSeconds {
        ClockSeconds(self.0 * seconds_per_beat)
    }

    /// Convert to the corresponding time in samples.
    pub fn to_sample_time_with_spb(
        &self,
        seconds_per_beat: f64,
        sample_rate: NonZeroU32,
    ) -> ClockSamples {
        self.to_seconds_with_spb(seconds_per_beat)
            .to_samples(sample_rate)
    }
}

pub fn seconds_per_beat(beats_per_minute: f64) -> f64 {
    60.0 / beats_per_minute
}

pub fn beats_per_second(beats_per_minute: f64) -> f64 {
    beats_per_minute * (1.0 / 60.0)
}

impl Add for MusicalTime {
    type Output = Self;
    fn add(self, rhs: Self) -> Self::Output {
        Self(self.0 + rhs.0)
    }
}

impl Sub for MusicalTime {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self::Output {
        Self(self.0 - rhs.0)
    }
}

impl AddAssign for MusicalTime {
    fn add_assign(&mut self, rhs: Self) {
        self.0 += rhs.0;
    }
}

impl SubAssign for MusicalTime {
    fn sub_assign(&mut self, rhs: Self) {
        self.0 -= rhs.0;
    }
}

#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MusicalTransport {
    Static(StaticTransport),
    // TODO: Linearly automated tempo.
}

impl Default for MusicalTransport {
    fn default() -> Self {
        Self::Static(StaticTransport::default())
    }
}

impl MusicalTransport {
    pub fn musical_to_seconds(&self, musical: MusicalTime) -> ClockSeconds {
        match self {
            MusicalTransport::Static(s) => s.musical_to_seconds(musical),
        }
    }

    pub fn musical_to_samples(
        &self,
        musical: MusicalTime,
        sample_rate: NonZeroU32,
    ) -> ClockSamples {
        match self {
            MusicalTransport::Static(s) => s.musical_to_samples(musical, sample_rate),
        }
    }

    pub fn samples_to_musical(
        &self,
        sample_time: ClockSamples,
        sample_rate: NonZeroU32,
        sample_rate_recip: f64,
    ) -> MusicalTime {
        match self {
            MusicalTransport::Static(s) => {
                s.samples_to_musical(sample_time, sample_rate, sample_rate_recip)
            }
        }
    }

    pub fn seconds_to_musical(&self, seconds: ClockSeconds) -> MusicalTime {
        match self {
            MusicalTransport::Static(s) => s.seconds_to_musical(seconds),
        }
    }

    /// Return the musical time that occurs `delta_seconds` seconds after the
    /// given `from` timestamp.
    pub fn delta_seconds_from(
        &self,
        from: MusicalTime,
        delta_seconds: ClockSeconds,
    ) -> MusicalTime {
        match self {
            MusicalTransport::Static(s) => s.delta_seconds_from(from, delta_seconds),
        }
    }

    /// Return the tempo in beats per minute at the given musical time.
    pub fn bpm_at_musical(&self, _musical: MusicalTime) -> f64 {
        match self {
            MusicalTransport::Static(s) => s.beats_per_minute(),
        }
    }

    pub fn proc_transport_info(&self, frames: usize, _playhead: MusicalTime) -> ProcTransportInfo {
        match self {
            MusicalTransport::Static(s) => ProcTransportInfo {
                frames,
                beats_per_minute: s.beats_per_minute,
                delta_beats_per_minute: 0.0,
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProcTransportInfo {
    /// The number of frames in this processing block that this information
    /// lasts for before it changes.
    pub frames: usize,

    /// The beats per minute at the first frame of this process block.
    pub beats_per_minute: f64,

    /// The rate at which `beats_per_minute` changes each frame in this
    /// processing block.
    ///
    /// For example, if this value is `0.0`, then the bpm remains static for
    /// the entire duration of this processing block.
    ///
    /// And for example, if this is `0.1`, then the bpm increases by `0.1`
    /// each frame, and if this is `-0.1`, then the bpm decreased by `0.1`
    /// each frame.
    pub delta_beats_per_minute: f64,
}

/// A musical transport with a single static tempo in beats per minute.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StaticTransport {
    pub beats_per_minute: f64,
}

impl Default for StaticTransport {
    fn default() -> Self {
        Self {
            beats_per_minute: 110.0,
        }
    }
}

impl StaticTransport {
    pub const fn new(beats_per_minute: f64) -> Self {
        Self { beats_per_minute }
    }

    pub const fn beats_per_minute(&self) -> f64 {
        self.beats_per_minute
    }

    pub fn seconds_per_beat(&self) -> f64 {
        60.0 / self.beats_per_minute
    }

    pub fn beats_per_second(&self) -> f64 {
        self.beats_per_minute * (1.0 / 60.0)
    }

    pub fn musical_to_seconds(&self, musical: MusicalTime) -> ClockSeconds {
        ClockSeconds(musical.0 * self.seconds_per_beat())
    }

    pub fn musical_to_samples(
        &self,
        musical: MusicalTime,
        sample_rate: NonZeroU32,
    ) -> ClockSamples {
        self.musical_to_seconds(musical).to_samples(sample_rate)
    }

    pub fn samples_to_musical(
        &self,
        sample_time: ClockSamples,
        sample_rate: NonZeroU32,
        sample_rate_recip: f64,
    ) -> MusicalTime {
        self.seconds_to_musical(sample_time.to_seconds(sample_rate, sample_rate_recip))
    }

    pub fn seconds_to_musical(&self, seconds: ClockSeconds) -> MusicalTime {
        MusicalTime(seconds.0 * self.beats_per_second())
    }

    /// Return the musical time that occurs `delta_seconds` seconds after the
    /// given `from` timestamp.
    pub fn delta_seconds_from(
        &self,
        from: MusicalTime,
        delta_seconds: ClockSeconds,
    ) -> MusicalTime {
        from + self.seconds_to_musical(delta_seconds)
    }
}

/* This has turned out to be quite complex, so I'll finish this later in a separate PR.
//
/// A musical transport with linearly automated tempo.
#[derive(Clone)]
pub struct AutomatedTransport {
    keyframes: Vec<TransportKeyframe>,
    // Contains the start time in seconds for each keyframe.
    cache: Vec<ClockSeconds>,
}

impl Default for AutomatedTransport {
    fn default() -> Self {
        Self {
            keyframes: vec![TransportKeyframe::default()],
            cache: vec![ClockSeconds(0.0)],
        }
    }
}

impl AutomatedTransport {
    pub fn new(keyframes: Vec<TransportKeyframe>) -> Result<Self, DynamicTransportError> {
        // Check prequisits.
        if keyframes.is_empty() {
            return Err(DynamicTransportError::Empty);
        }

        let mut prev_time = MusicalTime(f64::NEG_INFINITY);
        for k in keyframes.iter() {
            if prev_time >= k.time {
                return Err(DynamicTransportError::ImproperOrdering);
            }
            if k.beats_per_minute <= 0.0 {
                return Err(DynamicTransportError::InvalidBPM(k.beats_per_minute));
            }

            prev_time = k.time;
        }

        let mut cache = Vec::with_capacity(keyframes.len());
        cache.push(ClockSeconds(0.0));

        let mut seconds = ClockSeconds(0.0);
        for (i, k) in keyframes.iter().enumerate().skip(1) {
            let prev_k = &keyframes[i - 1];

            let delta_seconds = if prev_k.interpolate_to_next {
                linear_interp_bpm_to_delta_seconds(
                    prev_k.time,
                    k.time,
                    prev_k.beats_per_minute,
                    k.beats_per_minute,
                )
            } else {
                (k.time - prev_k.time).to_seconds(prev_k.beats_per_minute)
            };

            seconds += delta_seconds;
            cache.push(seconds);
        }

        Ok(Self { keyframes, cache })
    }

    pub fn musical_to_seconds(&self, musical: MusicalTime) -> ClockSeconds {
        ClockSeconds(musical.0 * self.seconds_per_beat())
    }

    pub fn musical_to_samples(
        &self,
        musical: MusicalTime,
        sample_rate: NonZeroU32,
    ) -> ClockSamples {
        self.musical_to_seconds(musical).to_samples(sample_rate)
    }

    pub fn samples_to_musical(
        &self,
        sample_time: ClockSamples,
        sample_rate: NonZeroU32,
        sample_rate_recip: f64,
    ) -> MusicalTime {
        self.seconds_to_musical(sample_time.to_seconds(sample_rate, sample_rate_recip))
    }

    pub fn seconds_to_musical(&self, seconds: ClockSeconds) -> MusicalTime {
        MusicalTime(seconds.0 * self.beats_per_second())
    }

    /// Return the musical time that occurs `delta_seconds` seconds after the
    /// given `from` timestamp.
    pub fn delta_seconds_from(
        &self,
        from: MusicalTime,
        delta_seconds: ClockSeconds,
    ) -> MusicalTime {
        from + self.seconds_to_musical(delta_seconds)
    }
}

fn linear_interp_bpm_to_delta_seconds(
    from_beat: MusicalTime,
    to_beat: MusicalTime,
    from_bpm: f64,
    to_bpm: f64,
) -> ClockSeconds {
    // This can be solved with the standard kinematic equation of displacement
    //
    // delta_x = (v0 * t) + (1/2 * a * t^2)
    //
    // where
    // x = minutes
    // v = 1/bpm = mpb
    // t = beats1 - beats0 = delta_beats
    // a = (mpb1 - mbp0) / (beats1 - beats0) = delta_mpb / delta_beats
    //
    // which gives us
    //
    // delta_minutes = (mpb0 * delta_beats) + (1/2 * (delta_mpb / delta_beats) * delta_beats^2)
    //
    // and simplifies to:
    //
    // delta_minutes = (mpb0 * delta_beats) + (1/2 * delta_mpb * delta_beats)

    let delta_beats = (to_beat - from_beat).0;

    let from_mpb = from_bpm.recip();
    let delta_mpb = to_bpm.recip() - from_mpb;

    let delta_minutes = (from_mpb * delta_beats) + (0.5 * delta_mpb * delta_beats);

    ClockSeconds(delta_minutes * 60.0)
}

#[derive(Debug, thiserror::Error)]
pub enum DynamicTransportError {
    /// The dynamic tranport contained no keyframes.
    #[error("The dynamic transport contained no keyframes")]
    Empty,
    /// The keyframes of the dynamic transport are not properly ordered.
    #[error("The keyframes of the dynamic transport are not properly ordered")]
    ImproperOrdering,
    /// Invalid BPM value.
    #[error("Invalid beats per minute value: {0}")]
    InvalidBPM(f64),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TransportKeyframe {
    /// The musical time at which this keyframe occurs.
    pub time: MusicalTime,

    /// The beats per minute at the start of this keyframe.
    pub beats_per_minute: f64,

    /// If `true`, then the bpm will linearly interpolate from this keyframe to the
    /// next keyframe. If `false`, then the bpm will stay static until right before
    /// the next keyframe, and then immediately jump to the next keyframe.
    pub interpolate_to_next: bool,
}

impl Default for TransportKeyframe {
    fn default() -> Self {
        Self {
            time: MusicalTime::ZERO,
            beats_per_minute: 110.0,
            interpolate_to_next: false,
        }
    }
}
*/

/// The time of the internal audio clock.
///
/// Note, due to the nature of audio processing, this clock is is *NOT* synced with
/// the system's time (`Instant::now`). (Instead it is based on the amount of data
/// that has been processed.) For applications where the timing of audio events is
/// critical (i.e. a rythm game), sync the game to this audio clock instead of the
/// OS's clock (`Instant::now()`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AudioClock {
    /// The timestamp from the audio stream, equal to the number of frames
    /// (samples in a single channel of audio) of data that have been processed
    /// since the Firewheel context was first started.
    ///
    /// Note, generally this value will always count up, but there may be a
    /// few edge cases that cause this value to be less than the previous call,
    /// such as when the sample rate of the stream has been changed.
    ///
    /// Note, this value is *NOT* synced to the system's time (`Instant::now`), and
    /// does *NOT* account for any output underflows (underruns) that may have
    /// occured. For applications where the timing of audio events is critical (i.e.
    /// a rythm game), sync the game to this audio clock.
    pub samples: ClockSamples,

    /// The timestamp from the audio stream, equal to the number of seconds of
    /// data that have been processed since the Firewheel context was first started.
    ///
    /// Note, this value is *NOT* synced to the system's time (`Instant::now`), and
    /// does *NOT* account for any output underflows (underruns) that may have
    /// occured. For applications where the timing of audio events is critical (i.e.
    /// a rythm game), sync the game to this audio clock.
    pub seconds: ClockSeconds,

    /// The current time of the playhead of the musical transport.
    ///
    /// If no musical transport is present, then this will be `None`.
    ///
    /// Note, this value is *NOT* synced to the system's time (`Instant::now`), and
    /// does *NOT* account for any output underflows (underruns) that may have
    /// occured. For applications where the timing of audio events is critical (i.e.
    /// a rythm game), sync the game to this audio clock.
    pub musical: Option<MusicalTime>,

    /// This is `true` if a musical transport is present and it is not paused,
    /// `false` otherwise.
    pub transport_is_playing: bool,

    /// The instant the audio clock was last updated.
    ///
    /// If the audio thread is not currently running, then this will be `None`.
    ///
    /// Note, if this was returned via `FirewheelCtx::audio_clock_corrected()`, then
    /// `samples`, `seconds`, and `musical` have already taken this delay into
    /// account.
    pub update_instant: Option<Instant>,
}

/// The state of the musical transport in a Firewheel context.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "bevy", derive(bevy_ecs::prelude::Component))]
pub struct TransportState {
    /// The current musical transport.
    pub transport: Option<MusicalTransport>,

    /// Whether or not the musical transport is playing (true) or is paused (false).
    pub playing: Notify<bool>,

    /// The playhead of the musical transport.
    pub playhead: Notify<MusicalTime>,

    /// If this is `Some`, then the transport will automatically stop when the playhead
    /// reaches the given musical time.
    ///
    /// This has no effect if [`TransportState::loop_range`] is `Some`.
    pub stop_at: Option<MusicalTime>,

    /// If this is `Some`, then the transport will continously loop the given region.
    pub loop_range: Option<Range<MusicalTime>>,
}

impl Default for TransportState {
    fn default() -> Self {
        Self {
            transport: None,
            playing: Notify::new(false),
            playhead: Notify::new(MusicalTime::ZERO),
            stop_at: None,
            loop_range: None,
        }
    }
}
