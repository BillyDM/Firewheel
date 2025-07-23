use bevy_platform::time::Instant;
use core::num::NonZeroU32;
use core::ops::{Add, AddAssign, Div, DivAssign, Mul, MulAssign, Range, Sub, SubAssign};

use crate::diff::{Diff, Notify, Patch};
use crate::event::ParamData;
use crate::node::ProcInfo;

pub const MAX_PROC_TRANSPORT_KEYFRAMES: usize = 16;

/// When a particular audio event should occur, in units of absolute
/// audio clock time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EventInstant {
    /// The event should happen when the clock reaches the given time in
    /// seconds.
    ///
    /// The value is an absolute time, *NOT* a delta time. Use
    /// `FirewheelCtx::audio_clock` to get the current time of the clock.
    Seconds(InstantSeconds),

    /// The event should happen when the clock reaches the given time in
    /// samples (of a single channel of audio).
    ///
    /// The value is an absolute time, *NOT* a delta time. Use
    /// `FirewheelCtx::audio_clock` to get the current time of the clock.
    Samples(InstantSamples),

    /// The event should happen when the musical clock reaches the given
    /// musical time.
    Musical(InstantMusical),
}

impl EventInstant {
    pub fn is_musical(&self) -> bool {
        if let EventInstant::Musical(_) = self {
            true
        } else {
            false
        }
    }

    /// Convert the instant to the given time in samples.
    ///
    /// If this instant is of type [`EventInstant::Musical`] and either
    /// there is no musical transport or the musical transport is not
    /// currently playing, then this will return `None`.
    pub fn to_samples(&self, proc_info: &ProcInfo) -> Option<InstantSamples> {
        match self {
            EventInstant::Samples(samples) => Some(*samples),
            EventInstant::Seconds(seconds) => Some(seconds.to_samples(proc_info.sample_rate)),
            EventInstant::Musical(musical) => proc_info.musical_to_samples(*musical),
        }
    }
}

impl From<InstantSeconds> for EventInstant {
    fn from(value: InstantSeconds) -> Self {
        Self::Seconds(value)
    }
}

impl From<InstantSamples> for EventInstant {
    fn from(value: InstantSamples) -> Self {
        Self::Samples(value)
    }
}

impl From<InstantMusical> for EventInstant {
    fn from(value: InstantMusical) -> Self {
        Self::Musical(value)
    }
}

impl Diff for EventInstant {
    fn diff<E: crate::diff::EventQueue>(
        &self,
        baseline: &Self,
        path: crate::diff::PathBuilder,
        event_queue: &mut E,
    ) {
        if self != baseline {
            match self {
                EventInstant::Seconds(s) => event_queue.push_param(*s, path),
                EventInstant::Samples(s) => event_queue.push_param(*s, path),
                EventInstant::Musical(m) => event_queue.push_param(*m, path),
            }
        }
    }
}

impl Patch for EventInstant {
    type Patch = Self;

    fn patch(data: ParamData, _path: &[u32]) -> Result<Self::Patch, crate::diff::PatchError> {
        match data {
            ParamData::InstantSeconds(s) => Ok(EventInstant::Seconds(s)),
            ParamData::InstantSamples(s) => Ok(EventInstant::Samples(s)),
            ParamData::InstantMusical(s) => Ok(EventInstant::Musical(s)),
            _ => Err(crate::diff::PatchError::InvalidData),
        }
    }

    fn apply(&mut self, patch: Self::Patch) {
        *self = patch;
    }
}

impl Diff for Option<EventInstant> {
    fn diff<E: crate::diff::EventQueue>(
        &self,
        baseline: &Self,
        path: crate::diff::PathBuilder,
        event_queue: &mut E,
    ) {
        if self != baseline {
            match self {
                Some(EventInstant::Seconds(s)) => event_queue.push_param(*s, path),
                Some(EventInstant::Samples(s)) => event_queue.push_param(*s, path),
                Some(EventInstant::Musical(m)) => event_queue.push_param(*m, path),
                None => event_queue.push_param(ParamData::None, path),
            }
        }
    }
}

impl Patch for Option<EventInstant> {
    type Patch = Self;

    fn patch(data: ParamData, _path: &[u32]) -> Result<Self::Patch, crate::diff::PatchError> {
        match data {
            ParamData::InstantSeconds(s) => Ok(Some(EventInstant::Seconds(s))),
            ParamData::InstantSamples(s) => Ok(Some(EventInstant::Samples(s))),
            ParamData::InstantMusical(s) => Ok(Some(EventInstant::Musical(s))),
            _ => Err(crate::diff::PatchError::InvalidData),
        }
    }

    fn apply(&mut self, patch: Self::Patch) {
        *self = patch;
    }
}

/// An absolute audio clock instant in units of seconds.
#[repr(transparent)]
#[derive(Default, Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct InstantSeconds(pub f64);

impl InstantSeconds {
    pub const ZERO: Self = Self(0.0);

    pub const fn new(seconds: f64) -> Self {
        Self(seconds)
    }

    pub fn to_samples(self, sample_rate: NonZeroU32) -> InstantSamples {
        InstantSamples(seconds_to_samples(self.0, sample_rate))
    }

    /// Convert to the corresponding musical time.
    pub fn to_musical(
        self,
        transport: &MusicalTransport,
        transport_start: InstantSeconds,
    ) -> InstantMusical {
        transport.seconds_to_musical(self, transport_start)
    }

    /// Returns the amount of time elapsed from another instant to this one.
    ///
    /// If `earlier` is later than this one, then the returned value will be negative.
    pub const fn duration_since(&self, earlier: Self) -> DurationSeconds {
        DurationSeconds(self.0 - earlier.0)
    }

    /// Returns the amount of time elapsed from another instant to this one, or
    /// `None`` if that instant is later than this one.
    pub fn checked_duration_since(&self, earlier: Self) -> Option<DurationSeconds> {
        (self.0 >= earlier.0).then(|| DurationSeconds(self.0 - earlier.0))
    }

    /// Returns the amount of time elapsed from another instant to this one, or
    /// zero` if that instant is later than this one.
    pub const fn saturating_duration_since(&self, earlier: Self) -> DurationSeconds {
        DurationSeconds((self.0 - earlier.0).max(0.0))
    }
}

/// An audio clock duration in units of seconds.
#[repr(transparent)]
#[derive(Default, Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct DurationSeconds(pub f64);

impl DurationSeconds {
    pub const ZERO: Self = Self(0.0);

    pub const fn new(seconds: f64) -> Self {
        Self(seconds)
    }

    pub fn to_samples(self, sample_rate: NonZeroU32) -> DurationSamples {
        DurationSamples(seconds_to_samples(self.0, sample_rate))
    }
}

fn seconds_to_samples(seconds: f64, sample_rate: NonZeroU32) -> i64 {
    let seconds_i64 = seconds.floor() as i64;
    let fract_samples_i64 = (seconds.fract() * f64::from(sample_rate.get())).round() as i64;

    (seconds_i64 * i64::from(sample_rate.get())) + fract_samples_i64
}

impl Add<DurationSeconds> for InstantSeconds {
    type Output = InstantSeconds;
    fn add(self, rhs: DurationSeconds) -> Self::Output {
        Self(self.0 + rhs.0)
    }
}

impl Sub<DurationSeconds> for InstantSeconds {
    type Output = InstantSeconds;
    fn sub(self, rhs: DurationSeconds) -> Self::Output {
        Self(self.0 - rhs.0)
    }
}

impl AddAssign<DurationSeconds> for InstantSeconds {
    fn add_assign(&mut self, rhs: DurationSeconds) {
        *self = *self + rhs;
    }
}

impl SubAssign<DurationSeconds> for InstantSeconds {
    fn sub_assign(&mut self, rhs: DurationSeconds) {
        *self = *self - rhs;
    }
}

impl Sub<InstantSeconds> for InstantSeconds {
    type Output = DurationSeconds;
    fn sub(self, rhs: Self) -> Self::Output {
        DurationSeconds(self.0 - rhs.0)
    }
}

impl Add for DurationSeconds {
    type Output = Self;
    fn add(self, rhs: Self) -> Self::Output {
        Self(self.0 + rhs.0)
    }
}

impl Sub for DurationSeconds {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self::Output {
        Self(self.0 - rhs.0)
    }
}

impl AddAssign for DurationSeconds {
    fn add_assign(&mut self, rhs: Self) {
        self.0 += rhs.0;
    }
}

impl SubAssign for DurationSeconds {
    fn sub_assign(&mut self, rhs: Self) {
        self.0 -= rhs.0;
    }
}

impl Mul<f64> for DurationSeconds {
    type Output = Self;
    fn mul(self, rhs: f64) -> Self::Output {
        Self(self.0 * rhs)
    }
}

impl Div<f64> for DurationSeconds {
    type Output = Self;
    fn div(self, rhs: f64) -> Self::Output {
        Self(self.0 / rhs)
    }
}

impl MulAssign<f64> for DurationSeconds {
    fn mul_assign(&mut self, rhs: f64) {
        self.0 *= rhs;
    }
}

impl DivAssign<f64> for DurationSeconds {
    fn div_assign(&mut self, rhs: f64) {
        self.0 /= rhs;
    }
}

impl From<f64> for InstantSeconds {
    fn from(value: f64) -> Self {
        Self(value)
    }
}

impl Into<f64> for InstantSeconds {
    fn into(self) -> f64 {
        self.0
    }
}

impl From<f64> for DurationSeconds {
    fn from(value: f64) -> Self {
        Self(value)
    }
}

impl Into<f64> for DurationSeconds {
    fn into(self) -> f64 {
        self.0
    }
}

/// An absolute audio clock instant in units of samples (in a single channel of audio).
#[repr(transparent)]
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InstantSamples(pub i64);

impl InstantSamples {
    pub const ZERO: Self = Self(0);
    pub const MAX: Self = Self(i64::MAX);

    pub const fn new(samples: i64) -> Self {
        Self(samples)
    }

    /// (whole seconds, samples *after* whole seconds)
    pub fn whole_seconds_and_fract(&self, sample_rate: NonZeroU32) -> (i64, u32) {
        whole_seconds_and_fract(self.0, sample_rate)
    }

    pub fn fract_second_samples(&self, sample_rate: NonZeroU32) -> u32 {
        fract_second_samples(self.0, sample_rate)
    }

    pub fn to_seconds(self, sample_rate: NonZeroU32, sample_rate_recip: f64) -> InstantSeconds {
        InstantSeconds(samples_to_seconds(self.0, sample_rate, sample_rate_recip))
    }

    /// Convert to the corresponding musical time.
    pub fn to_musical(
        self,
        transport: &MusicalTransport,
        transport_start: InstantSamples,
        sample_rate: NonZeroU32,
        sample_rate_recip: f64,
    ) -> InstantMusical {
        transport.samples_to_musical(self, transport_start, sample_rate, sample_rate_recip)
    }

    /// Returns the amount of time elapsed from another instant to this one.
    ///
    /// If `earlier` is later than this one, then the returned value will be negative.
    pub const fn duration_since(&self, earlier: Self) -> DurationSamples {
        DurationSamples(self.0 - earlier.0)
    }

    /// Returns the amount of time elapsed from another instant to this one, or
    /// `None`` if that instant is later than this one.
    pub fn checked_duration_since(&self, earlier: Self) -> Option<DurationSamples> {
        (self.0 >= earlier.0).then(|| DurationSamples(self.0 - earlier.0))
    }

    /// Returns the amount of time elapsed from another instant to this one, or
    /// zero` if that instant is later than this one.
    pub fn saturating_duration_since(&self, earlier: Self) -> DurationSamples {
        DurationSamples((self.0 - earlier.0).max(0))
    }
}

/// An audio clock duration in units of samples (in a single channel of audio).
#[repr(transparent)]
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DurationSamples(pub i64);

impl DurationSamples {
    pub const ZERO: Self = Self(0);

    pub const fn new(samples: i64) -> Self {
        Self(samples)
    }

    /// (whole seconds, samples *after* whole seconds)
    pub fn whole_seconds_and_fract(&self, sample_rate: NonZeroU32) -> (i64, u32) {
        whole_seconds_and_fract(self.0, sample_rate)
    }

    pub fn fract_second_samples(&self, sample_rate: NonZeroU32) -> u32 {
        fract_second_samples(self.0, sample_rate)
    }

    pub fn to_seconds(self, sample_rate: NonZeroU32, sample_rate_recip: f64) -> DurationSeconds {
        DurationSeconds(samples_to_seconds(self.0, sample_rate, sample_rate_recip))
    }
}

/// (whole seconds, samples *after* whole seconds)
fn whole_seconds_and_fract(samples: i64, sample_rate: NonZeroU32) -> (i64, u32) {
    // Provide optimized implementations for common sample rates.
    let (whole_seconds, fract_samples) = match sample_rate.get() {
        44100 => (samples / 44100, samples % 44100),
        48000 => (samples / 48000, samples % 48000),
        sample_rate => (
            samples / i64::from(sample_rate),
            samples % i64::from(sample_rate),
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

fn fract_second_samples(samples: i64, sample_rate: NonZeroU32) -> u32 {
    match sample_rate.get() {
        44100 => (samples % 44100) as u32,
        48000 => (samples % 48000) as u32,
        sample_rate => (samples % i64::from(sample_rate)) as u32,
    }
}

fn samples_to_seconds(samples: i64, sample_rate: NonZeroU32, sample_rate_recip: f64) -> f64 {
    let (whole_seconds, fract_samples) = whole_seconds_and_fract(samples, sample_rate);
    whole_seconds as f64 + (fract_samples as f64 * sample_rate_recip)
}

impl Add<DurationSamples> for InstantSamples {
    type Output = InstantSamples;
    fn add(self, rhs: DurationSamples) -> Self::Output {
        Self(self.0 + rhs.0)
    }
}

impl Sub<DurationSamples> for InstantSamples {
    type Output = InstantSamples;
    fn sub(self, rhs: DurationSamples) -> Self::Output {
        Self(self.0 - rhs.0)
    }
}

impl AddAssign<DurationSamples> for InstantSamples {
    fn add_assign(&mut self, rhs: DurationSamples) {
        *self = *self + rhs;
    }
}

impl SubAssign<DurationSamples> for InstantSamples {
    fn sub_assign(&mut self, rhs: DurationSamples) {
        *self = *self - rhs;
    }
}

impl Sub<InstantSamples> for InstantSamples {
    type Output = DurationSamples;
    fn sub(self, rhs: Self) -> Self::Output {
        DurationSamples(self.0 - rhs.0)
    }
}

impl Add for DurationSamples {
    type Output = Self;
    fn add(self, rhs: Self) -> Self::Output {
        Self(self.0 + rhs.0)
    }
}

impl Sub for DurationSamples {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self::Output {
        Self(self.0 - rhs.0)
    }
}

impl AddAssign for DurationSamples {
    fn add_assign(&mut self, rhs: Self) {
        self.0 += rhs.0;
    }
}

impl SubAssign for DurationSamples {
    fn sub_assign(&mut self, rhs: Self) {
        self.0 -= rhs.0;
    }
}

impl Mul<i64> for DurationSamples {
    type Output = Self;
    fn mul(self, rhs: i64) -> Self::Output {
        Self(self.0 * rhs)
    }
}

impl Div<i64> for DurationSamples {
    type Output = Self;
    fn div(self, rhs: i64) -> Self::Output {
        Self(self.0 / rhs)
    }
}

impl MulAssign<i64> for DurationSamples {
    fn mul_assign(&mut self, rhs: i64) {
        self.0 *= rhs;
    }
}

impl DivAssign<i64> for DurationSamples {
    fn div_assign(&mut self, rhs: i64) {
        self.0 /= rhs;
    }
}

impl From<i64> for InstantSamples {
    fn from(value: i64) -> Self {
        Self(value)
    }
}

impl Into<i64> for InstantSamples {
    fn into(self) -> i64 {
        self.0
    }
}

impl From<i64> for DurationSamples {
    fn from(value: i64) -> Self {
        Self(value)
    }
}

impl Into<i64> for DurationSamples {
    fn into(self) -> i64 {
        self.0
    }
}

/// An absolute audio clock instant in units of musical beats.
#[derive(Default, Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct InstantMusical(pub f64);

impl InstantMusical {
    pub const ZERO: Self = Self(0.0);

    pub const fn new(beats: f64) -> Self {
        Self(beats)
    }

    /// Convert to the corresponding time in seconds.
    pub fn to_seconds(&self, beats_per_minute: f64) -> InstantSeconds {
        InstantSeconds(self.0 * 60.0 / beats_per_minute)
    }

    /// Convert to the corresponding time in samples.
    pub fn to_sample_time(&self, beats_per_minute: f64, sample_rate: NonZeroU32) -> InstantSamples {
        self.to_seconds(beats_per_minute).to_samples(sample_rate)
    }

    /// Convert to the corresponding time in seconds.
    pub fn to_seconds_with_spb(&self, seconds_per_beat: f64) -> InstantSeconds {
        InstantSeconds(self.0 * seconds_per_beat)
    }

    /// Convert to the corresponding time in samples.
    pub fn to_sample_time_with_spb(
        &self,
        seconds_per_beat: f64,
        sample_rate: NonZeroU32,
    ) -> InstantSamples {
        self.to_seconds_with_spb(seconds_per_beat)
            .to_samples(sample_rate)
    }
}

/// An audio clock duration in units of musical beats.
#[repr(transparent)]
#[derive(Default, Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct DurationMusical(pub f64);

impl DurationMusical {
    pub const ZERO: Self = Self(0.0);

    pub const fn new(beats: f64) -> Self {
        Self(beats)
    }
}

impl Add<DurationMusical> for InstantMusical {
    type Output = InstantMusical;
    fn add(self, rhs: DurationMusical) -> Self::Output {
        Self(self.0 + rhs.0)
    }
}

impl Sub<DurationMusical> for InstantMusical {
    type Output = InstantMusical;
    fn sub(self, rhs: DurationMusical) -> Self::Output {
        Self(self.0 - rhs.0)
    }
}

impl AddAssign<DurationMusical> for InstantMusical {
    fn add_assign(&mut self, rhs: DurationMusical) {
        *self = *self + rhs;
    }
}

impl SubAssign<DurationMusical> for InstantMusical {
    fn sub_assign(&mut self, rhs: DurationMusical) {
        *self = *self - rhs;
    }
}

impl Sub<InstantMusical> for InstantMusical {
    type Output = DurationMusical;
    fn sub(self, rhs: Self) -> Self::Output {
        DurationMusical(self.0 - rhs.0)
    }
}

impl Add for DurationMusical {
    type Output = Self;
    fn add(self, rhs: Self) -> Self::Output {
        Self(self.0 + rhs.0)
    }
}

impl Sub for DurationMusical {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self::Output {
        Self(self.0 - rhs.0)
    }
}

impl AddAssign for DurationMusical {
    fn add_assign(&mut self, rhs: Self) {
        self.0 += rhs.0;
    }
}

impl SubAssign for DurationMusical {
    fn sub_assign(&mut self, rhs: Self) {
        self.0 -= rhs.0;
    }
}

impl Mul<f64> for DurationMusical {
    type Output = Self;
    fn mul(self, rhs: f64) -> Self::Output {
        Self(self.0 * rhs)
    }
}

impl Div<f64> for DurationMusical {
    type Output = Self;
    fn div(self, rhs: f64) -> Self::Output {
        Self(self.0 / rhs)
    }
}

impl MulAssign<f64> for DurationMusical {
    fn mul_assign(&mut self, rhs: f64) {
        self.0 *= rhs;
    }
}

impl DivAssign<f64> for DurationMusical {
    fn div_assign(&mut self, rhs: f64) {
        self.0 /= rhs;
    }
}

impl From<f64> for InstantMusical {
    fn from(value: f64) -> Self {
        Self(value)
    }
}

impl Into<f64> for InstantMusical {
    fn into(self) -> f64 {
        self.0
    }
}

impl From<f64> for DurationMusical {
    fn from(value: f64) -> Self {
        Self(value)
    }
}

impl Into<f64> for DurationMusical {
    fn into(self) -> f64 {
        self.0
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
    pub fn musical_to_seconds(
        &self,
        musical: InstantMusical,
        transport_start: InstantSeconds,
    ) -> InstantSeconds {
        match self {
            MusicalTransport::Static(s) => s.musical_to_seconds(musical, transport_start),
        }
    }

    pub fn musical_to_samples(
        &self,
        musical: InstantMusical,
        transport_start: InstantSamples,
        sample_rate: NonZeroU32,
    ) -> InstantSamples {
        match self {
            MusicalTransport::Static(s) => {
                s.musical_to_samples(musical, transport_start, sample_rate)
            }
        }
    }

    pub fn samples_to_musical(
        &self,
        sample_time: InstantSamples,
        transport_start: InstantSamples,
        sample_rate: NonZeroU32,
        sample_rate_recip: f64,
    ) -> InstantMusical {
        match self {
            MusicalTransport::Static(s) => {
                s.samples_to_musical(sample_time, transport_start, sample_rate, sample_rate_recip)
            }
        }
    }

    pub fn seconds_to_musical(
        &self,
        seconds: InstantSeconds,
        transport_start: InstantSeconds,
    ) -> InstantMusical {
        match self {
            MusicalTransport::Static(s) => s.seconds_to_musical(seconds, transport_start),
        }
    }

    /// Return the musical time that occurs `delta_seconds` seconds after the
    /// given `from` timestamp.
    pub fn delta_seconds_from(
        &self,
        from: InstantMusical,
        delta_seconds: DurationSeconds,
    ) -> InstantMusical {
        match self {
            MusicalTransport::Static(s) => s.delta_seconds_from(from, delta_seconds),
        }
    }

    /// Return the tempo in beats per minute at the given musical time.
    pub fn bpm_at_musical(&self, _musical: InstantMusical) -> f64 {
        match self {
            MusicalTransport::Static(s) => s.beats_per_minute(),
        }
    }

    pub fn proc_transport_info(
        &self,
        frames: usize,
        _playhead: InstantMusical,
    ) -> ProcTransportInfo {
        match self {
            MusicalTransport::Static(s) => ProcTransportInfo {
                frames,
                beats_per_minute: s.beats_per_minute,
                delta_beats_per_minute: 0.0,
            },
        }
    }

    pub fn transport_start(
        &self,
        now: InstantSamples,
        playhead: InstantMusical,
        sample_rate: NonZeroU32,
    ) -> InstantSamples {
        match self {
            MusicalTransport::Static(s) => s.transport_start(now, playhead, sample_rate),
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

impl ProcTransportInfo {
    /// Get the BPM at the given frame.
    ///
    /// Returns `None` if `frame >= self.frames`.
    pub fn bpm_at_frame(&self, frame: usize) -> Option<f64> {
        (frame < self.frames)
            .then(|| self.beats_per_minute + (self.delta_beats_per_minute * frame as f64))
    }
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

    pub fn musical_to_seconds(
        &self,
        musical: InstantMusical,
        transport_start: InstantSeconds,
    ) -> InstantSeconds {
        transport_start + DurationSeconds(musical.0 * self.seconds_per_beat())
    }

    pub fn musical_to_samples(
        &self,
        musical: InstantMusical,
        transport_start: InstantSamples,
        sample_rate: NonZeroU32,
    ) -> InstantSamples {
        transport_start
            + DurationSeconds(musical.0 * self.seconds_per_beat()).to_samples(sample_rate)
    }

    pub fn samples_to_musical(
        &self,
        sample_time: InstantSamples,
        transport_start: InstantSamples,
        sample_rate: NonZeroU32,
        sample_rate_recip: f64,
    ) -> InstantMusical {
        InstantMusical(
            (sample_time - transport_start)
                .to_seconds(sample_rate, sample_rate_recip)
                .0
                * self.beats_per_second(),
        )
    }

    pub fn seconds_to_musical(
        &self,
        seconds: InstantSeconds,
        transport_start: InstantSeconds,
    ) -> InstantMusical {
        InstantMusical((seconds - transport_start).0 * self.beats_per_second())
    }

    /// Return the musical time that occurs `delta_seconds` seconds after the
    /// given `from` timestamp.
    pub fn delta_seconds_from(
        &self,
        from: InstantMusical,
        delta_seconds: DurationSeconds,
    ) -> InstantMusical {
        from + DurationMusical(delta_seconds.0 * self.beats_per_second())
    }

    pub fn transport_start(
        &self,
        now: InstantSamples,
        playhead: InstantMusical,
        sample_rate: NonZeroU32,
    ) -> InstantSamples {
        now - DurationSeconds(playhead.0 * self.seconds_per_beat()).to_samples(sample_rate)
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
    pub samples: InstantSamples,

    /// The timestamp from the audio stream, equal to the number of seconds of
    /// data that have been processed since the Firewheel context was first started.
    ///
    /// Note, this value is *NOT* synced to the system's time (`Instant::now`), and
    /// does *NOT* account for any output underflows (underruns) that may have
    /// occured. For applications where the timing of audio events is critical (i.e.
    /// a rythm game), sync the game to this audio clock.
    pub seconds: InstantSeconds,

    /// The current time of the playhead of the musical transport.
    ///
    /// If no musical transport is present, then this will be `None`.
    ///
    /// Note, this value is *NOT* synced to the system's time (`Instant::now`), and
    /// does *NOT* account for any output underflows (underruns) that may have
    /// occured. For applications where the timing of audio events is critical (i.e.
    /// a rythm game), sync the game to this audio clock.
    pub musical: Option<InstantMusical>,

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
    pub playhead: Notify<InstantMusical>,

    /// If this is `Some`, then the transport will automatically stop when the playhead
    /// reaches the given musical time.
    ///
    /// This has no effect if [`TransportState::loop_range`] is `Some`.
    pub stop_at: Option<InstantMusical>,

    /// If this is `Some`, then the transport will continously loop the given region.
    pub loop_range: Option<Range<InstantMusical>>,
}

impl Default for TransportState {
    fn default() -> Self {
        Self {
            transport: None,
            playing: Notify::new(false),
            playhead: Notify::new(InstantMusical::ZERO),
            stop_at: None,
            loop_range: None,
        }
    }
}

pub fn seconds_per_beat(beats_per_minute: f64) -> f64 {
    60.0 / beats_per_minute
}

pub fn beats_per_second(beats_per_minute: f64) -> f64 {
    beats_per_minute * (1.0 / 60.0)
}
