//! A set of diff and patch implementations for common collections.

use super::{Diff, EventQueue, Patch, PatchError, PathBuilder};
use crate::event::ParamData;

macro_rules! sequence_diff {
    ($gen:ident, $ty:ty) => {
        impl<$gen: Diff> Diff for $ty {
            fn diff<E: EventQueue>(&self, baseline: &Self, path: PathBuilder, event_queue: &mut E) {
                for (i, item) in self.iter().enumerate() {
                    item.diff(&baseline[i], path.with(i as u32), event_queue);
                }
            }
        }

        impl<$gen: Patch> Patch for $ty {
            type Patch = (usize, $gen::Patch);

            fn patch(data: ParamData, path: &[u32]) -> Result<Self::Patch, PatchError> {
                let first = *path.first().ok_or(PatchError::InvalidPath)?;
                let inner = $gen::patch(data, &path[1..])?;

                Ok((first as usize, inner))
            }

            fn apply(&mut self, patch: Self::Patch) {
                self[patch.0].apply(patch.1);
            }
        }
    };
}

sequence_diff!(T, Vec<T>);
sequence_diff!(T, Box<[T]>);
sequence_diff!(T, [T]);

impl<T: Diff, const LEN: usize> Diff for [T; LEN] {
    fn diff<E: EventQueue>(&self, baseline: &Self, path: PathBuilder, event_queue: &mut E) {
        for (i, item) in self.iter().enumerate() {
            item.diff(&baseline[i], path.with(i as u32), event_queue);
        }
    }
}

impl<T: Patch, const LEN: usize> Patch for [T; LEN] {
    type Patch = (usize, T::Patch);

    fn patch(data: ParamData, path: &[u32]) -> Result<Self::Patch, PatchError> {
        let first = *path.first().ok_or(PatchError::InvalidPath)? as usize;
        if first >= LEN {
            return Err(PatchError::InvalidPath);
        }
        let inner = T::patch(data, &path[1..])?;

        Ok((first, inner))
    }

    fn apply(&mut self, patch: Self::Patch) {
        self[patch.0].apply(patch.1);
    }
}

macro_rules! tuple_diff {
    ($tup:ident, $($gen:ident, $base:ident, $index:literal),*) => {
        pub enum $tup<$($gen: Patch),*> {
            $($gen($gen::Patch)),*
        }

        #[allow(non_snake_case, unused_variables)]
        impl<$($gen: Diff),*> Diff for ($($gen,)*) {
            fn diff<EQ: EventQueue>(&self, baseline: &Self, path: PathBuilder, event_queue: &mut EQ) {
                let ($($gen,)*) = self;
                let ($($base,)*) = baseline;

                $(
                    $gen.diff($base, path.with($index), event_queue);
                )*
            }
        }

        #[allow(non_snake_case, unused_variables)]
        impl<$($gen: Patch),*> Patch for ($($gen,)*) {
            type Patch = $tup<$($gen),*>;

            fn patch(data: ParamData, path: &[u32]) -> Result<Self::Patch, PatchError> {
                match path {
                    $(
                        [$index, tail @ ..] => Ok($tup::$gen($gen::patch(data, tail)?)),
                    )*
                    _ => Err(PatchError::InvalidPath),
                }
            }

            fn apply(&mut self, patch: Self::Patch) {
                let ($($gen,)*) = self;

                match patch {
                    $(
                        $tup::$gen(p) => $gen.apply(p)
                    ),*
                }
            }
        }
    };
}

tuple_diff!(Tuple1, A, A1, 0);
tuple_diff!(Tuple2, A, A1, 0, B, B1, 1);
tuple_diff!(Tuple3, A, A1, 0, B, B1, 1, C, C1, 2);
tuple_diff!(Tuple4, A, A1, 0, B, B1, 1, C, C1, 2, D, D1, 3);
tuple_diff!(Tuple5, A, A1, 0, B, B1, 1, C, C1, 2, D, D1, 3, E, E1, 4);
tuple_diff!(Tuple6, A, A1, 0, B, B1, 1, C, C1, 2, D, D1, 3, E, E1, 4, F, F1, 5);
tuple_diff!(Tuple7, A, A1, 0, B, B1, 1, C, C1, 2, D, D1, 3, E, E1, 4, F, F1, 5, G, G1, 6);
tuple_diff!(Tuple8, A, A1, 0, B, B1, 1, C, C1, 2, D, D1, 3, E, E1, 4, F, F1, 5, G, G1, 6, H, H1, 7);
