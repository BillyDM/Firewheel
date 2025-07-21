use core::any::Any;

pub use glam::{Vec2, Vec3};

use crate::{
    clock::{
        DurationMusical, DurationSamples, DurationSeconds, EventInstant, InstantMusical,
        InstantSamples, InstantSeconds,
    },
    collector::{ArcGc, OwnedGc},
    diff::{Notify, ParamPath},
    dsp::volume::Volume,
    node::NodeID,
};

/// An event sent to an [`AudioNodeProcessor`][crate::node::AudioNodeProcessor].
pub struct NodeEvent {
    /// The ID of the node that should receive the event.
    pub node_id: NodeID,
    /// Optionally, a time to schedule this event at. If `None`, the event is considered
    /// to be at the start of the next processing period.
    pub time: Option<EventInstant>,
    /// The type of event.
    pub event: NodeEventType,
}

impl NodeEvent {
    pub const DUMMY: Self = Self {
        node_id: NodeID::DANGLING,
        time: None,
        event: NodeEventType::Dummy,
    };
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
    /// Custom event type stored on the heap.
    Custom(OwnedGc<Box<dyn Any + Send + 'static>>),
    /// Custom event type stored on the stack as raw bytes.
    CustomBytes([u8; 36]),
    /// Event which does nothing. Used internally.
    Dummy,
}

impl NodeEventType {
    pub fn custom<T: Send + 'static>(value: T) -> Self {
        Self::Custom(OwnedGc::new(Box::new(value)))
    }

    /// Try to downcast the custom event to an immutable reference to `T`.
    ///
    /// If this does not contain [`NodeEventType::Custom`] or if the
    /// downcast failed, then this returns `None`.
    pub fn downcast_ref<T: Send + 'static>(&self) -> Option<&T> {
        if let Self::Custom(owned) = self {
            owned.downcast_ref()
        } else {
            None
        }
    }

    /// Try to downcast the custom event to a mutable reference to `T`.
    ///
    /// If this does not contain [`NodeEventType::Custom`] or if the
    /// downcast failed, then this returns `None`.
    pub fn downcast_mut<T: Send + 'static>(&mut self) -> Option<&mut T> {
        if let Self::Custom(owned) = self {
            owned.downcast_mut()
        } else {
            None
        }
    }
}

/// Data that can be used to patch an individual parameter.
#[non_exhaustive]
pub enum ParamData {
    F32(f32),
    F64(f64),
    I32(i32),
    U32(u32),
    I64(i64),
    U64(u64),
    Bool(bool),
    Volume(Volume),
    Vector2D(Vec2),
    Vector3D(Vec3),

    NotifyF32(Notify<f32>),
    NotifyF64(Notify<f64>),
    NotifyI32(Notify<i32>),
    NotifyU32(Notify<u32>),
    NotifyI64(Notify<i64>),
    NotifyU64(Notify<u64>),
    NotifyBool(Notify<bool>),

    EventInstant(EventInstant),
    InstantSeconds(InstantSeconds),
    DurationSeconds(DurationSeconds),
    InstantSamples(InstantSamples),
    DurationSamples(DurationSamples),
    InstantMusical(InstantMusical),
    DurationMusical(DurationMusical),

    /// Custom type stored on the heap.
    Any(ArcGc<dyn Any + Send + Sync>),

    /// Custom type stored on the stack as raw bytes.
    CustomBytes([u8; 20]),

    /// No data (i.e. the type is `None`).
    None,
}

impl ParamData {
    /// Construct a [`ParamData::Any`] variant.
    pub fn any<T: Send + Sync + 'static>(value: T) -> Self {
        Self::Any(ArcGc::new_any(value))
    }

    /// Construct a [`ParamData::OptAny`] variant.
    pub fn opt_any<T: Any + Send + Sync + 'static>(value: Option<T>) -> Self {
        if let Some(value) = value {
            Self::any(value)
        } else {
            Self::None
        }
    }

    /// Try to downcast [`ParamData::Any`] into `T`.
    ///
    /// If this enum doesn't hold [`ParamData::Any`] or the downcast fails,
    /// then this returns `None`.
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

        impl TryInto<$ty> for ParamData {
            type Error = crate::diff::PatchError;

            fn try_into(self) -> Result<$ty, crate::diff::PatchError> {
                match self {
                    ParamData::$variant(value) => Ok(value),
                    _ => Err(crate::diff::PatchError::InvalidData),
                }
            }
        }

        impl From<Option<$ty>> for ParamData {
            fn from(value: Option<$ty>) -> Self {
                if let Some(value) = value {
                    Self::$variant(value)
                } else {
                    Self::None
                }
            }
        }

        impl TryInto<Option<$ty>> for ParamData {
            type Error = crate::diff::PatchError;

            fn try_into(self) -> Result<Option<$ty>, crate::diff::PatchError> {
                match self {
                    ParamData::$variant(value) => Ok(Some(value)),
                    ParamData::None => Ok(None),
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
param_data_from!(u32, U32);
param_data_from!(i64, I64);
param_data_from!(u64, U64);
param_data_from!(bool, Bool);
param_data_from!(Vec2, Vector2D);
param_data_from!(Vec3, Vector3D);
param_data_from!(Notify<f32>, NotifyF32);
param_data_from!(Notify<f64>, NotifyF64);
param_data_from!(Notify<i32>, NotifyI32);
param_data_from!(Notify<u32>, NotifyU32);
param_data_from!(Notify<i64>, NotifyI64);
param_data_from!(Notify<u64>, NotifyU64);
param_data_from!(Notify<bool>, NotifyBool);
param_data_from!(EventInstant, EventInstant);
param_data_from!(InstantSeconds, InstantSeconds);
param_data_from!(DurationSeconds, DurationSeconds);
param_data_from!(InstantSamples, InstantSamples);
param_data_from!(DurationSamples, DurationSamples);
param_data_from!(InstantMusical, InstantMusical);
param_data_from!(DurationMusical, DurationMusical);

/// A list of events for an [`AudioNodeProcessor`][crate::node::AudioNodeProcessor].
pub struct NodeEventList<'a> {
    immediate_event_buffer: &'a mut [Option<NodeEvent>],
    scheduled_event_arena: &'a mut [Option<NodeEvent>],
    indices: &'a mut Vec<NodeEventListIndex>,
}

impl<'a> NodeEventList<'a> {
    pub fn new(
        immediate_event_buffer: &'a mut [Option<NodeEvent>],
        scheduled_event_arena: &'a mut [Option<NodeEvent>],
        indices: &'a mut Vec<NodeEventListIndex>,
    ) -> Self {
        Self {
            immediate_event_buffer,
            scheduled_event_arena,
            indices,
        }
    }

    pub fn num_events(&self) -> usize {
        self.indices.len()
    }

    /// Iterate over all events, draining the events from the list.
    pub fn drain<'b>(&'b mut self) -> impl IntoIterator<Item = NodeEventType> + use<'b> {
        self.indices.drain(..).map(|i1| match i1 {
            NodeEventListIndex::Immediate(i2) => {
                self.immediate_event_buffer[i2 as usize]
                    .take()
                    .unwrap()
                    .event
            }
            NodeEventListIndex::Scheduled(i2) => {
                self.scheduled_event_arena[i2 as usize]
                    .take()
                    .unwrap()
                    .event
            }
        })
    }

    /// Iterate over patches for `T`, draining the events from the list.
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
    /// let mut node = FilterNode::default();
    ///
    /// // You can match on individual patch variants.
    /// for patch in event_list.iter_patch::<FilterNode>() {
    ///     match patch {
    ///         FilterNodePatch::Frequency(frequency) => {
    ///             node.frequency = frequency;
    ///         }
    ///         FilterNodePatch::Quality(quality) => {
    ///             node.quality = quality;
    ///         }
    ///     }
    /// }
    ///
    /// // Or simply apply all of them.
    /// for patch in event_list.iter_patch::<FilterNode>() { node.apply(patch); }
    /// # }
    /// ```
    ///
    /// Errors produced while constructing patches are simply skipped.
    pub fn drain_patches<'b, T: crate::diff::Patch>(
        &'b mut self,
    ) -> impl IntoIterator<Item = <T as crate::diff::Patch>::Patch> + use<'b, T> {
        // Ideally this would parameterise the `FnMut` over some `impl From<PatchEvent<T>>`
        // but it would require a marker trait for the `diff::Patch::Patch` assoc type to
        // prevent overlapping impls.
        self.drain().into_iter().filter_map(|e| T::patch_event(e))
    }
}

/// Used internally by the Firewheel processor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeEventListIndex {
    Immediate(u32),
    Scheduled(u32),
}
