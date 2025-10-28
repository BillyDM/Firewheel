use core::cmp::Ordering;
use core::num::NonZeroU32;

use bevy_platform::prelude::Vec;

use crate::clock::{
    beats_per_second, seconds_per_beat, DurationMusical, DurationSeconds, InstantMusical,
    InstantSamples, InstantSeconds, ProcTransportInfo,
};

#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "bevy_reflect", derive(bevy_reflect::Reflect))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TransportKeyframe {
    /// The beats per minute of this keyframe.
    pub beats_per_minute: f64,
    /// The instant this keyframe starts.
    pub instant: InstantMusical,
}

#[derive(Debug, Clone)]
struct KeyframeCache {
    start_time_seconds: DurationSeconds,
}

/// A musical transport with multiple keyframes of tempo. The tempo
/// immediately jumps from one keyframe to another (the tempo is *NOT*
/// linearly interpolated between keyframes).
#[derive(Debug, Clone)]
pub struct DynamicTransport {
    keyframes: Vec<TransportKeyframe>,
    cache: Vec<KeyframeCache>,
}

impl DynamicTransport {
    /// Construct a new `DynamicTransport`.
    pub fn new(keyframes: Vec<TransportKeyframe>) -> Result<Self, DynamicTransportError> {
        if keyframes.len() == 0 {
            return Err(DynamicTransportError::NoKeyframes);
        }
        if keyframes[0].instant != InstantMusical::ZERO {
            return Err(DynamicTransportError::FirstKeyframeNotZero);
        }
        if keyframes[0].beats_per_minute <= 0.0 {
            return Err(DynamicTransportError::InvalidKeyframe);
        }

        let mut cache: Vec<KeyframeCache> = Vec::with_capacity(keyframes.len());

        let mut start_time_seconds = DurationSeconds::ZERO;
        let mut prev_instant = InstantMusical::ZERO;

        for i in 1..keyframes.len() {
            if !keyframes[i].instant.0.is_finite() {
                return Err(DynamicTransportError::InvalidKeyframe);
            }

            match keyframes[i].instant.partial_cmp(&prev_instant) {
                Some(Ordering::Greater) => {}
                Some(Ordering::Less) => return Err(DynamicTransportError::KeyframesNotSorted),
                Some(Ordering::Equal) => return Err(DynamicTransportError::DuplicateKeyframes),
                None => return Err(DynamicTransportError::InvalidKeyframe),
            }
            prev_instant = keyframes[i].instant;

            if keyframes[i].beats_per_minute <= 0.0 {
                return Err(DynamicTransportError::InvalidKeyframe);
            }

            cache.push(KeyframeCache { start_time_seconds });

            let duration = keyframes[i].instant - keyframes[i - 1].instant;
            start_time_seconds += DurationSeconds(
                duration.0 * seconds_per_beat(keyframes[i - 1].beats_per_minute, 1.0),
            );
        }

        cache.push(KeyframeCache { start_time_seconds });

        Ok(Self { keyframes, cache })
    }

    pub fn keyframes(&self) -> &[TransportKeyframe] {
        &self.keyframes
    }

    pub fn musical_to_seconds(
        &self,
        musical: InstantMusical,
        transport_start: InstantSeconds,
        speed_multiplier: f64,
    ) -> InstantSeconds {
        transport_start + self.musical_to_seconds_inner(musical, speed_multiplier)
    }

    pub fn musical_to_samples(
        &self,
        musical: InstantMusical,
        transport_start: InstantSamples,
        speed_multiplier: f64,
        sample_rate: NonZeroU32,
    ) -> InstantSamples {
        transport_start
            + self
                .musical_to_seconds_inner(musical, speed_multiplier)
                .to_samples(sample_rate)
    }

    pub fn seconds_to_musical(
        &self,
        seconds: InstantSeconds,
        transport_start: InstantSeconds,
        speed_multiplier: f64,
    ) -> InstantMusical {
        self.seconds_to_musical_inner(seconds - transport_start, speed_multiplier)
    }

    pub fn samples_to_musical(
        &self,
        sample_time: InstantSamples,
        transport_start: InstantSamples,
        speed_multiplier: f64,
        sample_rate: NonZeroU32,
        sample_rate_recip: f64,
    ) -> InstantMusical {
        self.seconds_to_musical_inner(
            (sample_time - transport_start).to_seconds(sample_rate, sample_rate_recip),
            speed_multiplier,
        )
    }

    pub fn delta_seconds_from(
        &self,
        from: InstantMusical,
        delta_seconds: DurationSeconds,
        speed_multiplier: f64,
    ) -> InstantMusical {
        self.seconds_to_musical_inner(
            self.musical_to_seconds_inner(from, speed_multiplier) + delta_seconds,
            speed_multiplier,
        )
    }

    pub fn transport_start(
        &self,
        now: InstantSamples,
        playhead: InstantMusical,
        speed_multiplier: f64,
        sample_rate: NonZeroU32,
    ) -> InstantSamples {
        now - self
            .musical_to_seconds_inner(playhead, speed_multiplier)
            .to_samples(sample_rate)
    }

    pub fn bpm_at_musical(&self, musical: InstantMusical, speed_multiplier: f64) -> f64 {
        let keyframe_i = binary_search_musical(&self.keyframes, musical);

        self.keyframes[keyframe_i].beats_per_minute * speed_multiplier
    }

    pub fn proc_transport_info(
        &self,
        mut frames: usize,
        playhead: InstantMusical,
        speed_multiplier: f64,
        sample_rate: NonZeroU32,
    ) -> ProcTransportInfo {
        let keyframe_i = binary_search_musical(&self.keyframes, playhead);

        if keyframe_i < self.keyframes.len() - 1 {
            let beats_left_in_keyframe = self.keyframes[keyframe_i + 1].instant - playhead;

            let frames_left_in_keyframe = DurationSeconds(
                beats_left_in_keyframe.0
                    * seconds_per_beat(
                        self.keyframes[keyframe_i].beats_per_minute,
                        speed_multiplier,
                    ),
            )
            .to_samples(sample_rate)
            .0 as usize;

            frames = frames.min(frames_left_in_keyframe);
        }

        ProcTransportInfo {
            frames,
            beats_per_minute: self.keyframes[keyframe_i].beats_per_minute * speed_multiplier,
        }
    }

    fn musical_to_seconds_inner(
        &self,
        musical: InstantMusical,
        speed_multiplier: f64,
    ) -> DurationSeconds {
        let keyframe_i = binary_search_musical(&self.keyframes, musical);
        let keyframe = &self.keyframes[keyframe_i];
        let cache = &self.cache[keyframe_i];

        DurationSeconds(
            cache.start_time_seconds.0
                + ((musical - keyframe.instant).0
                    * seconds_per_beat(keyframe.beats_per_minute, 1.0)),
        ) / speed_multiplier
    }

    fn seconds_to_musical_inner(
        &self,
        seconds: DurationSeconds,
        speed_multiplier: f64,
    ) -> InstantMusical {
        let seconds = seconds * speed_multiplier;

        let keyframe_i = binary_search_seconds(&self.cache, seconds);
        let keyframe = &self.keyframes[keyframe_i];
        let cache = &self.cache[keyframe_i];

        keyframe.instant
            + DurationMusical(
                (seconds.0 - cache.start_time_seconds.0)
                    * beats_per_second(keyframe.beats_per_minute, 1.0),
            )
    }
}

impl PartialEq for DynamicTransport {
    fn eq(&self, other: &Self) -> bool {
        self.keyframes.eq(&other.keyframes)
    }
}

/// An error while constructing a [`DynamicTransport`].
#[derive(Debug, Clone, Copy, thiserror::Error)]
pub enum DynamicTransportError {
    /// The Vec of keyframes was empty.
    #[error("The Vec of keyframes was empty")]
    NoKeyframes,
    /// The first keyframe does not occur at `MusicalTime::Zero`.
    #[error("The first keyframe does not occur at `MusicalTime::Zero`")]
    FirstKeyframeNotZero,
    /// One or more keyframes occur on the same instant.
    #[error("One or more keyframes occur on the same instant")]
    DuplicateKeyframes,
    /// The keyframes are not sorted by instant.
    #[error("The keyframes are not sorted by instant")]
    KeyframesNotSorted,
    /// A keyframe contained an invalid `beats_per_minute` or `instant` value.
    #[error("A keyframe contained an invalid `beats_per_minute` or `instant` value")]
    InvalidKeyframe,
}

fn binary_search_musical(keyframes: &[TransportKeyframe], musical: InstantMusical) -> usize {
    // We have checked that all values are finite in the constructor, so the
    // `unwrap_or(Ordering::Equal)` case will never happen.
    match keyframes.binary_search_by(|k| k.instant.partial_cmp(&musical).unwrap_or(Ordering::Equal))
    {
        Ok(i) => i,
        Err(i) => i,
    }
}

fn binary_search_seconds(cache: &[KeyframeCache], seconds: DurationSeconds) -> usize {
    // We have checked that all values are finite in the constructor, so the
    // `unwrap_or(Ordering::Equal)` case will never happen.
    match cache.binary_search_by(|k| {
        k.start_time_seconds
            .partial_cmp(&seconds)
            .unwrap_or(Ordering::Equal)
    }) {
        Ok(i) => i,
        Err(i) => i,
    }
}
