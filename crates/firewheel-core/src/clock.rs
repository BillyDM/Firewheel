use core::ops::{Add, AddAssign, Sub, SubAssign};

use crate::{diff::Notify, node::ProcInfo};

/// When a particular audio event should occur.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EventDelay {
    /// The event should happen when the clock reaches the given time in
    /// seconds.
    ///
    /// The value is an absolute time, *NOT* a delta time. Use the context's
    /// `clock_now` method to get the current time of the clock.
    DelayUntilSeconds(ClockSeconds),

    /// The event should happen when the clock reaches the given time in
    /// samples (of a single channel of audio).
    ///
    /// The value is an absolute time, *NOT* a delta time. Use the context's
    /// `clock_samples` to get the current time of the clock.
    DelayUntilSamples(ClockSamples),

    /// The event should happen when the musical clock reaches the given
    /// musical time.
    ///
    /// Like `DelayUntilSamples`, this is very accurate, but note it also
    /// does not account for any output underflows that may occur.
    DelayUntilMusical(MusicalTime),
}

impl EventDelay {
    pub fn elapsed_before_this_block(&self, proc_info: &ProcInfo) -> bool {
        match self {
            EventDelay::DelayUntilSeconds(seconds) => *seconds < proc_info.clock_seconds.start,
            EventDelay::DelayUntilSamples(samples) => *samples < proc_info.clock_samples.start,
            EventDelay::DelayUntilMusical(musical) => {
                if let Some(transport) = &proc_info.transport_info {
                    transport.playing && *musical < transport.musical_clock.start
                } else {
                    false
                }
            }
        }
    }

    pub fn elapsed_this_block(&self, proc_info: &ProcInfo) -> bool {
        match self {
            EventDelay::DelayUntilSeconds(seconds) => *seconds < proc_info.clock_seconds.end,
            EventDelay::DelayUntilSamples(samples) => *samples < proc_info.clock_samples.end,
            EventDelay::DelayUntilMusical(musical) => {
                if let Some(transport) = &proc_info.transport_info {
                    transport.playing && *musical < transport.musical_clock.end
                } else {
                    false
                }
            }
        }
    }

    pub fn elapsed_on_frame(&self, proc_info: &ProcInfo, sample_rate: u32) -> Option<usize> {
        match self {
            EventDelay::DelayUntilSeconds(seconds) => {
                if *seconds <= proc_info.clock_seconds.start {
                    Some(0)
                } else if *seconds >= proc_info.clock_seconds.end {
                    None
                } else {
                    let frame = ((seconds.0 - proc_info.clock_seconds.start.0)
                        * f64::from(sample_rate))
                    .round() as usize;

                    if frame >= proc_info.frames {
                        None
                    } else {
                        Some(frame)
                    }
                }
            }
            EventDelay::DelayUntilSamples(samples) => {
                if *samples <= proc_info.clock_samples.start {
                    Some(0)
                } else if *samples >= proc_info.clock_samples.end {
                    None
                } else {
                    Some((*samples - proc_info.clock_samples.start).0 as usize)
                }
            }
            EventDelay::DelayUntilMusical(musical) => {
                if let Some(transport) = &proc_info.transport_info {
                    if !transport.playing || *musical >= transport.musical_clock.end {
                        None
                    } else if *musical <= transport.musical_clock.start {
                        Some(0)
                    } else {
                        let frame = transport.transport.musical_to_sample(*musical, sample_rate)
                            - proc_info.clock_samples.start;

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
    pub fn to_samples(self, sample_rate: u32) -> ClockSamples {
        let seconds_i64 = self.0.floor() as i64;
        let fract_samples_i64 = (self.0.fract() * f64::from(sample_rate)).round() as i64;

        ClockSamples((seconds_i64 * i64::from(sample_rate)) + fract_samples_i64)
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

/// An absolute clock time in units of samples (in a single channel of audio).
#[repr(transparent)]
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ClockSamples(pub i64);

impl ClockSamples {
    pub const fn new(samples: i64) -> Self {
        Self(samples)
    }

    /// (whole seconds, samples *after* whole seconds)
    pub fn whole_seconds_and_fract(&self, sample_rate: u32) -> (i64, u32) {
        // Provide optimized implementations for common sample rates.
        let (whole_seconds, fract_samples) = match sample_rate {
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
                sample_rate - (fract_samples.abs() as u32),
            )
        } else {
            (whole_seconds, fract_samples as u32)
        }
    }

    pub fn fract_second_samples(&self, sample_rate: u32) -> u32 {
        match sample_rate {
            44100 => (self.0 % 44100) as u32,
            48000 => (self.0 % 48000) as u32,
            sample_rate => (self.0 % i64::from(sample_rate)) as u32,
        }
    }

    pub fn to_seconds(self, sample_rate: u32, sample_rate_recip: f64) -> ClockSeconds {
        // Provide optimized implementations for common sample rates.
        let (whole_seconds, fract_samples) = self.whole_seconds_and_fract(sample_rate);

        ClockSeconds(whole_seconds as f64 + (fract_samples as f64 * sample_rate_recip))
    }

    /// Convert to the corresponding musical time.
    pub fn to_musical(
        self,
        transport: &MusicalTransport,
        sample_rate: u32,
        sample_rate_recip: f64,
    ) -> MusicalTime {
        transport.sample_to_musical(self, sample_rate, sample_rate_recip)
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

/// Musical time in units of beats.
#[derive(Default, Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct MusicalTime(pub f64);

impl MusicalTime {
    pub const ZERO: Self = Self(0.0);

    pub const fn new(beats: f64) -> Self {
        Self(beats)
    }

    /// Convert to the corresponding time in samples.
    pub fn to_sample_time(&self, seconds_per_beat: f64, sample_rate: u32) -> ClockSamples {
        ClockSeconds(self.0 * seconds_per_beat).to_samples(sample_rate)
    }

    /// Convert to the corresponding time in seconds.
    pub fn to_seconds(&self, seconds_per_beat: f64) -> ClockSeconds {
        ClockSeconds(self.0 * seconds_per_beat)
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

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MusicalTransport {
    pub beats_per_minute: f64,
    // TODO: Automated tempo?
}

impl Default for MusicalTransport {
    fn default() -> Self {
        Self {
            beats_per_minute: 110.0,
        }
    }
}

impl MusicalTransport {
    pub fn new(beats_per_minute: f64) -> Self {
        Self { beats_per_minute }
    }

    pub fn beats_per_minute(&self) -> f64 {
        self.beats_per_minute
    }

    pub fn seconds_per_beat(&self) -> f64 {
        60.0 / self.beats_per_minute
    }

    pub fn beats_per_second(&self) -> f64 {
        self.beats_per_minute * (1.0 / 60.0)
    }

    /// Convert from musical time the corresponding time in seconds.
    pub fn musical_to_seconds(&self, musical: MusicalTime) -> ClockSeconds {
        ClockSeconds(musical.0 * self.seconds_per_beat())
    }

    /// Convert from musical time the corresponding time in samples.
    pub fn musical_to_sample(&self, musical: MusicalTime, sample_rate: u32) -> ClockSamples {
        self.musical_to_seconds(musical).to_samples(sample_rate)
    }

    /// Convert from the time in samples to the corresponding musical time.
    pub fn sample_to_musical(
        &self,
        sample_time: ClockSamples,
        sample_rate: u32,
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

// Information about the time of the internal audio clock.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AudioClock {
    /// The timestamp from the audio stream, equal to the number of frames
    /// (samples in a single channel of audio) of data that have been processed
    /// since the Firewheel context was first started.
    ///
    /// This value also automatically accounts for any output underflows that
    /// occur.
    ///
    /// Note, generally this value will always count up, but there may be a
    /// few edge cases that cause this value to be less than the previous call,
    /// such as when the sample rate of the stream has been changed.
    pub samples: ClockSamples,

    /// The timestamp from the audio stream, equal to the number of seconds of
    /// data that have been processed since the Firewheel context was first started.
    ///
    /// This value also automatically accounts for any output underflows that
    /// occur.
    pub seconds: ClockSeconds,

    /// The current time of the playhead of the musical transport.
    ///
    /// If no musical transport is present, then this will be `None`.
    pub musical: Option<MusicalTime>,

    /// This is `true` if a musical transport is present and it is not paused,
    /// `false` otherwise.
    pub transport_is_playing: bool,
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
    // TODO: Loop range
}

impl Default for TransportState {
    fn default() -> Self {
        Self {
            transport: None,
            playing: Notify::new(false),
            playhead: Notify::new(MusicalTime::ZERO),
        }
    }
}
