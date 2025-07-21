pub const DEFAULT_AMP_EPSILON: f32 = 0.00001;
pub const DEFAULT_DB_EPSILON: f32 = -100.0;

/// A value representing a volume (gain) applied to an audio signal.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Volume {
    /// Volume in a linear scale, where `0.0` is silence and `1.0` is unity gain.
    Linear(f32),
    /// Volume in decibels, where `0.0` is unity gain and `f32::NEG_INFINITY` is silence.
    Decibels(f32),
}

impl Volume {
    /// Unity gain (the resulting volume is the same as the source signal)
    pub const UNITY_GAIN: Self = Self::Linear(1.0);
    /// Silence
    pub const SILENT: Self = Self::Linear(0.0);

    /// Get the volume in raw amplitude for use in DSP.
    pub fn amp(&self) -> f32 {
        match *self {
            Self::Linear(volume) => linear_volume_to_amp_clamped(volume, 0.0),
            Self::Decibels(db) => db_to_amp(db),
        }
    }

    /// Get the volume in raw amplitude for use in DSP.
    ///
    /// If the resulting amplitude is `<= amp_epsilon`, then `0.0` (silence) will be returned.
    pub fn amp_clamped(&self, amp_epsilon: f32) -> f32 {
        match *self {
            Self::Linear(volume) => linear_volume_to_amp_clamped(volume, amp_epsilon),
            Self::Decibels(db) => {
                if db == f32::NEG_INFINITY {
                    0.0
                } else {
                    let amp = db_to_amp(db);
                    if amp <= amp_epsilon {
                        0.0
                    } else {
                        amp
                    }
                }
            }
        }
    }

    /// Get the volume in decibles.
    pub fn decibels(&self) -> f32 {
        match *self {
            Self::Linear(volume) => {
                if volume == 0.0 {
                    f32::NEG_INFINITY
                } else {
                    let amp = linear_volume_to_amp_clamped(volume, 0.0);
                    amp_to_db(amp)
                }
            }
            Self::Decibels(db) => db,
        }
    }

    /// Get the volume in decibles.
    ///
    /// If the resulting decibel value is `<= db_epsilon`, then `f32::NEG_INFINITY` (silence)
    /// will be returned.
    pub fn decibels_clamped(&self, db_epsilon: f32) -> f32 {
        match *self {
            Self::Linear(volume) => {
                if volume == 0.0 {
                    f32::NEG_INFINITY
                } else {
                    let amp = linear_volume_to_amp_clamped(volume, 0.0);
                    let db = amp_to_db(amp);
                    if db <= db_epsilon {
                        f32::NEG_INFINITY
                    } else {
                        db
                    }
                }
            }
            Self::Decibels(db) => {
                if db <= db_epsilon {
                    f32::NEG_INFINITY
                } else {
                    db
                }
            }
        }
    }

    /// Get the volume in linear units, where `0.0` is silence and `1.0` is unity gain.
    pub fn linear(&self) -> f32 {
        match *self {
            Self::Linear(volume) => volume,
            Self::Decibels(db) => amp_to_linear_volume_clamped(db_to_amp(db), 0.0),
        }
    }

    /// Get the value as a [`Volume::Linear`] value.
    pub fn as_linear_variant(&self) -> Self {
        Self::Linear(self.linear())
    }

    /// Get the value as a [`Volume::Decibels`] value.
    pub fn as_decibel_variant(&self) -> Self {
        Self::Decibels(self.decibels())
    }
}

impl Default for Volume {
    fn default() -> Self {
        Self::UNITY_GAIN
    }
}

impl core::ops::Add<Self> for Volume {
    type Output = Self;

    fn add(self, rhs: Self) -> Self {
        use Volume::{Decibels, Linear};

        match (self, rhs) {
            (Linear(a), Linear(b)) => Linear(a + b),
            (Decibels(a), Decibels(b)) => Decibels(amp_to_db(db_to_amp(a) + db_to_amp(b))),
            // {Linear, Decibels} favors the left hand side of the operation by
            // first converting the right hand side to the same type as the left
            // hand side and then performing the operation.
            (Linear(..), Decibels(..)) => self + rhs.as_linear_variant(),
            (Decibels(..), Linear(..)) => self + rhs.as_decibel_variant(),
        }
    }
}

impl core::ops::Sub<Self> for Volume {
    type Output = Self;

    fn sub(self, rhs: Self) -> Self {
        use Volume::{Decibels, Linear};

        match (self, rhs) {
            (Linear(a), Linear(b)) => Linear(a - b),
            (Decibels(a), Decibels(b)) => Decibels(amp_to_db(db_to_amp(a) - db_to_amp(b))),
            // {Linear, Decibels} favors the left hand side of the operation by
            // first converting the right hand side to the same type as the left
            // hand side and then performing the operation.
            (Linear(..), Decibels(..)) => self - rhs.as_linear_variant(),
            (Decibels(..), Linear(..)) => self - rhs.as_decibel_variant(),
        }
    }
}

impl core::ops::Mul<Self> for Volume {
    type Output = Self;

    fn mul(self, rhs: Self) -> Self {
        use Volume::{Decibels, Linear};

        match (self, rhs) {
            (Linear(a), Linear(b)) => Linear(a * b),
            (Decibels(a), Decibels(b)) => Decibels(amp_to_db(db_to_amp(a) * db_to_amp(b))),
            // {Linear, Decibels} favors the left hand side of the operation by
            // first converting the right hand side to the same type as the left
            // hand side and then performing the operation.
            (Linear(..), Decibels(..)) => self * rhs.as_linear_variant(),
            (Decibels(..), Linear(..)) => self * rhs.as_decibel_variant(),
        }
    }
}

impl core::ops::Div<Self> for Volume {
    type Output = Self;

    fn div(self, rhs: Self) -> Self {
        use Volume::{Decibels, Linear};

        match (self, rhs) {
            (Linear(a), Linear(b)) => Linear(a / b),
            (Decibels(a), Decibels(b)) => Decibels(amp_to_db(db_to_amp(a) / db_to_amp(b))),
            // {Linear, Decibels} favors the left hand side of the operation by
            // first converting the right hand side to the same type as the left
            // hand side and then performing the operation.
            (Linear(..), Decibels(..)) => self / rhs.as_linear_variant(),
            (Decibels(..), Linear(..)) => self / rhs.as_decibel_variant(),
        }
    }
}

impl core::ops::AddAssign<Self> for Volume {
    fn add_assign(&mut self, rhs: Self) {
        *self = *self + rhs;
    }
}

impl core::ops::SubAssign<Self> for Volume {
    fn sub_assign(&mut self, rhs: Self) {
        *self = *self - rhs;
    }
}

impl core::ops::MulAssign<Self> for Volume {
    fn mul_assign(&mut self, rhs: Self) {
        *self = *self * rhs;
    }
}

impl core::ops::DivAssign<Self> for Volume {
    fn div_assign(&mut self, rhs: Self) {
        *self = *self / rhs;
    }
}

/// Returns the raw amplitude from the given decibel value.
#[inline]
pub fn db_to_amp(db: f32) -> f32 {
    if db == f32::NEG_INFINITY {
        0.0
    } else {
        10.0f32.powf(0.05 * db)
    }
}

/// Returns the decibel value from the given raw amplitude.
#[inline]
pub fn amp_to_db(amp: f32) -> f32 {
    if amp == 0.0 {
        f32::NEG_INFINITY
    } else {
        20.0 * amp.log10()
    }
}

/// Returns the raw amplitude from the given decibel value.
///
/// If `db == f32::NEG_INFINITY || db <= db_epsilon`, then `0.0` (silence) will be
/// returned.
#[inline]
pub fn db_to_amp_clamped(db: f32, db_epsilon: f32) -> f32 {
    if db == f32::NEG_INFINITY || db <= db_epsilon {
        0.0
    } else {
        db_to_amp(db)
    }
}

/// Returns the decibel value from the given raw amplitude.
///
/// If `amp <= amp_epsilon`, then `f32::NEG_INFINITY` (silence) will be returned.
#[inline]
pub fn amp_to_db_clamped(amp: f32, amp_epsilon: f32) -> f32 {
    if amp <= amp_epsilon {
        f32::NEG_INFINITY
    } else {
        amp_to_db(amp)
    }
}

/// Map the linear volume (where `0.0` means mute and `1.0` means unity
/// gain) to the corresponding raw amplitude value (not decibels) for use in
/// DSP. Values above `1.0` are allowed.
///
/// If the resulting amplitude is `<= amp_epsilon`, then `0.0` (silence) will be
/// returned.
#[inline]
pub fn linear_volume_to_amp_clamped(linear_volume: f32, amp_epsilon: f32) -> f32 {
    let v = linear_volume * linear_volume;
    if v <= amp_epsilon {
        0.0
    } else {
        v
    }
}

/// Map the raw amplitude (where `0.0` means mute and `1.0` means unity
/// gain) to the corresponding linear volume.
///
/// If the amplitude is `<= amp_epsilon`, then `0.0` (silence) will be
/// returned.
#[inline]
pub fn amp_to_linear_volume_clamped(amp: f32, amp_epsilon: f32) -> f32 {
    if amp <= amp_epsilon {
        0.0
    } else {
        amp.sqrt()
    }
}

/// A struct that converts a value in decibels to a normalized range used in
/// meters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DbMeterNormalizer {
    min_db: f32,
    range_recip: f32,
    factor: f32,
}

impl DbMeterNormalizer {
    /// * `min_db` - The minimum decibel value shown in the meter.
    /// * `max_db` - The maximum decibel value shown in the meter.
    /// * `center_db` - The decibel value that will appear halfway (0.5) in the
    /// normalized range. For example, if you had `min_db` as `-100.0` and
    /// `max_db` as `0.0`, then a good `center_db` value would be `-22`.
    pub fn new(min_db: f32, max_db: f32, center_db: f32) -> Self {
        assert!(max_db > min_db);
        assert!(center_db > min_db && center_db < max_db);

        let range_recip = (max_db - min_db).recip();
        let center_normalized = ((center_db - min_db) * range_recip).clamp(0.0, 1.0);

        Self {
            min_db,
            range_recip,
            factor: 0.5_f32.log(center_normalized),
        }
    }

    #[inline]
    pub fn normalize(&self, db: f32) -> f32 {
        ((db - self.min_db) * self.range_recip)
            .clamp(0.0, 1.0)
            .powf(self.factor)
    }
}

impl Default for DbMeterNormalizer {
    fn default() -> Self {
        Self::new(-100.0, 0.0, -22.0)
    }
}
