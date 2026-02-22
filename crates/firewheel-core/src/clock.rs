#[cfg(not(feature = "std"))]
use num_traits::Float;

use bevy_platform::time::Instant;
use core::num::NonZeroU32;
use core::ops::{Add, AddAssign, Div, DivAssign, Mul, MulAssign, Sub, SubAssign};

#[cfg(feature = "scheduled_events")]
use crate::diff::{Diff, Patch};
#[cfg(feature = "scheduled_events")]
use crate::event::ParamData;
#[cfg(feature = "scheduled_events")]
use crate::node::ProcInfo;

#[cfg(feature = "musical_transport")]
mod transport;
#[cfg(feature = "musical_transport")]
pub use transport::*;

/// When a particular audio event should occur, in units of absolute
/// audio clock time.
#[cfg(feature = "scheduled_events")]
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "bevy_reflect", derive(bevy_reflect::Reflect))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
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
    #[cfg(feature = "musical_transport")]
    Musical(InstantMusical),
}

#[cfg(feature = "scheduled_events")]
impl EventInstant {
    pub fn is_musical(&self) -> bool {
        #[cfg(feature = "musical_transport")]
        if let EventInstant::Musical(_) = self {
            return true;
        } else {
            return false;
        }

        #[cfg(not(feature = "musical_transport"))]
        return false;
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
            #[cfg(feature = "musical_transport")]
            EventInstant::Musical(musical) => proc_info.musical_to_samples(*musical),
        }
    }
}

#[cfg(feature = "scheduled_events")]
impl From<InstantSeconds> for EventInstant {
    fn from(value: InstantSeconds) -> Self {
        Self::Seconds(value)
    }
}

#[cfg(feature = "scheduled_events")]
impl From<InstantSamples> for EventInstant {
    fn from(value: InstantSamples) -> Self {
        Self::Samples(value)
    }
}

#[cfg(feature = "musical_transport")]
impl From<InstantMusical> for EventInstant {
    fn from(value: InstantMusical) -> Self {
        Self::Musical(value)
    }
}

#[cfg(feature = "scheduled_events")]
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
                #[cfg(feature = "musical_transport")]
                EventInstant::Musical(m) => event_queue.push_param(*m, path),
            }
        }
    }
}

#[cfg(feature = "scheduled_events")]
impl Patch for EventInstant {
    type Patch = Self;

    fn patch(data: &ParamData, _path: &[u32]) -> Result<Self::Patch, crate::diff::PatchError> {
        match data {
            ParamData::InstantSeconds(s) => Ok(EventInstant::Seconds(*s)),
            ParamData::InstantSamples(s) => Ok(EventInstant::Samples(*s)),
            #[cfg(feature = "musical_transport")]
            ParamData::InstantMusical(s) => Ok(EventInstant::Musical(*s)),
            _ => Err(crate::diff::PatchError::InvalidData),
        }
    }

    fn apply(&mut self, patch: Self::Patch) {
        *self = patch;
    }
}

#[cfg(feature = "scheduled_events")]
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
                #[cfg(feature = "musical_transport")]
                Some(EventInstant::Musical(m)) => event_queue.push_param(*m, path),
                None => event_queue.push_param(ParamData::None, path),
            }
        }
    }
}

#[cfg(feature = "scheduled_events")]
impl Patch for Option<EventInstant> {
    type Patch = Self;

    fn patch(data: &ParamData, _path: &[u32]) -> Result<Self::Patch, crate::diff::PatchError> {
        match data {
            ParamData::InstantSeconds(s) => Ok(Some(EventInstant::Seconds(*s))),
            ParamData::InstantSamples(s) => Ok(Some(EventInstant::Samples(*s))),
            #[cfg(feature = "musical_transport")]
            ParamData::InstantMusical(s) => Ok(Some(EventInstant::Musical(*s))),
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
#[cfg_attr(feature = "bevy_reflect", derive(bevy_reflect::Reflect))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
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
    #[cfg(feature = "musical_transport")]
    pub fn to_musical(
        self,
        transport: &MusicalTransport,
        transport_start: InstantSeconds,
        speed_multiplier: f64,
    ) -> InstantMusical {
        transport.seconds_to_musical(self, transport_start, speed_multiplier)
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
        (self.0 >= earlier.0).then_some(DurationSeconds(self.0 - earlier.0))
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
#[cfg_attr(feature = "bevy_reflect", derive(bevy_reflect::Reflect))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
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

impl From<InstantSeconds> for f64 {
    fn from(value: InstantSeconds) -> Self {
        value.0
    }
}

impl From<f64> for DurationSeconds {
    fn from(value: f64) -> Self {
        Self(value)
    }
}

impl From<DurationSeconds> for f64 {
    fn from(value: DurationSeconds) -> Self {
        value.0
    }
}

/// An absolute audio clock instant in units of samples (in a single channel of audio).
#[repr(transparent)]
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "bevy_reflect", derive(bevy_reflect::Reflect))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
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
    #[cfg(feature = "musical_transport")]
    pub fn to_musical(
        self,
        transport: &MusicalTransport,
        transport_start: InstantSamples,
        speed_multiplier: f64,
        sample_rate: NonZeroU32,
        sample_rate_recip: f64,
    ) -> InstantMusical {
        transport.samples_to_musical(
            self,
            transport_start,
            speed_multiplier,
            sample_rate,
            sample_rate_recip,
        )
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
#[cfg_attr(feature = "bevy_reflect", derive(bevy_reflect::Reflect))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
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
            sample_rate.get() - (fract_samples.unsigned_abs() as u32),
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

impl From<InstantSamples> for i64 {
    fn from(value: InstantSamples) -> Self {
        value.0
    }
}

impl From<i64> for DurationSamples {
    fn from(value: i64) -> Self {
        Self(value)
    }
}

impl From<DurationSamples> for i64 {
    fn from(value: DurationSamples) -> Self {
        value.0
    }
}

/// An absolute audio clock instant in units of musical beats.
#[derive(Default, Debug, Clone, Copy, PartialEq, PartialOrd)]
#[cfg_attr(feature = "bevy_reflect", derive(bevy_reflect::Reflect))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg(feature = "musical_transport")]
pub struct InstantMusical(pub f64);

#[cfg(feature = "musical_transport")]
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
#[cfg_attr(feature = "bevy_reflect", derive(bevy_reflect::Reflect))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg(feature = "musical_transport")]
pub struct DurationMusical(pub f64);

#[cfg(feature = "musical_transport")]
impl DurationMusical {
    pub const ZERO: Self = Self(0.0);

    pub const fn new(beats: f64) -> Self {
        Self(beats)
    }
}

#[cfg(feature = "musical_transport")]
impl Add<DurationMusical> for InstantMusical {
    type Output = InstantMusical;
    fn add(self, rhs: DurationMusical) -> Self::Output {
        Self(self.0 + rhs.0)
    }
}

#[cfg(feature = "musical_transport")]
impl Sub<DurationMusical> for InstantMusical {
    type Output = InstantMusical;
    fn sub(self, rhs: DurationMusical) -> Self::Output {
        Self(self.0 - rhs.0)
    }
}

#[cfg(feature = "musical_transport")]
impl AddAssign<DurationMusical> for InstantMusical {
    fn add_assign(&mut self, rhs: DurationMusical) {
        *self = *self + rhs;
    }
}

#[cfg(feature = "musical_transport")]
impl SubAssign<DurationMusical> for InstantMusical {
    fn sub_assign(&mut self, rhs: DurationMusical) {
        *self = *self - rhs;
    }
}

#[cfg(feature = "musical_transport")]
impl Sub<InstantMusical> for InstantMusical {
    type Output = DurationMusical;
    fn sub(self, rhs: Self) -> Self::Output {
        DurationMusical(self.0 - rhs.0)
    }
}

#[cfg(feature = "musical_transport")]
impl Add for DurationMusical {
    type Output = Self;
    fn add(self, rhs: Self) -> Self::Output {
        Self(self.0 + rhs.0)
    }
}

#[cfg(feature = "musical_transport")]
impl Sub for DurationMusical {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self::Output {
        Self(self.0 - rhs.0)
    }
}

#[cfg(feature = "musical_transport")]
impl AddAssign for DurationMusical {
    fn add_assign(&mut self, rhs: Self) {
        self.0 += rhs.0;
    }
}

#[cfg(feature = "musical_transport")]
impl SubAssign for DurationMusical {
    fn sub_assign(&mut self, rhs: Self) {
        self.0 -= rhs.0;
    }
}

#[cfg(feature = "musical_transport")]
impl Mul<f64> for DurationMusical {
    type Output = Self;
    fn mul(self, rhs: f64) -> Self::Output {
        Self(self.0 * rhs)
    }
}

#[cfg(feature = "musical_transport")]
impl Div<f64> for DurationMusical {
    type Output = Self;
    fn div(self, rhs: f64) -> Self::Output {
        Self(self.0 / rhs)
    }
}

#[cfg(feature = "musical_transport")]
impl MulAssign<f64> for DurationMusical {
    fn mul_assign(&mut self, rhs: f64) {
        self.0 *= rhs;
    }
}

#[cfg(feature = "musical_transport")]
impl DivAssign<f64> for DurationMusical {
    fn div_assign(&mut self, rhs: f64) {
        self.0 /= rhs;
    }
}

#[cfg(feature = "musical_transport")]
impl From<f64> for InstantMusical {
    fn from(value: f64) -> Self {
        Self(value)
    }
}

#[cfg(feature = "musical_transport")]
impl From<InstantMusical> for f64 {
    fn from(value: InstantMusical) -> Self {
        value.0
    }
}

#[cfg(feature = "musical_transport")]
impl From<f64> for DurationMusical {
    fn from(value: f64) -> Self {
        Self(value)
    }
}

#[cfg(feature = "musical_transport")]
impl From<DurationMusical> for f64 {
    fn from(value: DurationMusical) -> Self {
        value.0
    }
}

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
    #[cfg(feature = "musical_transport")]
    pub musical: Option<InstantMusical>,

    /// This is `true` if a musical transport is present and it is not paused,
    /// `false` otherwise.
    #[cfg(feature = "musical_transport")]
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
