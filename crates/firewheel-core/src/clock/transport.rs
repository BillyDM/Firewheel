mod dynamic_transport;
mod static_transport;

use bevy_platform::prelude::Vec;
use bevy_platform::sync::Arc;

use core::{fmt::Debug, num::NonZeroU32, ops::Range};

pub use dynamic_transport::{DynamicTransport, TransportKeyframe};
pub use static_transport::StaticTransport;

use crate::{
    clock::{DurationSeconds, EventInstant, InstantMusical, InstantSamples, InstantSeconds},
    diff::Notify,
};

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "bevy", derive(bevy_ecs::prelude::Component))]
pub enum MusicalTransport {
    /// A musical transport with a single static tempo in beats per minute.
    Static(StaticTransport),
    /// A musical transport with multiple keyframes of tempo. The tempo
    /// immediately jumps from one keyframe to another (the tempo is *NOT*
    /// linearly interpolated between keyframes).
    Dynamic(Arc<DynamicTransport>),
}

impl MusicalTransport {
    /// Returns the beats per minute if this is of type [`MusicalTransport::Static`],
    /// `None` otherwise.
    pub fn beats_per_minute(&self) -> Option<f64> {
        if let MusicalTransport::Static(s) = self {
            Some(s.beats_per_minute)
        } else {
            None
        }
    }

    /// Convert the time in musical beats to the corresponding time in seconds.
    ///
    /// * `musical` - The time in musical beats to convert.
    /// * `transport_start` - The instant of the start of the transport (musical
    /// time of `0`).
    /// * `speed_multiplier` - A multiplier for the playback speed. A value of
    /// `1.0` means no change in speed, a value less than `1.0` means a decrease
    /// in speed, and a value greater than `1.0` means an increase in speed.
    pub fn musical_to_seconds(
        &self,
        musical: InstantMusical,
        transport_start: InstantSeconds,
        speed_multiplier: f64,
    ) -> InstantSeconds {
        match self {
            MusicalTransport::Static(t) => {
                t.musical_to_seconds(musical, transport_start, speed_multiplier)
            }
            MusicalTransport::Dynamic(t) => {
                t.musical_to_seconds(musical, transport_start, speed_multiplier)
            }
        }
    }

    /// Convert the time in musical beats to the corresponding time in samples.
    ///
    /// * `musical` - The time in musical beats to convert.
    /// * `transport_start` - The instant of the start of the transport (musical
    /// time of `0`).
    /// * `speed_multiplier` - A multiplier for the playback speed. A value of
    /// `1.0` means no change in speed, a value less than `1.0` means a decrease
    /// in speed, and a value greater than `1.0` means an increase in speed.
    /// * `sample_rate` - The sample rate of the stream.
    pub fn musical_to_samples(
        &self,
        musical: InstantMusical,
        transport_start: InstantSamples,
        speed_multiplier: f64,
        sample_rate: NonZeroU32,
    ) -> InstantSamples {
        match self {
            MusicalTransport::Static(t) => {
                t.musical_to_samples(musical, transport_start, speed_multiplier, sample_rate)
            }
            MusicalTransport::Dynamic(t) => {
                t.musical_to_samples(musical, transport_start, speed_multiplier, sample_rate)
            }
        }
    }

    /// Convert the time in seconds to the corresponding time in musical beats.
    ///
    /// * `seconds` - The time in seconds to convert.
    /// * `transport_start` - The instant of the start of the transport (musical
    /// time of `0`).
    /// * `speed_multiplier` - A multiplier for the playback speed. A value of
    /// `1.0` means no change in speed, a value less than `1.0` means a decrease
    /// in speed, and a value greater than `1.0` means an increase in speed.
    /// * `sample_rate` - The sample rate of the stream.
    pub fn seconds_to_musical(
        &self,
        seconds: InstantSeconds,
        transport_start: InstantSeconds,
        speed_multiplier: f64,
    ) -> InstantMusical {
        match self {
            MusicalTransport::Static(t) => {
                t.seconds_to_musical(seconds, transport_start, speed_multiplier)
            }
            MusicalTransport::Dynamic(t) => {
                t.seconds_to_musical(seconds, transport_start, speed_multiplier)
            }
        }
    }

    /// Convert the time in samples to the corresponding time in musical beats.
    ///
    /// * `sample_time` - The time in samples to convert.
    /// * `transport_start` - The instant of the start of the transport (musical
    /// time of `0`).
    /// * `speed_multiplier` - A multiplier for the playback speed. A value of
    /// `1.0` means no change in speed, a value less than `1.0` means a decrease
    /// in speed, and a value greater than `1.0` means an increase in speed.
    /// * `sample_rate` - The sample rate of the stream.
    /// * `sample_rate` - The reciprocal of the sample rate.
    pub fn samples_to_musical(
        &self,
        sample_time: InstantSamples,
        transport_start: InstantSamples,
        speed_multiplier: f64,
        sample_rate: NonZeroU32,
        sample_rate_recip: f64,
    ) -> InstantMusical {
        match self {
            MusicalTransport::Static(t) => t.samples_to_musical(
                sample_time,
                transport_start,
                speed_multiplier,
                sample_rate,
                sample_rate_recip,
            ),
            MusicalTransport::Dynamic(t) => t.samples_to_musical(
                sample_time,
                transport_start,
                speed_multiplier,
                sample_rate,
                sample_rate_recip,
            ),
        }
    }

    /// Return the musical time that occurs `delta_seconds` seconds after the
    /// given `from` timestamp.
    ///
    /// * `speed_multiplier` - A multiplier for the playback speed. A value of
    /// `1.0` means no change in speed, a value less than `1.0` means a decrease
    /// in speed, and a value greater than `1.0` means an increase in speed.
    pub fn delta_seconds_from(
        &self,
        from: InstantMusical,
        delta_seconds: DurationSeconds,
        speed_multiplier: f64,
    ) -> InstantMusical {
        match self {
            MusicalTransport::Static(t) => {
                t.delta_seconds_from(from, delta_seconds, speed_multiplier)
            }
            MusicalTransport::Dynamic(t) => {
                t.delta_seconds_from(from, delta_seconds, speed_multiplier)
            }
        }
    }

    /// Return the tempo in beats per minute at the given musical time.
    ///
    /// * `speed_multiplier` - A multiplier for the playback speed. A value of
    /// `1.0` means no change in speed, a value less than `1.0` means a decrease
    /// in speed, and a value greater than `1.0` means an increase in speed.
    pub fn bpm_at_musical(&self, musical: InstantMusical, speed_multiplier: f64) -> f64 {
        match self {
            MusicalTransport::Static(t) => t.bpm_at_musical(musical, speed_multiplier),
            MusicalTransport::Dynamic(t) => t.bpm_at_musical(musical, speed_multiplier),
        }
    }

    /// Return information about this transport for this processing block.
    ///
    /// * `frames` - The number of frames in this processing block.
    /// * `playhead` - The current playhead of the transport at frame `0` in this
    /// processing block.
    /// * `speed_multiplier` - A multiplier for the playback speed. A value of
    /// `1.0` means no change in speed, a value less than `1.0` means a decrease
    /// in speed, and a value greater than `1.0` means an increase in speed.
    /// * `sample_rate` - The sample rate of the stream.
    pub fn proc_transport_info(
        &self,
        frames: usize,
        playhead: InstantMusical,
        speed_multiplier: f64,
        sample_rate: NonZeroU32,
    ) -> ProcTransportInfo {
        match self {
            MusicalTransport::Static(t) => t.proc_transport_info(frames, speed_multiplier),
            MusicalTransport::Dynamic(t) => {
                t.proc_transport_info(frames, playhead, speed_multiplier, sample_rate)
            }
        }
    }

    /// Return the instant the beginning of this transport (musical time of `0`)
    /// occurs on.
    ///
    /// * `now` - The current time in samples.
    /// * `playhead` - The current playhead of the transport.
    /// * `speed_multiplier` - A multiplier for the playback speed. A value of
    /// `1.0` means no change in speed, a value less than `1.0` means a decrease
    /// in speed, and a value greater than `1.0` means an increase in speed.
    /// * `sample_rate` - The sample rate of the stream.
    pub fn transport_start(
        &self,
        now: InstantSamples,
        playhead: InstantMusical,
        speed_multiplier: f64,
        sample_rate: NonZeroU32,
    ) -> InstantSamples {
        match self {
            MusicalTransport::Static(t) => {
                t.transport_start(now, playhead, speed_multiplier, sample_rate)
            }
            MusicalTransport::Dynamic(t) => {
                t.transport_start(now, playhead, speed_multiplier, sample_rate)
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProcTransportInfo {
    /// The number of frames in this processing block that this information
    /// lasts for before either the information changes, or the end of the
    /// processing block is reached (whichever comes first).
    pub frames: usize,

    /// The beats per minute at the first frame of this process block.
    pub beats_per_minute: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpeedMultiplierKeyframe {
    /// The multiplier for the playback speed. A value of `1.0` means no change
    /// in speed, a value less than `1.0` means a decrease in speed, and a value
    /// greater than `1.0` means an increase in speed.
    ///
    /// This can cause a panic if `multiplier <= 0.0`.
    pub multiplier: f64,

    /// The instant that this keyframe happens.
    pub instant: EventInstant,
}

/// A multiplier for the speed of the transport.
///
/// A value of `1.0` means no change in speed, a value less than `1.0` means
/// a decrease in speed, and a value greater than `1.0` means an increase in
/// speed.
#[derive(Debug, Clone, PartialEq)]
pub enum TransportSpeed {
    /// Set the mulitplier to a single static value.
    Static {
        /// The speed multiplier.
        ///
        /// This can cause a panic if `multiplier <= 0.0`.
        multiplier: f64,
        /// If this is `Some`, then the change will happen when the transport
        /// reaches the given playhead.
        ///
        /// If this is `None`, then the change will happen as soon as the
        /// processor receives the event.
        start_at: Option<InstantMusical>,
    },
    /// Automate the speed multiplier values.
    Automate {
        /// The keyframes of animation.
        ///
        /// Note, the keyframes must be sorted by the event instant or else it
        /// will not work correctly.
        keyframes: Arc<Vec<SpeedMultiplierKeyframe>>,
        /// If this is `Some`, then the change will happen when the transport
        /// reaches the given playhead.
        ///
        /// If this is `None`, then the change will happen as soon as the
        /// processor receives the event.
        start_at: Option<InstantMusical>,
    },
}

impl TransportSpeed {
    /// Create a [`TransportSpeed`] with a single static value.
    ///
    /// * `speed_multiplier` - A multiplier for the playback speed. A value of
    /// `1.0` means no change in speed, a value less than `1.0` means a decrease
    /// in speed, and a value greater than `1.0` means an increase in speed.
    /// * `change_at`: If this is `Some`, then the change will happen when the transport
    /// reaches the given playhead. If this is `None`, then the change will happen as soon
    /// as the processor receives the event.
    pub const fn static_multiplier(multiplier: f64, change_at: Option<InstantMusical>) -> Self {
        Self::Static {
            multiplier,
            start_at: change_at,
        }
    }

    pub fn start_at(&self) -> Option<InstantMusical> {
        match self {
            Self::Static { start_at, .. } => *start_at,
            Self::Automate { start_at, .. } => *start_at,
        }
    }
}

impl Default for TransportSpeed {
    fn default() -> Self {
        Self::Static {
            multiplier: 1.0,
            start_at: None,
        }
    }
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

    /// A multiplier for the speed of the transport.
    ///
    /// A value of `1.0` means no change in speed, a value less than `1.0` means
    /// a decrease in speed, and a value greater than `1.0` means an increase in
    /// speed.
    pub speed: TransportSpeed,

    /// If this is `Some`, then the transport will automatically stop when the playhead
    /// reaches the given musical time.
    ///
    /// This has no effect if [`TransportState::loop_range`] is `Some`.
    pub stop_at: Option<InstantMusical>,

    /// If this is `Some`, then the transport will continously loop the given region.
    pub loop_range: Option<Range<InstantMusical>>,
}

impl TransportState {
    /// Set the transport to a single static tempo ([`StaticTransport`]).
    ///
    /// If `beats_per_minute` is `None`, then this will set the transport to `None`.
    pub fn set_static_transport(&mut self, beats_per_minute: Option<f64>) {
        self.transport =
            beats_per_minute.map(|bpm| MusicalTransport::Static(StaticTransport::new(bpm)));
    }

    /// Get the beats per minute of the current static transport.
    ///
    /// Returns `None` if `transport` is `None` or if `transport` is not
    /// [`MusicalTransport::Static`].
    pub fn beats_per_minute(&self) -> Option<f64> {
        self.transport.as_ref().and_then(|t| t.beats_per_minute())
    }

    /// Set a multiplier for the speed of the transport to a single static value.
    ///
    /// * `speed_multiplier` - A multiplier for the playback speed. A value of
    /// `1.0` means no change in speed, a value less than `1.0` means a decrease
    /// in speed, and a value greater than `1.0` means an increase in speed.
    /// * `change_at`: If this is `Some`, then the change will happen when the transport
    /// reaches the given playhead. If this is `None`, then the change will happen as soon
    /// as the processor receives the event.
    pub fn set_speed_multiplier(
        &mut self,
        speed_multiplier: f64,
        change_at: Option<InstantMusical>,
    ) {
        self.speed = TransportSpeed::static_multiplier(speed_multiplier, change_at);
    }
}

impl Default for TransportState {
    fn default() -> Self {
        Self {
            transport: None,
            playing: Notify::new(false),
            playhead: Notify::new(InstantMusical::ZERO),
            speed: TransportSpeed::default(),
            stop_at: None,
            loop_range: None,
        }
    }
}

#[inline]
pub fn seconds_per_beat(beats_per_minute: f64, speed_multiplier: f64) -> f64 {
    60.0 / (beats_per_minute * speed_multiplier)
}

#[inline]
pub fn beats_per_second(beats_per_minute: f64, speed_multiplier: f64) -> f64 {
    beats_per_minute * speed_multiplier * (1.0 / 60.0)
}
