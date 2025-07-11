//! Traits and derive macros for diffing and patching.
//!
//! _Diffing_ is the process of comparing a piece of data to some
//! baseline and generating events to describe the differences.
//! _Patching_ takes these events and applies them to another
//! instance of this data. The [`Diff`] and [`Patch`] traits facilitate _fine-grained_
//! event generation, meaning they'll generate events for
//! only what's changed.
//!
//! In typical usage, [`Diff`] will be called in non-realtime contexts
//! like game logic, whereas [`Patch`] will be called directly within
//! audio processors. Consequently, [`Patch`] has been optimized for
//! maximum performance and realtime predictability.
//!
//! [`Diff`] and [`Patch`] are [derivable](https://doc.rust-lang.org/book/appendix-03-derivable-traits.html),
//! and most aggregate types should prefer the derive macros over
//! manual implementations since the diffing data model is not
//! yet guaranteed to be stable.
//!
//! # Examples
//!
//! Aggregate types like node parameters can derive
//! [`Diff`] and [`Patch`] as long as each field also
//! implements these traits.
//!
//! ```
//! use firewheel_core::diff::{Diff, Patch};
//!
//! #[derive(Diff, Patch)]
//! struct MyParams {
//!     a: f32,
//!     b: (bool, bool),
//! }
//! ```
//!
//! The derived implementation produces fine-grained
//! events, making it easy to keep your audio processors in sync
//! with the rest of your code with minimal overhead.
//!
//! ```
//! # use firewheel_core::diff::{Diff, Patch, PathBuilder};
//! # #[derive(Diff, Patch, Clone, PartialEq, Debug)]
//! # struct MyParams {
//! #     a: f32,
//! #     b: (bool, bool),
//! # }
//! let mut params = MyParams {
//!     a: 1.0,
//!     b: (false, false),
//! };
//! let mut baseline = params.clone();
//!
//! // A change to any arbitrarily nested parameter
//! // will produce a single event.
//! params.b.0 = true;
//!
//! let mut event_queue = Vec::new();
//! params.diff(&baseline, PathBuilder::default(), &mut event_queue);
//!
//! // When we apply this patch to another instance of
//! // the same type, it will be brought in sync.
//! baseline.apply(MyParams::patch_event(&event_queue[0]).unwrap());
//! assert_eq!(params, baseline);
//!
//! ```
//!
//! Both traits can also be derived on enums.
//!
//! ```
//! # use firewheel_core::diff::{Diff, Patch, PathBuilder};
//! #[derive(Diff, Patch, Clone, PartialEq)]
//! enum MyParams {
//!     Unit,
//!     Tuple(f32, f32),
//!     Struct { a: f32, b: f32 },
//! }
//! ```
//!
//! However, note that enums will only perform coarse diffing. If a single
//! field in a variant changes, the entire variant will still be sent.
//! As a result, you can accidentally introduce allocations
//! in audio processors by including types that allocate on clone.
//!
//! ```
//! # use firewheel_core::diff::{Diff, Patch, PathBuilder};
//! #[derive(Diff, Patch, Clone, PartialEq)]
//! enum MaybeAllocates {
//!     A(Vec<f32>), // Will cause allocations in `Patch`!
//!     B(f32),
//! }
//! ```
//!
//! [`Clone`] types are permitted because [`Clone`] does
//! not always imply allocation. For example, consider
//! the type:
//!
//! ```
//! use firewheel_core::{collector::ArcGc, sample_resource::SampleResource};
//!
//! # use firewheel_core::diff::{Diff, Patch, PathBuilder};
//! #[derive(Diff, Patch, Clone, PartialEq)]
//! enum SoundSource {
//!     Sample(ArcGc<dyn SampleResource>), // Will _not_ cause allocations in `Patch`.
//!     Frequency(f32),
//! }
//! ```
//!
//! This bound may be restricted to [`Copy`] in the future.
//!
//! # Macro attributes
//!
//! [`Diff`] and [`Patch`] each accept a single attribute, `skip`, on
//! struct fields. Any field annotated with `skip` will not receive
//! diffing or patching, which may be useful for atomically synchronized
//! types.
//! ```
//! use firewheel_core::{collector::ArcGc, diff::{Diff, Patch}};
//! use bevy_platform::sync::atomic::AtomicUsize;
//!
//! #[derive(Diff, Patch)]
//! struct MultiParadigm {
//!     normal_field: f32,
//!     #[diff(skip)]
//!     atomic_field: ArcGc<AtomicUsize>,
//! }
//! ```
//!
//! # Data model
//!
//! Diffing events are represented as `(data, path)` pairs. This approach
//! provides a few important advantages. For one, the fields within nearly
//! all Rust types can be uniquely addressed with index paths.
//!
//! ```
//! # use firewheel_core::diff::{Diff, Patch};
//! #[derive(Diff, Patch, Default)]
//! struct MyParams {
//!     a: f32,
//!     b: (bool, bool),
//! }
//!
//! let params = MyParams::default();
//!
//! params.a;   // [0]
//! params.b.0; // [1, 0]
//! params.b.1; // [1, 1]
//! ```
//!
//! Since these paths can be arbitrarily long, you can arbitrarily
//! nest implementors of [`Diff`] and [`Patch`].
//!
//! ```
//! # use firewheel_core::diff::{Diff, Patch};
//! # #[derive(Diff, Patch, Default)]
//! # struct MyParams {
//! #     a: f32,
//! #     b: (bool, bool),
//! # }
//! #[derive(Diff, Patch)]
//! struct Aggregate {
//!     a: MyParams,
//!     b: MyParams,
//!     // Indexable types work great too!
//!     collection: [MyParams; 8],
//! }
//! ```
//!
//! Furthermore, since we build up paths during calls to
//! [`Diff`], the derive macros and implementations only need
//! to worry about _local indexing._ And, since the paths
//! are built only during [`Diff`], we can traverse them
//! highly performantly during [`Patch`] calls in audio processors.
//!
//! Firewheel provides a number of primitive types in [`ParamData`]
//! that cover most use-cases for audio parameters. For anything
//! not covered in the concrete variants, you can insert arbitrary
//! data into [`ParamData::Any`]. Since this only incurs allocations
//! during [`Diff`], this will still be generally performant.
//!
//! # Preserving invariants
//!
//! Firewheel's [`Patch`] derive macro cannot make assurances about
//! your type's invariants. If two types `A` and `B` have similar structures:
//!
//! ```
//! struct A {
//!     pub field_one: f32,
//!     pub field_two: f32,
//! }
//!
//! struct B {
//!     special_field_one: f32,
//!     special_field_two: f32,
//! }
//! ```
//!
//! Then events produced for `A` are also valid for `B`.
//!
//! Receiving events produced by the wrong type is unlikely. Most
//! types will not need special handling to preserve invariants.
//! However, if your invariants are safety-critical, you _must_
//! implement [`Patch`] manually.

use crate::{
    collector::ArcGc,
    event::{NodeEventType, ParamData},
};

use smallvec::SmallVec;

mod collections;
mod leaf;
mod memo;
mod notify;

pub use memo::Memo;
pub use notify::Notify;

/// Derive macros for diffing and patching.
pub use firewheel_macros::{Diff, Patch};

/// Fine-grained parameter diffing.
///
/// This trait allows a type to perform diffing on itself,
/// generating events that another instance can use to patch
/// itself.
///
/// For more information, see the [module docs][self].
///
/// # Examples
///
/// For most use cases, [`Diff`] is fairly straightforward.
///
/// ```
/// use firewheel_core::diff::{Diff, PathBuilder};
///
/// #[derive(Diff, Clone)]
/// struct MyParams {
///     a: f32,
///     b: f32,
/// }
///
/// let mut params = MyParams {
///     a: 1.0,
///     b: 1.0,
/// };
///
/// // This "baseline" instance allows us to keep track
/// // of what's changed over time.
/// let baseline = params.clone();
///
/// // A single mutation to a "leaf" type like `f32` will
/// // produce a single event.
/// params.a = 0.5;
///
/// // `Vec<NodeEventType>` implements `EventQueue`, meaning we
/// // don't necessarily need to keep track of `NodeID`s for event generation.
/// let mut event_queue = Vec::new();
/// // Top-level calls to diff should always provide a default path builder.
/// params.diff(&baseline, PathBuilder::default(), &mut event_queue);
///
/// assert_eq!(event_queue.len(), 1);
/// ```
///
/// When using Firewheel in a standalone context, the [`Memo`] type can
/// simplify this process.
///
/// ```
/// # use firewheel_core::diff::{Diff, PathBuilder};
/// # #[derive(Diff, Clone)]
/// # struct MyParams {
/// #     a: f32,
/// #     b: f32,
/// # }
/// use firewheel_core::diff::Memo;
///
/// let mut params_memo = Memo::new(MyParams {
///     a: 1.0,
///     b: 1.0,
/// });
///
/// // `Memo` implements `DerefMut` on the wrapped type, allowing you
/// // to use it almost transparently.
/// params_memo.a = 0.5;
///
/// let mut event_queue = Vec::new();
/// // This generates patches and brings the internally managed
/// // baseline in sync.
/// params_memo.update_memo(&mut event_queue);
/// ```
///
/// # Manual implementation
///
/// Aggregate types like parameters should prefer the derive macro, but
/// manual implementations can occasionally be handy. You should strive
/// to match the derived data model for maximum compatibility.
///
/// ```
/// use firewheel_core::diff::{Diff, PathBuilder, EventQueue};
/// # struct MyParams {
/// #     a: f32,
/// #     b: f32,
/// # }
///
/// impl Diff for MyParams {
///     fn diff<E: EventQueue>(&self, baseline: &Self, path: PathBuilder, event_queue: &mut E) {
///         // The diffing data model requires a unique path to each field.
///         // Because this type can be arbitrarily nested, you should always
///         // extend the provided path builder using `PathBuilder::with`.
///         //
///         // Because this is the first field, we'll extend the path with 0.
///         self.a.diff(&baseline.a, path.with(0), event_queue);
///         self.b.diff(&baseline.b, path.with(1), event_queue);
///     }
/// }
/// ```
///
/// You can easily override a type's [`Diff`] implementation by simply
/// doing comparisons by hand.
///
/// ```
/// use firewheel_core::event::ParamData;
/// # use firewheel_core::diff::{Diff, PathBuilder, EventQueue};
/// # struct MyParams {
/// #     a: f32,
/// #     b: f32,
/// # }
///
/// impl Diff for MyParams {
///     fn diff<E: EventQueue>(&self, baseline: &Self, path: PathBuilder, event_queue: &mut E) {
///         // The above is essentially equivalent to:
///         if self.a != baseline.a {
///             event_queue.push_param(ParamData::F32(self.a), path.with(0));
///         }
///
///         if self.b != baseline.b {
///             event_queue.push_param(ParamData::F32(self.b), path.with(1));
///         }
///     }
/// }
/// ```
///
/// If your type has invariants between fields that _must not_ be violated, you
/// can consider the whole type a "leaf," similar to how [`Diff`] is implemented
/// on primitives. Depending on the type's data, you may require an allocation.
///
/// ```
/// # use firewheel_core::{diff::{Diff, PathBuilder, EventQueue}, event::ParamData};
/// # #[derive(PartialEq, Clone)]
/// # struct MyParams {
/// #     a: f32,
/// #     b: f32,
/// # }
/// impl Diff for MyParams {
///     fn diff<E: EventQueue>(&self, baseline: &Self, path: PathBuilder, event_queue: &mut E) {
///         if self != baseline {
///             // Note that if we consider the whole type to be a leaf, there
///             // is no need to extend the path.
///             event_queue.push_param(ParamData::any(self.clone()), path);
///         }
///     }
/// }
/// ```
pub trait Diff {
    /// Compare `self` to `baseline` and generate events to resolve any differences.
    fn diff<E: EventQueue>(&self, baseline: &Self, path: PathBuilder, event_queue: &mut E);
}

/// A path of indices that uniquely describes an arbitrarily nested field.
#[derive(PartialEq, Eq)]
pub enum ParamPath {
    /// A path of one element.
    ///
    /// Parameters tend to be shallow structures, so allocations
    /// can generally be avoided using this variant.
    Single(u32),
    /// When paths are more than one element, this variant keeps
    /// the stack size to two pointers while avoiding double-indirection
    /// in the range 2..=4.
    Multi(ArcGc<SmallVec<[u32; 4]>>),
}

impl core::ops::Deref for ParamPath {
    type Target = [u32];

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Single(single) => core::slice::from_ref(single),
            Self::Multi(multi) => multi.as_ref(),
        }
    }
}

/// Fine-grained parameter patching.
///
/// This trait allows a type to perform patching on itself,
/// applying changes generated from another instance.
///
/// For more information, see the [module docs][self].
///
/// # Examples
///
/// Like with [`Diff`], the typical [`Patch`] usage is simple.
///
/// ```
/// use firewheel_core::{diff::Patch, event::*, node::*};
///
/// #[derive(Patch)]
/// struct MyParams {
///     a: f32,
///     b: f32,
/// }
///
/// struct MyProcessor {
///     params: MyParams,
/// }
///
/// impl AudioNodeProcessor for MyProcessor {
///     fn process(
///         &mut self,
///         buffers: ProcBuffers,
///         proc_info: &ProcInfo,
///         mut events: NodeEventList,
///     ) -> ProcessStatus {
///         // Synchronize `params` from the event list.
///         events.for_each_patch::<MyParams>(|patch| self.params.apply(patch.event));
///
///         // ...
///
///         ProcessStatus::outputs_not_silent()
///     }
/// }
/// ```
///
/// If you need fine access to each patch, you can
/// match on the patch type.
///
/// ```
/// # use firewheel_core::{diff::{Patch}, event::*, node::*};
/// # #[derive(Patch)]
/// # struct MyParams {
/// #     a: f32,
/// #     b: f32,
/// # }
/// # struct MyProcessor {
/// #    params: MyParams,
/// # }
/// impl AudioNodeProcessor for MyProcessor {
///     fn process(
///         &mut self,
///         buffers: ProcBuffers,
///         proc_info: &ProcInfo,
///         mut events: NodeEventList,
///     ) -> ProcessStatus {
///         events.for_each_patch::<MyParams>(|mut patch| {
///             // When you derive `Patch`, it creates an enum with variants
///             // for each field.
///             match &mut patch.event {
///                 MyParamsPatch::A(a) => {
///                     // You can mutate the patch itself if you want
///                     // to constrain or modify values.
///                     *a = a.clamp(0.0, 1.0);
///                 }
///                 MyParamsPatch::B(b) => {}
///             }
///
///             // And / or apply it directly.
///             self.params.apply(patch.event);
///         });
///
///         // ...
///
///         ProcessStatus::outputs_not_silent()
///     }
/// }
/// ```
///
/// # Manual implementation
///
/// Like with [`Diff`], types like parameters should prefer the [`Patch`] derive macro.
/// Nonetheless, Firewheel provides a few tools to make manual implementations straightforward.
///
/// ```
/// use firewheel_core::{diff::{Patch, PatchError}, event::ParamData};
///
/// struct MyParams {
///     a: f32,
///     b: bool,
/// }
///
/// // To follow the derive macro convention, create an
/// // enum with variants for each field.
/// enum MyParamsPatch {
///     A(f32),
///     B(bool),
/// }
///
/// impl Patch for MyParams {
///     type Patch = MyParamsPatch;
///
///     fn patch(data: &ParamData, path: &[u32]) -> Result<Self::Patch, PatchError> {
///         match path {
///             [0] => {
///                 // Types that exist in `ParamData`'s variants can use
///                 // `try_into`.
///                 let a = data.try_into()?;
///                 Ok(MyParamsPatch::A(a))
///             }
///             [1] => {
///                 let b = data.try_into()?;
///                 Ok(MyParamsPatch::B(b))
///             }
///             _ => Err(PatchError::InvalidPath)
///         }
///     }
///
///     fn apply(&mut self, patch: Self::Patch) {
///         match patch {
///             MyParamsPatch::A(a) => self.a = a,
///             MyParamsPatch::B(b) => self.b = b,
///         }
///     }
/// }
/// ```
pub trait Patch {
    /// A type's _patch_.
    ///
    /// This is a value that enumerates all the ways a type can be changed.
    /// For leaf types (values that represent the smallest diffable unit) like `f32`,
    /// this is just the type itself. For aggregate types like structs, this should
    /// be an enum over each field.
    ///
    /// ```
    /// struct FilterParams {
    ///     frequency: f32,
    ///     quality: f32,
    /// }
    ///
    /// enum FilterParamsPatch {
    ///     Frequency(f32),
    ///     Quality(f32),
    /// }
    /// ```
    ///
    /// This type is converted from [`NodeEventType::Param`] in the [`patch`][Patch::patch]
    /// method.
    type Patch;

    /// Construct a patch from a parameter event.
    ///
    /// This converts the intermediate representation in [`NodeEventType::Param`] into
    /// a concrete value, making it easy to manipulate the event in audio processors.
    ///
    /// ```
    /// # use firewheel_core::{diff::Patch, event::{NodeEventList, NodeEventType}};
    /// # fn patching(mut event_list: NodeEventList) {
    /// #[derive(Patch, Default)]
    /// struct FilterParams {
    ///     frequency: f32,
    ///     quality: f32,
    /// }
    ///
    /// let mut filter_params = FilterParams::default();
    ///
    /// event_list.for_each(|e| {
    ///     match e.event {
    ///         NodeEventType::Param { data, path } => {
    ///             let Ok(patch) = FilterParams::patch(data, path) else {
    ///                 return;
    ///             };
    ///
    ///             // You can match on the patch directly
    ///             match &patch {
    ///                 FilterParamsPatch::Frequency(f) => {
    ///                     // Handle frequency event...
    ///                 }
    ///                 FilterParamsPatch::Quality(q) => {
    ///                     // Handle quality event...
    ///                 }
    ///             }
    ///
    ///             // And/or apply it.
    ///             filter_params.apply(patch);
    ///         }
    ///         _ => {}
    ///     }
    /// });
    /// # }
    /// ```
    fn patch(data: &ParamData, path: &[u32]) -> Result<Self::Patch, PatchError>;

    /// Construct a patch from a node event.
    ///
    /// This is a convenience wrapper around [`patch`][Patch::patch], discarding
    /// errors and node events besides [`NodeEventType::Param`].
    fn patch_event(event: &NodeEventType) -> Option<Self::Patch> {
        match event {
            NodeEventType::Param { data, path } => Some(Self::patch(data, path).ok()?),
            _ => None,
        }
    }

    /// Apply a patch.
    ///
    /// This will generally be called from within
    /// the audio thread, so real-time constraints should be respected.
    ///
    /// Typically, you'll call this within [`for_each_patch`].
    ///
    /// ```
    /// # use firewheel_core::{diff::Patch, event::{NodeEventList, NodeEventType}};
    /// # fn patching(mut event_list: NodeEventList) {
    /// #[derive(Patch, Default)]
    /// struct FilterParams {
    ///     frequency: f32,
    ///     quality: f32,
    /// }
    ///
    /// let mut filter_params = FilterParams::default();
    /// event_list.for_each_patch::<FilterParams>(|patch| filter_params.apply(patch.event));
    /// # }
    /// ```
    ///
    /// [`for_each_patch`]: crate::event::NodeEventList::for_each_patch
    fn apply(&mut self, patch: Self::Patch);
}

// NOTE: Using a `SmallVec` instead of a `Box<[u32]>` yields
// around an 8% performance uplift for cases where the path
// is in the range 2..=4.
//
// Beyond this range, the performance drops off around 13%.
//
// Since this avoids extra allocations in the common < 5
// scenario, this seems like a reasonable tradeoff.

/// A simple builder for [`ParamPath`].
///
/// When performing top-level diffing, you should provide a default
/// [`PathBuilder`].
///
/// ```
/// # use firewheel_core::{diff::{Diff, PathBuilder}, event::*, node::*};
/// #[derive(Diff, Default, Clone)]
/// struct FilterNode {
///     frequency: f32,
///     quality: f32,
/// }
///
/// let baseline = FilterNode::default();
/// let node = baseline.clone();
///
/// let mut events = Vec::new();
/// node.diff(&baseline, PathBuilder::default(), &mut events);
/// ```
#[derive(Debug, Default, Clone)]
pub struct PathBuilder(SmallVec<[u32; 4]>);

impl PathBuilder {
    /// Clone the path, appending the index to the returned value.
    pub fn with(&self, index: u32) -> Self {
        let mut new = self.0.clone();
        new.push(index);
        Self(new)
    }

    /// Convert this path builder into a [`ParamPath`].
    pub fn build(self) -> ParamPath {
        if self.0.len() == 1 {
            ParamPath::Single(self.0[0])
        } else {
            ParamPath::Multi(ArcGc::new(self.0.clone()))
        }
    }
}

/// An event queue for diffing.
pub trait EventQueue {
    /// Push an event to the queue.
    fn push(&mut self, data: NodeEventType);

    /// Push an event to the queue.
    ///
    /// This is a convenience method for constructing a [`NodeEventType`]
    /// from param data and a path.
    #[inline(always)]
    fn push_param(&mut self, data: impl Into<ParamData>, path: PathBuilder) {
        self.push(NodeEventType::Param {
            data: data.into(),
            path: path.build(),
        });
    }
}

impl EventQueue for Vec<NodeEventType> {
    fn push(&mut self, data: NodeEventType) {
        self.push(data);
    }
}

/// An error encountered when patching a type
/// from [`ParamData`].
#[derive(Debug, Clone)]
pub enum PatchError {
    /// The provided path does not match any children.
    InvalidPath,
    /// The data supplied for the path did not match the expected type.
    InvalidData,
}

#[cfg(test)]
mod test {
    use super::*;

    #[derive(Debug, Clone, Diff, Patch, PartialEq)]
    struct StructDiff {
        a: f32,
        b: bool,
    }

    #[test]
    fn test_simple_diff() {
        let mut a = StructDiff { a: 1.0, b: false };

        let mut b = a.clone();

        a.a = 0.5;

        let mut patches = Vec::new();
        a.diff(&b, PathBuilder::default(), &mut patches);

        assert_eq!(patches.len(), 1);

        for patch in &patches {
            let patch = StructDiff::patch_event(patch).unwrap();

            assert!(matches!(patch, StructDiffPatch::A(a) if a == 0.5));

            b.apply(patch);
        }

        assert_eq!(a, b);
    }

    #[derive(Debug, Clone, Diff, Patch, PartialEq)]
    enum DiffingExample {
        Unit,
        Tuple(f32, f32),
        Struct { a: f32, b: f32 },
    }

    #[test]
    fn test_enum_diff() {
        let mut baseline = DiffingExample::Tuple(1.0, 0.0);
        let value = DiffingExample::Tuple(1.0, 1.0);

        let mut messages = Vec::new();
        value.diff(&baseline, PathBuilder::default(), &mut messages);

        assert_eq!(messages.len(), 1);
        baseline.apply(DiffingExample::patch_event(&messages[0]).unwrap());
        assert_eq!(baseline, value);
    }

    #[test]
    fn test_enum_switch_variant() {
        let mut baseline = DiffingExample::Unit;
        let value = DiffingExample::Struct { a: 1.0, b: 1.0 };

        let mut messages = Vec::new();
        value.diff(&baseline, PathBuilder::default(), &mut messages);

        assert_eq!(messages.len(), 1);
        baseline.apply(DiffingExample::patch_event(&messages[0]).unwrap());
        assert_eq!(baseline, value);
    }
}
