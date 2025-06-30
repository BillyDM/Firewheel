use core::{error::Error, fmt, num::NonZeroU32};

/// A supported number of channels on an audio node.
///
/// This number cannot be greater than `64`.
#[repr(transparent)]
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ChannelCount(u32);

impl ChannelCount {
    pub const ZERO: Self = Self(0);
    pub const MONO: Self = Self(1);
    pub const STEREO: Self = Self(2);
    pub const MAX: Self = Self(64);

    /// Create a new [`ChannelCount`].
    ///
    /// Returns `None` if `count` is greater than `64`.
    #[inline]
    pub const fn new(count: u32) -> Option<Self> {
        if count <= 64 {
            Some(Self(count))
        } else {
            None
        }
    }

    #[inline]
    pub const fn get(&self) -> u32 {
        if self.0 <= 64 {
            self.0
        } else {
            // SAFETY:
            // The constructor ensures that the value is less than or
            // equal to `64`.
            unsafe { core::hint::unreachable_unchecked() }
        }
    }
}

impl From<usize> for ChannelCount {
    fn from(value: usize) -> Self {
        Self::new(value as u32).unwrap()
    }
}

impl Into<u32> for ChannelCount {
    #[inline]
    fn into(self) -> u32 {
        self.get()
    }
}

impl Into<usize> for ChannelCount {
    #[inline]
    fn into(self) -> usize {
        self.get() as usize
    }
}

/// A supported number of channels on an audio node.
///
/// This number cannot be `0` or greater than `64`.
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NonZeroChannelCount(NonZeroU32);

impl NonZeroChannelCount {
    pub const MONO: Self = Self(NonZeroU32::new(1).unwrap());
    pub const STEREO: Self = Self(NonZeroU32::new(2).unwrap());
    pub const MAX: Self = Self(NonZeroU32::new(64).unwrap());

    /// Create a new [`NonZeroChannelCount`].
    ///
    /// Returns `None` if `count` is greater than `64`.
    #[inline]
    pub const fn new(count: u32) -> Option<Self> {
        if count > 0 && count <= 64 {
            Some(Self(NonZeroU32::new(count).unwrap()))
        } else {
            None
        }
    }

    #[inline]
    pub const fn get(&self) -> ChannelCount {
        if self.0.get() <= 64 {
            ChannelCount(self.0.get())
        } else {
            // SAFETY:
            // The constructor ensures that the value is less than or
            // equal to `64`.
            unsafe { core::hint::unreachable_unchecked() }
        }
    }
}

impl Default for NonZeroChannelCount {
    fn default() -> Self {
        Self::STEREO
    }
}

impl From<usize> for NonZeroChannelCount {
    fn from(value: usize) -> Self {
        Self::new(value as u32).unwrap()
    }
}

impl Into<NonZeroU32> for NonZeroChannelCount {
    #[inline]
    fn into(self) -> NonZeroU32 {
        NonZeroU32::new(self.get().get()).unwrap()
    }
}

impl Into<usize> for NonZeroChannelCount {
    #[inline]
    fn into(self) -> usize {
        self.get().get() as usize
    }
}

/// A supported number of channels on an audio node.
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "bevy", derive(bevy_ecs::prelude::Component))]
pub struct ChannelConfig {
    pub num_inputs: ChannelCount,
    pub num_outputs: ChannelCount,
}

impl ChannelConfig {
    pub fn new(num_inputs: impl Into<ChannelCount>, num_outputs: impl Into<ChannelCount>) -> Self {
        Self {
            num_inputs: num_inputs.into(),
            num_outputs: num_outputs.into(),
        }
    }

    pub fn verify(
        &self,
        min_num_inputs: ChannelCount,
        max_num_inputs: ChannelCount,
        min_num_outputs: ChannelCount,
        max_num_outputs: ChannelCount,
        equal_num_ins_outs: bool,
    ) -> Result<(), ChannelConfigError> {
        if self.num_inputs.get() < min_num_inputs.get()
            || self.num_inputs.get() > max_num_inputs.get()
        {
            Err(ChannelConfigError::InvalidNumInputs {
                min: min_num_inputs,
                max: max_num_inputs,
                got: self.num_inputs,
            })
        } else if self.num_outputs.get() < min_num_outputs.get()
            || self.num_outputs.get() > max_num_outputs.get()
        {
            Err(ChannelConfigError::InvalidNumOutputs {
                min: min_num_outputs,
                max: max_num_outputs,
                got: self.num_outputs,
            })
        } else if equal_num_ins_outs && self.num_inputs.get() != self.num_outputs.get() {
            Err(ChannelConfigError::NumInOutNotEqual {
                got_in: self.num_inputs,
                got_out: self.num_outputs,
            })
        } else {
            Ok(())
        }
    }
}

impl From<(usize, usize)> for ChannelConfig {
    fn from(value: (usize, usize)) -> Self {
        Self::new(value.0, value.1)
    }
}

/// An invalid channel configuration
#[derive(Debug)]
pub enum ChannelConfigError {
    InvalidNumInputs {
        min: ChannelCount,
        max: ChannelCount,
        got: ChannelCount,
    },
    InvalidNumOutputs {
        min: ChannelCount,
        max: ChannelCount,
        got: ChannelCount,
    },
    NumInOutNotEqual {
        got_in: ChannelCount,
        got_out: ChannelCount,
    },
}

impl Error for ChannelConfigError {}

impl fmt::Display for ChannelConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidNumInputs { min, max, got } => {
                write!(
                    f,
                    "Invalid number of input channels on audio node | got: {}, min: {}, max: {}",
                    got.get(),
                    min.get(),
                    max.get()
                )
            }
            Self::InvalidNumOutputs { min, max, got } => {
                write!(
                    f,
                    "Invalid number of output channels on audio node | got: {}, min: {}, max: {}",
                    got.get(),
                    min.get(),
                    max.get()
                )
            }
            Self::NumInOutNotEqual { got_in, got_out } => {
                write!(f, "Number of input channels does not equal number of output channels | in: {}, out: {}", got_in.get(), got_out.get())
            }
        }
    }
}
