use core::ops::Range;

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
            let mask = !((0b1 << range.start) - 1);
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

    pub fn to_constant_mask(self) -> ConstantMask {
        ConstantMask(self.0)
    }
}

/// An optional optimization hint on which channels have all samples
/// set to the same value. The first bit (`0x1`) is the first
/// channel, the second bit is the second channel, and so on.
///
/// This can be useful for nodes that use audio buffers as CV
/// (control voltage) ports.
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ConstantMask(pub u64);

impl ConstantMask {
    /// A mask with no channels marked as constant
    pub const NONE_CONSTANT: Self = Self(0);

    /// A mask with only the first channel marked as constant
    pub const MONO_CONSTANT: Self = Self(0b1);

    /// A mask with only the first two channels marked as constant
    pub const STEREO_CONSTANT: Self = Self(0b11);

    /// Construct a new [`ConstantMask`] with all channels marked as
    /// constant.
    ///
    /// `num_channels` must be less than or equal to `64`.
    pub const fn new_all_constant(num_channels: usize) -> Self {
        if num_channels >= 64 {
            Self(u64::MAX)
        } else {
            Self((0b1 << num_channels) - 1)
        }
    }

    /// Returns `true` if the channel is marked as constant, `false`
    /// otherwise.
    ///
    /// `i` must be less than `64`.
    pub const fn is_channel_constant(&self, i: usize) -> bool {
        self.0 & (0b1 << i) != 0
    }

    /// Returns `true` if any channel is marked as constant, `false`
    /// otherwise.
    ///
    /// `num_channels` must be less than or equal to `64`.
    pub const fn any_channel_constant(&self, num_channels: usize) -> bool {
        if num_channels >= 64 {
            self.0 != 0
        } else {
            self.0 & ((0b1 << num_channels) - 1) != 0
        }
    }

    /// Returns `true` if all channels are marked as constant, `false`
    /// otherwise.
    ///
    /// `num_channels` must be less than or equal to `64`.
    pub const fn all_channels_constant(&self, num_channels: usize) -> bool {
        if num_channels >= 64 {
            self.0 == u64::MAX
        } else {
            let mask = (0b1 << num_channels) - 1;
            self.0 & mask == mask
        }
    }

    /// Returns `true` if all channels in the given range are marked
    /// as constant, `false` otherwise.
    ///
    /// This range must be in the range `[0, 64]`
    pub const fn range_constant(&self, range: Range<usize>) -> bool {
        if range.start >= 64 {
            false
        } else if range.end >= 64 {
            let mask = !((0b1 << range.start) - 1);
            self.0 & mask == mask
        } else {
            let mask = ((0b1 << range.end) - 1) & !((0b1 << range.start) - 1);
            self.0 & mask == mask
        }
    }

    /// Mark/un-mark the given channel as constant.
    ///
    /// `i` must be less than `64`.
    pub fn set_channel(&mut self, i: usize, constant: bool) {
        if constant {
            self.0 |= 0b1 << i;
        } else {
            self.0 &= !(0b1 << i);
        }
    }

    pub const fn union(self, other: Self) -> Self {
        ConstantMask(self.0 & other.0)
    }

    pub fn union_with(&mut self, other: Self) {
        self.0 &= other.0;
    }

    /// Convert this constant mask into a silence mask.
    pub fn to_silence_mask<V: AsRef<[f32]>>(self, channels: &[V]) -> SilenceMask {
        let mut silence_mask = SilenceMask::NONE_SILENT;
        for (i, ch) in channels.iter().enumerate() {
            let silent = self.is_channel_constant(i) && ch.as_ref()[0] == 0.0;
            silence_mask.set_channel(i, silent);
        }

        silence_mask
    }

    /// Convert this constant mask into a silence mask, assuming
    /// that none of the slices in `channels` are empty.
    ///
    /// # Safety
    /// All slices in `channels` must not be empty.
    pub unsafe fn to_silence_mask_unchecked<V: AsRef<[f32]>>(self, channels: &[V]) -> SilenceMask {
        let mut silence_mask = SilenceMask::NONE_SILENT;
        for (i, ch) in channels.iter().enumerate() {
            // SAFETY: the caller guarantees all slices are not empty, so they must have at least 1 element.
            let silent =
                self.is_channel_constant(i) && unsafe { *(ch.as_ref().get_unchecked(0)) == 0.0 };
            silence_mask.set_channel(i, silent);
        }

        silence_mask
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MaskType {
    Silence(SilenceMask),
    Constant(ConstantMask),
}

impl Default for MaskType {
    fn default() -> Self {
        MaskType::Silence(Default::default())
    }
}

/// An optional hint on which channels are connected to other
/// nodes in the graph. A bit set to `1` means that channel
/// is connected to another node, and a bit set to `0` means
/// that channel is not connected to any node.
///
/// The first bit (`0x1`) is the first channel, the second bit
/// is the second channel, and so on.
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ConnectedMask(pub u64);

impl ConnectedMask {
    pub const NONE_CONNECTED: Self = Self(0);
    pub const MONO_CONNECTED: Self = Self(0b1);
    pub const STEREO_CONNECTED: Self = Self(0b11);

    /// Returns `true` if the channel is connected to another node,
    /// `false` otherwise.
    ///
    /// `i` must be less than `64`.
    pub const fn is_channel_connected(&self, i: usize) -> bool {
        self.0 & (0b1 << i) != 0
    }

    /// Returns `true` if all channels are marked as connected, `false`
    /// otherwise.
    ///
    /// `num_channels` must be less than or equal to `64`.
    pub const fn all_channels_connected(&self, num_channels: usize) -> bool {
        if num_channels >= 64 {
            self.0 == u64::MAX
        } else {
            let mask = (0b1 << num_channels) - 1;
            self.0 & mask == mask
        }
    }

    /// Mark/un-mark the given channel as connected.
    ///
    /// `i` must be less than `64`.
    pub fn set_channel(&mut self, i: usize, connected: bool) {
        if connected {
            self.0 |= 0b1 << i;
        } else {
            self.0 &= !(0b1 << i);
        }
    }
}
