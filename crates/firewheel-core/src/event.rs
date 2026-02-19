use core::any::Any;

#[cfg(not(feature = "std"))]
use bevy_platform::prelude::{Box, Vec};

use crate::{
    clock::{DurationSamples, DurationSeconds, InstantSamples, InstantSeconds},
    collector::{ArcGc, OwnedGc},
    diff::{Notify, ParamPath},
    dsp::volume::Volume,
    node::NodeID,
    vector::{Vec2, Vec3},
};

#[cfg(feature = "midi_events")]
pub use wmidi;
#[cfg(feature = "midi_events")]
use wmidi::MidiMessage;

#[cfg(feature = "scheduled_events")]
use crate::clock::EventInstant;

#[cfg(feature = "musical_transport")]
use crate::clock::{DurationMusical, InstantMusical};

/// An event sent to an [`AudioNodeProcessor`][crate::node::AudioNodeProcessor].
#[derive(Debug)]
pub struct NodeEvent {
    /// The ID of the node that should receive the event.
    pub node_id: NodeID,
    /// Optionally, a time to schedule this event at. If `None`, the event is considered
    /// to be at the start of the next processing period.
    #[cfg(feature = "scheduled_events")]
    pub time: Option<EventInstant>,
    /// The type of event.
    pub event: NodeEventType,
}

impl NodeEvent {
    /// Construct an event to send to an [`AudioNodeProcessor`][crate::node::AudioNodeProcessor].
    ///
    /// * `node_id` - The ID of the node that should receive the event.
    /// * `event` - The type of event.
    pub const fn new(node_id: NodeID, event: NodeEventType) -> Self {
        Self {
            node_id,
            #[cfg(feature = "scheduled_events")]
            time: None,
            event,
        }
    }

    /// Construct a new scheduled event to send to an
    /// [`AudioNodeProcessor`][crate::node::AudioNodeProcessor].
    ///
    /// * `node_id` - The ID of the node that should receive the event.
    /// * `time` - The time to schedule this event at.
    /// * `event` - The type of event.
    #[cfg(feature = "scheduled_events")]
    pub const fn scheduled(node_id: NodeID, time: EventInstant, event: NodeEventType) -> Self {
        Self {
            node_id,
            time: Some(time),
            event,
        }
    }
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
    #[cfg(feature = "midi_events")]
    MIDI(MidiMessage<'static>),
}

impl NodeEventType {
    pub fn custom<T: Send + 'static>(value: T) -> Self {
        Self::Custom(OwnedGc::new(Box::new(value)))
    }

    pub fn custom_boxed<T: Send + 'static>(value: Box<T>) -> Self {
        Self::Custom(OwnedGc::new(value))
    }

    /// Try to downcast the custom event to an immutable reference to `T`.
    ///
    /// If this does not contain [`NodeEventType::Custom`] or if the
    /// downcast failed, then this returns `None`.
    pub fn downcast_ref<T: Send + 'static>(&self) -> Option<&T> {
        if let Self::Custom(owned) = self {
            owned.as_ref().downcast_ref()
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
            owned.as_mut().downcast_mut()
        } else {
            None
        }
    }

    /// Try to swap the contents of the custom event with the contents of
    /// the given value.
    ///
    /// If successful, the old contents that were stored in `value` will
    /// safely be dropped and deallocated on another non-realtime thread.
    ///
    /// Returns `true` if the value has been successfully swapped, `false`
    /// otherwise (i.e. the event did not contain [`NodeEventType::Custom`]
    /// or the downcast failed).
    pub fn downcast_swap<T: Send + 'static>(&mut self, value: &mut T) -> bool {
        if let Some(v) = self.downcast_mut::<T>() {
            core::mem::swap(v, value);
            true
        } else {
            false
        }
    }

    /// Try to swap the contents of the custom event with the contents of
    /// the given value wrapped in an [`OwnedGc`].
    ///
    /// If successful, the old contents that were stored in `value` will
    /// safely be dropped and deallocated on another non-realtime thread.
    ///
    /// Returns `true` if the value has been successfully swapped, `false`
    /// otherwise (i.e. the event did not contain [`NodeEventType::Custom`]
    /// or the downcast failed).
    pub fn downcast_into_owned<T: Send + 'static>(&mut self, value: &mut OwnedGc<T>) -> bool {
        if let Some(v) = self.downcast_mut::<T>() {
            value.swap(v);
            true
        } else {
            false
        }
    }
}

impl core::fmt::Debug for NodeEventType {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        match self {
            NodeEventType::Param { data, path } => f
                .debug_struct("Param")
                .field("data", &data)
                .field("path", &path)
                .finish(),
            NodeEventType::Custom(_) => f.debug_tuple("Custom").finish_non_exhaustive(),
            NodeEventType::CustomBytes(f0) => f.debug_tuple("CustomBytes").field(&f0).finish(),
            #[cfg(feature = "midi_events")]
            NodeEventType::MIDI(f0) => f.debug_tuple("MIDI").field(&f0).finish(),
        }
    }
}

/// Data that can be used to patch an individual parameter.
#[derive(Clone, Debug)]
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

    #[cfg(feature = "scheduled_events")]
    EventInstant(EventInstant),
    InstantSeconds(InstantSeconds),
    DurationSeconds(DurationSeconds),
    InstantSamples(InstantSamples),
    DurationSamples(DurationSamples),
    #[cfg(feature = "musical_transport")]
    InstantMusical(InstantMusical),
    #[cfg(feature = "musical_transport")]
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

    /// Construct an optional [`ParamData::Any`] variant.
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
                Self::$variant(value.into())
            }
        }

        impl TryInto<$ty> for &ParamData {
            type Error = crate::diff::PatchError;

            fn try_into(self) -> Result<$ty, crate::diff::PatchError> {
                match self {
                    ParamData::$variant(value) => Ok((*value).into()),
                    _ => Err(crate::diff::PatchError::InvalidData),
                }
            }
        }

        impl From<Option<$ty>> for ParamData {
            fn from(value: Option<$ty>) -> Self {
                if let Some(value) = value {
                    Self::$variant(value.into())
                } else {
                    Self::None
                }
            }
        }

        impl TryInto<Option<$ty>> for &ParamData {
            type Error = crate::diff::PatchError;

            fn try_into(self) -> Result<Option<$ty>, crate::diff::PatchError> {
                match self {
                    ParamData::$variant(value) => Ok(Some((*value).into())),
                    ParamData::None => Ok(None),
                    _ => Err(crate::diff::PatchError::InvalidData),
                }
            }
        }

        impl From<Notify<$ty>> for ParamData {
            fn from(value: Notify<$ty>) -> Self {
                Self::$variant((*value).into())
            }
        }

        impl TryInto<Notify<$ty>> for &ParamData {
            type Error = crate::diff::PatchError;

            fn try_into(self) -> Result<Notify<$ty>, crate::diff::PatchError> {
                match self {
                    ParamData::$variant(value) => Ok(Notify::new((*value).into())),
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
#[cfg(feature = "scheduled_events")]
param_data_from!(EventInstant, EventInstant);
param_data_from!(InstantSeconds, InstantSeconds);
param_data_from!(DurationSeconds, DurationSeconds);
param_data_from!(InstantSamples, InstantSamples);
param_data_from!(DurationSamples, DurationSamples);
#[cfg(feature = "musical_transport")]
param_data_from!(InstantMusical, InstantMusical);
#[cfg(feature = "musical_transport")]
param_data_from!(DurationMusical, DurationMusical);

#[cfg(feature = "glam-29")]
param_data_from!(glam_29::Vec2, Vector2D);
#[cfg(feature = "glam-29")]
param_data_from!(glam_29::Vec3, Vector3D);

#[cfg(feature = "glam-30")]
param_data_from!(glam_30::Vec2, Vector2D);
#[cfg(feature = "glam-30")]
param_data_from!(glam_30::Vec3, Vector3D);

#[cfg(feature = "glam-31")]
param_data_from!(glam_31::Vec2, Vector2D);
#[cfg(feature = "glam-31")]
param_data_from!(glam_31::Vec3, Vector3D);

impl From<()> for ParamData {
    fn from(_value: ()) -> Self {
        Self::None
    }
}

impl TryInto<()> for &ParamData {
    type Error = crate::diff::PatchError;

    fn try_into(self) -> Result<(), crate::diff::PatchError> {
        match self {
            ParamData::None => Ok(()),
            _ => Err(crate::diff::PatchError::InvalidData),
        }
    }
}

impl From<Notify<()>> for ParamData {
    fn from(_value: Notify<()>) -> Self {
        Self::None
    }
}

impl TryInto<Notify<()>> for &ParamData {
    type Error = crate::diff::PatchError;

    fn try_into(self) -> Result<Notify<()>, crate::diff::PatchError> {
        match self {
            ParamData::None => Ok(Notify::new(())),
            _ => Err(crate::diff::PatchError::InvalidData),
        }
    }
}

/// Used internally by the Firewheel processor
#[cfg(feature = "scheduled_events")]
pub struct ScheduledEventEntry {
    pub event: NodeEvent,
    pub is_pre_process: bool,
}

/// A list of events for an [`AudioNodeProcessor`][crate::node::AudioNodeProcessor].
pub struct ProcEvents<'a> {
    immediate_event_buffer: &'a mut [Option<NodeEvent>],
    #[cfg(feature = "scheduled_events")]
    scheduled_event_arena: &'a mut [Option<ScheduledEventEntry>],
    indices: &'a mut Vec<ProcEventsIndex>,
}

impl<'a> ProcEvents<'a> {
    pub fn new(
        immediate_event_buffer: &'a mut [Option<NodeEvent>],
        #[cfg(feature = "scheduled_events")] scheduled_event_arena: &'a mut [Option<
            ScheduledEventEntry,
        >],
        indices: &'a mut Vec<ProcEventsIndex>,
    ) -> Self {
        Self {
            immediate_event_buffer,
            #[cfg(feature = "scheduled_events")]
            scheduled_event_arena,
            indices,
        }
    }

    pub fn num_events(&self) -> usize {
        self.indices.len()
    }

    /// Iterate over all events, draining the events from the list.
    pub fn drain<'b>(&'b mut self) -> impl IntoIterator<Item = NodeEventType> + use<'b> {
        self.indices.drain(..).map(|index_type| match index_type {
            ProcEventsIndex::Immediate(i) => {
                self.immediate_event_buffer[i as usize]
                    .take()
                    .unwrap()
                    .event
            }
            #[cfg(feature = "scheduled_events")]
            ProcEventsIndex::Scheduled(i) => {
                self.scheduled_event_arena[i as usize]
                    .take()
                    .unwrap()
                    .event
                    .event
            }
        })
    }

    /// Iterate over all events and their timestamps, draining the
    /// events from the list.
    ///
    /// The iterator returns `(event_type, Option<event_instant>)`
    /// where `event_type` is the event, `event_instant` is the instant the
    /// event was schedueld for. If the event was not scheduled, then
    /// the latter will be `None`.
    #[cfg(feature = "scheduled_events")]
    pub fn drain_with_timestamps<'b>(
        &'b mut self,
    ) -> impl IntoIterator<Item = (NodeEventType, Option<EventInstant>)> + use<'b> {
        self.indices.drain(..).map(|index_type| match index_type {
            ProcEventsIndex::Immediate(i) => {
                let event = self.immediate_event_buffer[i as usize].take().unwrap();

                (event.event, event.time)
            }
            ProcEventsIndex::Scheduled(i) => {
                let event = self.scheduled_event_arena[i as usize].take().unwrap();

                (event.event.event, event.event.time)
            }
        })
    }

    /// Iterate over patches for `T`, draining the events from the list.
    ///
    /// ```
    /// # use firewheel_core::{diff::*, event::ProcEvents};
    /// # fn for_each_example(mut event_list: ProcEvents) {
    /// #[derive(Patch, Default)]
    /// struct FilterNode {
    ///     frequency: f32,
    ///     quality: f32,
    /// }
    ///
    /// let mut node = FilterNode::default();
    ///
    /// // You can match on individual patch variants.
    /// for patch in event_list.drain_patches::<FilterNode>() {
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
    /// for patch in event_list.drain_patches::<FilterNode>() { node.apply(patch); }
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
        self.drain().into_iter().filter_map(|e| T::patch_event(&e))
    }

    /// Iterate over patches for `T`, draining the events from the list, while also
    /// returning the timestamp the event was scheduled for.
    ///
    /// The iterator returns `(patch, Option<event_instant>)`
    /// where `event_instant` is the instant the event was schedueld for. If the event
    /// was not scheduled, then the latter will be `None`.
    ///
    /// ```
    /// # use firewheel_core::{diff::*, event::ProcEvents};
    /// # fn for_each_example(mut event_list: ProcEvents) {
    /// #[derive(Patch, Default)]
    /// struct FilterNode {
    ///     frequency: f32,
    ///     quality: f32,
    /// }
    ///
    /// let mut node = FilterNode::default();
    ///
    /// // You can match on individual patch variants.
    /// for (patch, timestamp) in event_list.drain_patches_with_timestamps::<FilterNode>() {
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
    /// for (patch, timestamp) in event_list.drain_patches_with_timestamps::<FilterNode>() { node.apply(patch); }
    /// # }
    /// ```
    ///
    /// Errors produced while constructing patches are simply skipped.
    #[cfg(feature = "scheduled_events")]
    pub fn drain_patches_with_timestamps<'b, T: crate::diff::Patch>(
        &'b mut self,
    ) -> impl IntoIterator<Item = (<T as crate::diff::Patch>::Patch, Option<EventInstant>)> + use<'b, T>
    {
        // Ideally this would parameterise the `FnMut` over some `impl From<PatchEvent<T>>`
        // but it would require a marker trait for the `diff::Patch::Patch` assoc type to
        // prevent overlapping impls.
        self.drain_with_timestamps()
            .into_iter()
            .filter_map(|(e, timestamp)| T::patch_event(&e).map(|patch| (patch, timestamp)))
    }
}

/// Used internally by the Firewheel processor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcEventsIndex {
    Immediate(u32),
    #[cfg(feature = "scheduled_events")]
    Scheduled(u32),
}
