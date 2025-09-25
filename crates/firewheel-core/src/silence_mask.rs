use core::{ops::Range, u64};

/// An optional optimization hint on which channels contain all
/// zeros (silence). The first bit (`0x1`) is the first channel,
/// the second bit is the second channel, and so on.
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SilenceMask(pub u64);

impl SilenceMask {
    /// A mask with no channels marked as silent
    pub const NONE_SILENT: Self = Self(0);

    /// A mask with only the first channel marked as silent
    pub const MONO_SILENT: Self = Self(0b1);

    /// A mask with only the first two channels marked as silent
    pub const STEREO_SILENT: Self = Self(0b11);

    /// Construct a new [`SilenceMask`] with all channels marked as
    /// silent.
    ///
    /// `num_channels` must be less than or equal to `64`.
    pub const fn new_all_silent(num_channels: usize) -> Self {
        if num_channels >= 64 {
            Self(u64::MAX)
        } else {
            Self((0b1 << num_channels) - 1)
        }
    }

    /// Returns `true` if the channel is marked as silent, `false`
    /// otherwise.
    ///
    /// `i` must be less than `64`.
    pub const fn is_channel_silent(&self, i: usize) -> bool {
        self.0 & (0b1 << i) != 0
    }

    /// Returns `true` if any channel is marked as silent, `false`
    /// otherwise.
    ///
    /// `num_channels` must be less than or equal to `64`.
    pub const fn any_channel_silent(&self, num_channels: usize) -> bool {
        if num_channels >= 64 {
            self.0 != 0
        } else {
            self.0 & ((0b1 << num_channels) - 1) != 0
        }
    }

    /// Returns `true` if all channels are marked as silent, `false`
    /// otherwise.
    ///
    /// `num_channels` must be less than or equal to `64`.
    pub const fn all_channels_silent(&self, num_channels: usize) -> bool {
        if num_channels >= 64 {
            self.0 == u64::MAX
        } else {
            let mask = (0b1 << num_channels) - 1;
            self.0 & mask == mask
        }
    }

    /// Returns `true` if all channels in the given range are marked
    /// as silent, `false` otherwise.
    ///
    /// This range must be in the range `[0, 64]`
    pub const fn range_silent(&self, range: Range<usize>) -> bool {
        if range.start >= 64 {
            false
        } else if range.end >= 64 {
            let mask = u64::MAX & !((0b1 << range.start) - 1);
            self.0 & mask == mask
        } else {
            let mask = ((0b1 << range.end) - 1) & !((0b1 << range.start) - 1);
            self.0 & mask == mask
        }
    }

    /// Mark/un-mark the given channel as silent.
    ///
    /// `i` must be less than `64`.
    pub fn set_channel(&mut self, i: usize, silent: bool) {
        if silent {
            self.0 |= 0b1 << i;
        } else {
            self.0 &= !(0b1 << i);
        }
    }

    pub const fn union(self, other: Self) -> Self {
        SilenceMask(self.0 & other.0)
    }

    pub fn union_with(&mut self, other: Self) {
        self.0 &= other.0;
    }
}
