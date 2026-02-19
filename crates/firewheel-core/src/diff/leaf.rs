//! A set of diff and patch implementations for common leaf types.

use super::{Diff, EventQueue, Patch, PatchError, PathBuilder};
use crate::{
    clock::{DurationSamples, DurationSeconds, InstantSamples, InstantSeconds},
    collector::ArcGc,
    diff::{Notify, RealtimeClone},
    dsp::volume::Volume,
    event::{NodeEventType, ParamData},
    vector::{Vec2, Vec3},
};

#[cfg(feature = "musical_transport")]
use crate::clock::{DurationMusical, InstantMusical};

macro_rules! primitive_diff {
    ($ty:ty, $variant:ident) => {
        impl Diff for $ty {
            fn diff<E: EventQueue>(&self, baseline: &Self, path: PathBuilder, event_queue: &mut E) {
                if self != baseline {
                    event_queue.push_param(*self, path);
                }
            }
        }

        impl Patch for $ty {
            type Patch = Self;

            fn patch(data: &ParamData, _: &[u32]) -> Result<Self::Patch, PatchError> {
                match data {
                    ParamData::$variant(value) => Ok((*value).into()),
                    _ => Err(PatchError::InvalidData),
                }
            }

            fn apply(&mut self, value: Self::Patch) {
                *self = value;
            }
        }

        impl Diff for Option<$ty> {
            fn diff<E: EventQueue>(&self, baseline: &Self, path: PathBuilder, event_queue: &mut E) {
                if self != baseline {
                    event_queue.push_param(*self, path);
                }
            }
        }

        impl Patch for Option<$ty> {
            type Patch = Self;

            fn patch(data: &ParamData, _: &[u32]) -> Result<Self::Patch, PatchError> {
                match data {
                    ParamData::$variant(value) => Ok(Some((*value).into())),
                    ParamData::None => Ok(None),
                    _ => Err(PatchError::InvalidData),
                }
            }

            fn apply(&mut self, value: Self::Patch) {
                *self = value;
            }
        }
    };

    ($ty:ty, $cast:ty, $variant:ident) => {
        impl Diff for $ty {
            fn diff<E: EventQueue>(&self, baseline: &Self, path: PathBuilder, event_queue: &mut E) {
                if self != baseline {
                    event_queue.push_param(*self as $cast, path);
                }
            }
        }

        impl Patch for $ty {
            type Patch = Self;

            fn patch(data: &ParamData, _: &[u32]) -> Result<Self::Patch, PatchError> {
                match data {
                    ParamData::$variant(value) => Ok(value.clone() as $ty),
                    _ => Err(PatchError::InvalidData),
                }
            }

            fn apply(&mut self, value: Self::Patch) {
                *self = value;
            }
        }

        impl Diff for Option<$ty> {
            fn diff<E: EventQueue>(&self, baseline: &Self, path: PathBuilder, event_queue: &mut E) {
                if self != baseline {
                    event_queue.push_param(self.map(|v| v as $cast), path);
                }
            }
        }

        impl Patch for Option<$ty> {
            type Patch = Self;

            fn patch(data: &ParamData, _: &[u32]) -> Result<Self::Patch, PatchError> {
                match data {
                    ParamData::$variant(value) => Ok(Some(value.clone() as $ty)),
                    ParamData::None => Ok(None),
                    _ => Err(PatchError::InvalidData),
                }
            }

            fn apply(&mut self, value: Self::Patch) {
                *self = value;
            }
        }
    };
}

primitive_diff!(bool, Bool);
primitive_diff!(u8, u32, U32);
primitive_diff!(u16, u32, U32);
primitive_diff!(u32, U32);
primitive_diff!(u64, U64);
primitive_diff!(i8, i32, I32);
primitive_diff!(i16, i32, I32);
primitive_diff!(i32, I32);
primitive_diff!(i64, I64);
primitive_diff!(usize, u64, U64);
primitive_diff!(isize, i64, I64);
primitive_diff!(f32, F32);
primitive_diff!(f64, F64);
primitive_diff!(Volume, Volume);
primitive_diff!(InstantSamples, InstantSamples);
primitive_diff!(DurationSamples, DurationSamples);
primitive_diff!(InstantSeconds, InstantSeconds);
primitive_diff!(DurationSeconds, DurationSeconds);

#[cfg(feature = "musical_transport")]
primitive_diff!(InstantMusical, InstantMusical);
#[cfg(feature = "musical_transport")]
primitive_diff!(DurationMusical, DurationMusical);

primitive_diff!(Vec2, Vector2D);
primitive_diff!(Vec3, Vector3D);

#[cfg(feature = "glam-29")]
primitive_diff!(glam_29::Vec2, Vector2D);
#[cfg(feature = "glam-29")]
primitive_diff!(glam_29::Vec3, Vector3D);

#[cfg(feature = "glam-30")]
primitive_diff!(glam_30::Vec2, Vector2D);
#[cfg(feature = "glam-30")]
primitive_diff!(glam_30::Vec3, Vector3D);

impl<A: ?Sized + Send + Sync + 'static> Diff for ArcGc<A> {
    fn diff<E: EventQueue>(&self, baseline: &Self, path: PathBuilder, event_queue: &mut E) {
        if !ArcGc::ptr_eq(self, baseline) {
            event_queue.push(NodeEventType::Param {
                data: ParamData::any(self.clone()),
                path: path.build(),
            });
        }
    }
}

impl<A: ?Sized + Send + Sync + 'static> Patch for ArcGc<A> {
    type Patch = Self;

    fn patch(data: &ParamData, _: &[u32]) -> Result<Self::Patch, PatchError> {
        if let ParamData::Any(any) = data {
            if let Some(data) = any.downcast_ref::<Self>() {
                return Ok(data.clone());
            }
        }

        Err(PatchError::InvalidData)
    }

    fn apply(&mut self, patch: Self::Patch) {
        *self = patch;
    }
}

impl<T: Send + Sync + RealtimeClone + PartialEq + 'static> Diff for Option<T> {
    fn diff<E: EventQueue>(&self, baseline: &Self, path: PathBuilder, event_queue: &mut E) {
        if self != baseline {
            event_queue.push_param(ParamData::opt_any(self.clone()), path);
        }
    }
}

impl<T: Send + Sync + RealtimeClone + PartialEq + 'static> Patch for Option<T> {
    type Patch = Self;

    fn patch(data: &ParamData, _: &[u32]) -> Result<Self::Patch, PatchError> {
        Ok(data.downcast_ref::<T>().cloned())
    }

    fn apply(&mut self, patch: Self::Patch) {
        *self = patch;
    }
}

// Here we specialize the `Notify` implementations since most
// primitives can have some number of optimizations applied.
impl Diff for Notify<()> {
    fn diff<E: EventQueue>(&self, baseline: &Self, path: PathBuilder, event_queue: &mut E) {
        if self != baseline {
            event_queue.push_param(ParamData::U64(self.id()), path);
        }
    }
}

impl Patch for Notify<()> {
    type Patch = Self;

    fn patch(data: &ParamData, _: &[u32]) -> Result<Self::Patch, PatchError> {
        match data {
            ParamData::U64(counter) => Ok(Notify::from_raw((), *counter)),
            _ => Err(PatchError::InvalidData),
        }
    }

    fn apply(&mut self, value: Self::Patch) {
        *self = value;
    }
}

impl Diff for Notify<bool> {
    fn diff<E: EventQueue>(&self, baseline: &Self, path: PathBuilder, event_queue: &mut E) {
        if self != baseline {
            event_queue.push_param(ParamData::U64(self.id()), path.with(**self as u32));
        }
    }
}

impl Patch for Notify<bool> {
    type Patch = Self;

    fn patch(data: &ParamData, path: &[u32]) -> Result<Self::Patch, PatchError> {
        let value = match path.first() {
            Some(0) => false,
            Some(1) => true,
            _ => return Err(PatchError::InvalidData),
        };

        match data {
            ParamData::U64(counter) => Ok(Notify::from_raw(value, *counter)),
            _ => Err(PatchError::InvalidData),
        }
    }

    fn apply(&mut self, value: Self::Patch) {
        *self = value;
    }
}

impl Diff for Notify<i8> {
    fn diff<E: EventQueue>(&self, baseline: &Self, path: PathBuilder, event_queue: &mut E) {
        if self != baseline {
            event_queue.push_param(ParamData::U64(self.id()), path.with(**self as i32 as u32));
        }
    }
}

impl Patch for Notify<i8> {
    type Patch = Self;

    fn patch(data: &ParamData, path: &[u32]) -> Result<Self::Patch, PatchError> {
        let value = (*path.first().ok_or(PatchError::InvalidData)?) as i8;

        match data {
            ParamData::U64(counter) => Ok(Notify::from_raw(value, *counter)),
            _ => Err(PatchError::InvalidData),
        }
    }

    fn apply(&mut self, value: Self::Patch) {
        *self = value;
    }
}

impl Diff for Notify<i16> {
    fn diff<E: EventQueue>(&self, baseline: &Self, path: PathBuilder, event_queue: &mut E) {
        if self != baseline {
            event_queue.push_param(ParamData::U64(self.id()), path.with(**self as i32 as u32));
        }
    }
}

impl Patch for Notify<i16> {
    type Patch = Self;

    fn patch(data: &ParamData, path: &[u32]) -> Result<Self::Patch, PatchError> {
        let value = (*path.first().ok_or(PatchError::InvalidData)?) as i16;

        match data {
            ParamData::U64(counter) => Ok(Notify::from_raw(value, *counter)),
            _ => Err(PatchError::InvalidData),
        }
    }

    fn apply(&mut self, value: Self::Patch) {
        *self = value;
    }
}

impl Diff for Notify<i32> {
    fn diff<E: EventQueue>(&self, baseline: &Self, path: PathBuilder, event_queue: &mut E) {
        if self != baseline {
            event_queue.push_param(ParamData::U64(self.id()), path.with(**self as u32));
        }
    }
}

impl Patch for Notify<i32> {
    type Patch = Self;

    fn patch(data: &ParamData, path: &[u32]) -> Result<Self::Patch, PatchError> {
        let value = (*path.first().ok_or(PatchError::InvalidData)?) as i32;

        match data {
            ParamData::U64(counter) => Ok(Notify::from_raw(value, *counter)),
            _ => Err(PatchError::InvalidData),
        }
    }

    fn apply(&mut self, value: Self::Patch) {
        *self = value;
    }
}

impl Diff for Notify<u8> {
    fn diff<E: EventQueue>(&self, baseline: &Self, path: PathBuilder, event_queue: &mut E) {
        if self != baseline {
            event_queue.push_param(ParamData::U64(self.id()), path.with(**self as u32));
        }
    }
}

impl Patch for Notify<u8> {
    type Patch = Self;

    fn patch(data: &ParamData, path: &[u32]) -> Result<Self::Patch, PatchError> {
        let value = (*path.first().ok_or(PatchError::InvalidData)?) as u8;

        match data {
            ParamData::U64(counter) => Ok(Notify::from_raw(value, *counter)),
            _ => Err(PatchError::InvalidData),
        }
    }

    fn apply(&mut self, value: Self::Patch) {
        *self = value;
    }
}

impl Diff for Notify<u16> {
    fn diff<E: EventQueue>(&self, baseline: &Self, path: PathBuilder, event_queue: &mut E) {
        if self != baseline {
            event_queue.push_param(ParamData::U64(self.id()), path.with(**self as u32));
        }
    }
}

impl Patch for Notify<u16> {
    type Patch = Self;

    fn patch(data: &ParamData, path: &[u32]) -> Result<Self::Patch, PatchError> {
        let value = (*path.first().ok_or(PatchError::InvalidData)?) as u16;

        match data {
            ParamData::U64(counter) => Ok(Notify::from_raw(value, *counter)),
            _ => Err(PatchError::InvalidData),
        }
    }

    fn apply(&mut self, value: Self::Patch) {
        *self = value;
    }
}

impl Diff for Notify<u32> {
    fn diff<E: EventQueue>(&self, baseline: &Self, path: PathBuilder, event_queue: &mut E) {
        if self != baseline {
            event_queue.push_param(ParamData::U64(self.id()), path.with(**self));
        }
    }
}

impl Patch for Notify<u32> {
    type Patch = Self;

    fn patch(data: &ParamData, path: &[u32]) -> Result<Self::Patch, PatchError> {
        let value = *path.first().ok_or(PatchError::InvalidData)?;

        match data {
            ParamData::U64(counter) => Ok(Notify::from_raw(value, *counter)),
            _ => Err(PatchError::InvalidData),
        }
    }

    fn apply(&mut self, value: Self::Patch) {
        *self = value;
    }
}

impl Diff for Notify<f32> {
    fn diff<E: EventQueue>(&self, baseline: &Self, path: PathBuilder, event_queue: &mut E) {
        if self != baseline {
            let value: f32 = **self;
            event_queue.push_param(ParamData::U64(self.id()), path.with(value.to_bits()));
        }
    }
}

impl Patch for Notify<f32> {
    type Patch = Self;

    fn patch(data: &ParamData, path: &[u32]) -> Result<Self::Patch, PatchError> {
        let value = *path.first().ok_or(PatchError::InvalidData)?;

        match data {
            ParamData::U64(counter) => Ok(Notify::from_raw(f32::from_bits(value), *counter)),
            _ => Err(PatchError::InvalidData),
        }
    }

    fn apply(&mut self, value: Self::Patch) {
        *self = value;
    }
}

macro_rules! trivial_notify {
    ($ty:path) => {
        impl Diff for Notify<$ty> {
            fn diff<E: EventQueue>(&self, baseline: &Self, path: PathBuilder, event_queue: &mut E) {
                if self != baseline {
                    event_queue.push_param(ParamData::any(self.clone()), path);
                }
            }
        }

        impl Patch for Notify<$ty> {
            type Patch = Self;

            fn patch(data: &ParamData, _: &[u32]) -> Result<Self::Patch, PatchError> {
                data.downcast_ref()
                    .ok_or(super::PatchError::InvalidData)
                    .cloned()
            }

            fn apply(&mut self, value: Self::Patch) {
                *self = value;
            }
        }
    };
}

// No good optimizations possible for these large values.
trivial_notify!(f64);
trivial_notify!(i64);
trivial_notify!(u64);

trivial_notify!(Volume);
trivial_notify!(InstantSamples);
trivial_notify!(DurationSamples);
trivial_notify!(InstantSeconds);
trivial_notify!(DurationSeconds);

#[cfg(feature = "musical_transport")]
trivial_notify!(InstantMusical);
#[cfg(feature = "musical_transport")]
trivial_notify!(DurationMusical);

trivial_notify!(Vec2);
trivial_notify!(Vec3);

#[cfg(feature = "glam-29")]
trivial_notify!(glam_29::Vec2);
#[cfg(feature = "glam-29")]
trivial_notify!(glam_29::Vec3);

#[cfg(feature = "glam-30")]
trivial_notify!(glam_30::Vec2);
#[cfg(feature = "glam-30")]
trivial_notify!(glam_30::Vec3);

#[cfg(feature = "glam-31")]
trivial_notify!(glam_31::Vec2);
#[cfg(feature = "glam-31")]
trivial_notify!(glam_31::Vec3);