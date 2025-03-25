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
//! baseline.patch_event(&event_queue[0]);
//! assert_eq!(params, baseline);
//!
//! ```
//!
//! Both traits can also be derived on enums.
//!
//! ```
//! # use firewheel_core::diff::{Diff, Patch, PathBuilder};
//! #[derive(Diff, Patch)]
//! enum MyParams {
//!     Unit,
//!     Tuple(f32, f32),
//!     Struct { a: f32, b: f32 },
//! }
//! ```
//!
//! Changing between tuple or struct fields will incur allocations
//! in [`Diff`], but changes _within_ a single variant are still fine-grained.
//!
//! It's important to note that you can accidentally introduce allocations
//! in audio processors by including types that allocate on clone.
//!
//! ```
//! # use firewheel_core::diff::{Diff, Patch, PathBuilder};
//! #[derive(Diff, Patch)]
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
//! #[derive(Diff, Patch)]
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
//! diffing or patching, which is particularly useful for atomically synchronized
//! types.
//! ```
//! use firewheel_core::{collector::ArcGc, diff::{Diff, Patch}};
//! use core::sync::atomic::AtomicUsize;
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

use crate::{
    collector::ArcGc,
    event::{NodeEventList, NodeEventType, ParamData},
};

use smallvec::SmallVec;

mod collections;
mod leaf;
mod memo;

pub use memo::Memo;

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

/// Fine-grained parameter patching.
///
/// This trait allows a type to perform patching on itself,
/// applying patches generated from another instance.
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
///         inputs: &[&[f32]],
///         outputs: &mut [&mut [f32]],
///         events: NodeEventList,
///         proc_info: &ProcInfo,
///         scratch_buffers: ScratchBuffers,
///     ) -> ProcessStatus {
///         // Synchronize `params` from the event list.
///         self.params.patch_list(events);
///
///         // ...
///
///         ProcessStatus::outputs_not_silent()
///     }
/// }
/// ```
///
/// [`Patch::patch_list`] is a convenience trait method
/// that takes a [`NodeEventList`] by value, applies any
/// parameter patches that may be present, and returns
/// a boolean indicating whether any parameters have changed.
///
/// If you need finer access to the event list, you can
/// apply patches more directly.
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
///         inputs: &[&[f32]],
///         outputs: &mut [&mut [f32]],
///         mut events: NodeEventList,
///         proc_info: &ProcInfo,
///         scratch_buffers: ScratchBuffers,
///     ) -> ProcessStatus {
///         events.for_each(|e| {
///             // You can take the whole event, which may
///             // or may not actually contain a parameter:
///             self.params.patch_event(e);
///
///             // Or match on the event and provide
///             // each element directly:
///             match e {
///                 NodeEventType::Param { data, path } => {
///                     // This allows you to handle errors as well.
///                     let _ = self.params.patch(data, &path);
///                 }
///                 _ => {}
///             }
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
/// Nonetheless, Firewheel provides a few tools to make manual implementations easy.
///
/// ```
/// use firewheel_core::{diff::{Patch, PatchError}, event::ParamData};
///
/// struct MyParams {
///     a: f32,
///     b: (bool, bool)
/// }
///
/// impl Patch for MyParams {
///     fn patch(&mut self, data: &ParamData, path: &[u32]) -> Result<(), PatchError> {
///         match path {
///             [0] => {
///                 // You can defer to `f32`'s `Patch` implementation, or simply
///                 // apply the data directly like we do here.
///                 self.a = data.try_into()?;
///
///                 Ok(())
///             }
///             // Shortening the path slice one element at a time as we descend the tree
///             // allows nested types to see the path as they expect it.
///             [1, tail @ ..] => {
///                 self.b.patch(data, tail)
///             }
///             _ => Err(PatchError::InvalidPath)
///         }
///     }
/// }
/// ```
pub trait Patch {
    /// Patch `self` according to the incoming data.
    /// This will generally be called from within
    /// the audio thread.
    ///
    /// `data` is intentionally made a shared reference.
    /// This should make accidental syscalls due to
    /// additional allocations or drops more difficult.
    /// If you find yourself reaching for interior
    /// mutability, consider whether you're building
    /// realtime-appropriate behavior.
    fn patch(&mut self, data: &ParamData, path: &[u32]) -> Result<(), PatchError>;

    /// Patch a set of parameters with incoming events.
    ///
    /// Returns `true` if any parameters have changed.
    ///
    /// This is useful as a convenience method for extracting the path
    /// and data components from a [`NodeEventType`]. Errors produced
    /// here are ignored.
    fn patch_event(&mut self, event: &NodeEventType) -> bool {
        if let NodeEventType::Param { data, path } = event {
            // NOTE: It may not be ideal to ignore errors.
            // Would it be possible to log these in debug mode?
            self.patch(data, path).is_ok()
        } else {
            false
        }
    }

    /// Patch a set of parameters with a list of incoming events.
    ///
    /// Returns `true` if any parameters have changed.
    ///
    /// This is useful as a convenience method for patching parameters
    /// directly from a [`NodeEventList`]. Errors produced here are ignored.
    fn patch_list(&mut self, mut event_list: NodeEventList) -> bool {
        let mut changed = false;

        event_list.for_each(|e| {
            changed |= self.patch_event(e);
        });

        changed
    }
}

/// A path of indices that uniquely describes an arbitrarily nested field.
#[derive(PartialEq, Eq)]
pub enum ParamPath {
    Single(u32),
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

// NOTE: Using a `SmallVec` instead of a `Box<[u32]>` yields
// around an 8% performance uplift for cases where the path
// is in the range 2..=4.
//
// Beyond this range, the performance drops off around 13%.
//
// Since this avoids extra allocations in the common < 5
// scenario, this seems like a reasonable tradeoff.

/// A simple builder for [`ParamPath`].
#[derive(Debug, Default, Clone)]
pub struct PathBuilder(SmallVec<[u32; 4]>);

impl PathBuilder {
    /// Clone the path and append the index.
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
        assert!(baseline.patch_event(&messages[0]));
        assert_eq!(baseline, value);
    }

    #[test]
    fn test_enum_switch_variant() {
        let mut baseline = DiffingExample::Unit;
        let value = DiffingExample::Struct { a: 1.0, b: 1.0 };

        let mut messages = Vec::new();
        value.diff(&baseline, PathBuilder::default(), &mut messages);

        assert_eq!(messages.len(), 1);
        assert!(baseline.patch_event(&messages[0]));
        assert_eq!(baseline, value);
    }
}
