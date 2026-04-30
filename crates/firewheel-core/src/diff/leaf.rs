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

impl Diff for () {
    fn diff<E: EventQueue>(&self, _baseline: &Self, _path: PathBuilder, _event_queue: &mut E) {}
}

impl Patch for () {
    type Patch = ();

    fn patch(data: &ParamData, _path: &[u32]) -> Result<Self::Patch, PatchError> {
        match data {
            ParamData::None => Ok(()),
            _ => Err(PatchError::InvalidData),
        }
    }

    fn apply(&mut self, _patch: Self::Patch) {}
}

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
            let mut bytes: [u8; 20] = [0; 20];
            bytes[0..8].copy_from_slice(&self.id().to_ne_bytes());
            bytes[8] = (**self) as u8;

            event_queue.push_param(ParamData::CustomBytes(bytes), path);
        }
    }
}

impl Patch for Notify<bool> {
    type Patch = Self;

    fn patch(data: &ParamData, _path: &[u32]) -> Result<Self::Patch, PatchError> {
        match data {
            ParamData::CustomBytes(bytes) => {
                let (counter_bytes, rest_bytes) = bytes.split_at(size_of::<u64>());
                let counter = u64::from_ne_bytes(counter_bytes.try_into().unwrap());

                let value = rest_bytes[0] != 0;

                Ok(Notify::from_raw(value, counter))
            }
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
                    let mut bytes: [u8; 20] = [0; 20];
                    bytes[0..8].copy_from_slice(&self.id().to_ne_bytes());
                    let value_bytes = self.to_ne_bytes();
                    bytes[8..8 + value_bytes.len()].copy_from_slice(&value_bytes);

                    event_queue.push_param(ParamData::CustomBytes(bytes), path);
                }
            }
        }

        impl Patch for Notify<$ty> {
            type Patch = Self;

            fn patch(data: &ParamData, _path: &[u32]) -> Result<Self::Patch, PatchError> {
                match data {
                    ParamData::CustomBytes(bytes) => {
                        let (counter_bytes, rest_bytes) = bytes.split_at(size_of::<u64>());
                        let counter = u64::from_ne_bytes(counter_bytes.try_into().unwrap());

                        let (value_bytes, _) = rest_bytes.split_at(size_of::<$ty>());
                        let value = <$ty>::from_ne_bytes(value_bytes.try_into().unwrap());

                        Ok(Notify::from_raw(value, counter))
                    }
                    _ => Err(PatchError::InvalidData),
                }
            }

            fn apply(&mut self, value: Self::Patch) {
                *self = value;
            }
        }
    };
}

macro_rules! non_trivial_notify {
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

trivial_notify!(i8);
trivial_notify!(u8);
trivial_notify!(i16);
trivial_notify!(u16);
trivial_notify!(i32);
trivial_notify!(u32);
trivial_notify!(i64);
trivial_notify!(u64);
trivial_notify!(f32);
trivial_notify!(f64);

// No good optimizations possible for these large values.
non_trivial_notify!(Volume);
non_trivial_notify!(InstantSamples);
non_trivial_notify!(DurationSamples);
non_trivial_notify!(InstantSeconds);
non_trivial_notify!(DurationSeconds);

#[cfg(feature = "musical_transport")]
non_trivial_notify!(InstantMusical);
#[cfg(feature = "musical_transport")]
non_trivial_notify!(DurationMusical);

non_trivial_notify!(Vec2);
non_trivial_notify!(Vec3);

#[cfg(feature = "glam-29")]
non_trivial_notify!(glam_29::Vec2);
#[cfg(feature = "glam-29")]
non_trivial_notify!(glam_29::Vec3);

#[cfg(feature = "glam-30")]
non_trivial_notify!(glam_30::Vec2);
#[cfg(feature = "glam-30")]
non_trivial_notify!(glam_30::Vec3);
