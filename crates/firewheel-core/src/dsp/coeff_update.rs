use crate::{
    diff::{Diff, Patch},
    event::ParamData,
};

/// An exponent representing the rate at which DSP coefficients are
/// updated when parameters are being smoothed.
///
/// Smaller values will produce less "stair-stepping" artifacts,
/// but will also consume more CPU.
///
/// The resulting number of frames (samples in a single channel of audio)
/// that will elapse between each update is calculated as
/// `2^coeff_update_mask`.
///
/// By default this is set to `4`.
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "bevy_reflect", derive(bevy_reflect::Reflect))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CoeffUpdateFactor(pub u32);

impl CoeffUpdateFactor {
    pub const DEFAULT: Self = Self(4);

    pub fn interval_frames(&self) -> usize {
        2u32.pow(self.0) as usize
    }

    pub fn mask(&self) -> CoeffUpdateMask {
        CoeffUpdateMask((2u32.pow(self.0) - 1) as usize)
    }
}

impl Default for CoeffUpdateFactor {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl From<u32> for CoeffUpdateFactor {
    fn from(value: u32) -> Self {
        Self(value)
    }
}

impl From<CoeffUpdateFactor> for u32 {
    fn from(value: CoeffUpdateFactor) -> Self {
        value.0
    }
}

impl Diff for CoeffUpdateFactor {
    fn diff<E: crate::diff::EventQueue>(
        &self,
        baseline: &Self,
        path: crate::diff::PathBuilder,
        event_queue: &mut E,
    ) {
        if self != baseline {
            event_queue.push_param(ParamData::U32(self.0), path);
        }
    }
}

impl Patch for CoeffUpdateFactor {
    type Patch = Self;

    fn patch(data: &ParamData, _: &[u32]) -> Result<Self::Patch, crate::diff::PatchError> {
        match data {
            ParamData::U32(value) => Ok(Self(*value)),
            _ => Err(crate::diff::PatchError::InvalidData),
        }
    }

    fn apply(&mut self, patch: Self::Patch) {
        *self = patch;
    }
}

/// Used in conjunction with [`CoeffUpdateFactor`].
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CoeffUpdateMask(pub usize);

impl Default for CoeffUpdateMask {
    fn default() -> Self {
        CoeffUpdateFactor::default().mask()
    }
}

impl CoeffUpdateMask {
    #[inline(always)]
    pub fn do_update(&self, i: usize) -> bool {
        i & self.0 == 0
    }
}
