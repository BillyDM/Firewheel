use core::any::Any;

pub use glam::{Vec2, Vec3};

use crate::{diff::ParamPath, dsp::volume::Volume, node::NodeID};

/// An event sent to an [`AudioNodeProcessor`][crate::node::AudioNodeProcessor].
pub struct NodeEvent {
    /// The ID of the node that should receive the event.
    pub node_id: NodeID,
    /// The type of event.
    pub event: NodeEventType,
}

/// An event type associated with an [`AudioNodeProcessor`][crate::node::AudioNodeProcessor].
#[non_exhaustive]
pub enum NodeEventType {
    Param {
        /// Data for a specific parameter.
        data: ParamData,
        /// The path to the parameter.
        path: ParamPath,
    },
    /// Custom event type.
    Custom(Box<dyn Any + Send + Sync>),
    /// Custom event type stored on the stack as raw bytes.
    CustomBytes([u8; 16]),
}

/// Data that can be used to patch an individual parameter.
///
/// The [`ParamData::Any`] variant is double-boxed to keep
/// its size small on the stack.
#[non_exhaustive]
pub enum ParamData {
    Volume(Volume),
    F32(f32),
    F64(f64),
    I32(i32),
    I64(i64),
    U32(u32),
    U64(u64),
    Bool(bool),
    Vector2D(Vec2),
    Vector3D(Vec3),
    Any(Box<Box<dyn Any + Send + Sync>>),
}

impl ParamData {
    /// Construct a [`ParamData::Any`] variant.
    pub fn any<T: Any + Send + Sync>(value: T) -> Self {
        Self::Any(Box::new(Box::new(value)))
    }

    /// Try to downcast [`ParamData::Any`] into `T`.
    ///
    /// If this enum doesn't hold [`ParamData::Any`] or
    /// the downcast fails, this returns `None`.
    pub fn downcast_ref<T: Any>(&self) -> Option<&T> {
        match self {
            Self::Any(any) => any.downcast_ref(),
            _ => None,
        }
    }
}

macro_rules! param_data_from {
    ($ty:ty, $variant:ident) => {
        impl From<$ty> for ParamData {
            fn from(value: $ty) -> Self {
                Self::$variant(value)
            }
        }

        impl TryInto<$ty> for &ParamData {
            type Error = crate::diff::PatchError;

            fn try_into(self) -> Result<$ty, crate::diff::PatchError> {
                match self {
                    ParamData::$variant(value) => Ok(*value),
                    _ => Err(crate::diff::PatchError::InvalidData),
                }
            }
        }
    };
}

param_data_from!(Volume, Volume);
param_data_from!(f32, F32);
param_data_from!(f64, F64);
param_data_from!(i32, I32);
param_data_from!(i64, I64);
param_data_from!(u32, U32);
param_data_from!(u64, U64);
param_data_from!(bool, Bool);
param_data_from!(Vec2, Vector2D);
param_data_from!(Vec3, Vector3D);

/// A list of events for an [`AudioNodeProcessor`][crate::node::AudioNodeProcessor].
pub struct NodeEventList<'a> {
    event_buffer: &'a mut [NodeEvent],
    indices: &'a [u32],
}

impl<'a> NodeEventList<'a> {
    pub fn new(event_buffer: &'a mut [NodeEvent], indices: &'a [u32]) -> Self {
        Self {
            event_buffer,
            indices,
        }
    }

    pub fn num_events(&self) -> usize {
        self.indices.len()
    }

    pub fn get_event(&mut self, index: usize) -> Option<&mut NodeEventType> {
        self.indices
            .get(index)
            .map(|idx| &mut self.event_buffer[*idx as usize].event)
    }

    pub fn for_each(&mut self, mut f: impl FnMut(&mut NodeEventType)) {
        for &idx in self.indices {
            if let Some(event) = self.event_buffer.get_mut(idx as usize) {
                (f)(&mut event.event);
            }
        }
    }

    /// Iterate over patches for `T`.
    ///
    /// ```
    /// # use firewheel_core::{diff::*, event::NodeEventList};
    /// # fn for_each_example(mut event_list: NodeEventList) {
    /// #[derive(Patch, Default)]
    /// struct FilterNode {
    ///     frequency: f32,
    ///     quality: f32,
    /// }
    ///
    /// // You can match on individual patch variants.
    /// event_list.for_each_patch::<FilterNode>(|patch| match patch {
    ///     FilterNodePatch::Frequency(frequency) => {}
    ///     FilterNodePatch::Quality(quality) => {}
    /// });
    ///
    /// // Or simply apply all of them.
    /// let mut node = FilterNode::default();
    /// event_list.for_each_patch::<FilterNode>(|patch| node.apply(patch));
    /// # }
    /// ```
    ///
    /// Errors produced while constructing patches are simply skipped.
    pub fn for_each_patch<T: crate::diff::Patch>(&mut self, mut f: impl FnMut(T::Patch)) {
        for &idx in self.indices {
            if let Some(patch) = self
                .event_buffer
                .get_mut(idx as usize)
                .and_then(|e| T::patch_event(&e.event))
            {
                (f)(patch);
            }
        }
    }
}
