use crate::{
    diff::{Diff, Patch, RealtimeClone},
    event::ParamData,
};
use bevy_platform::sync::atomic::{AtomicU64, Ordering};

// Increment an atomic counter.
//
// This is guaranteed to never return zero.
#[inline(always)]
fn increment_counter() -> u64 {
    static NOTIFY_COUNTER: AtomicU64 = AtomicU64::new(1);

    NOTIFY_COUNTER
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current_val| {
            current_val
                // Attempt increment
                .checked_add(1)
                // If it overflows, return 1 instead
                .or(Some(1))
        })
        // We always return `Some`
        .unwrap()
}

/// A lightweight wrapper that guarantees an event
/// will be generated every time the inner value is accessed mutably,
/// even if the value doesn't change.
///
/// This is useful for types like a play head
/// where periodically writing the same value
/// carries useful information.
///
/// [`Notify`] implements [`core::ops::Deref`] and [`core::ops::DerefMut`]
/// for the inner `T`.
#[derive(Debug, Clone)]
pub struct Notify<T> {
    value: T,
    counter: u64,
}

impl<T> Notify<T> {
    /// Construct a new [`Notify`].
    ///
    /// If two instances of [`Notify`] are constructed separately,
    /// a call to [`Diff::diff`] will produce an event, even if the
    /// value is the same.
    ///
    /// ```
    /// # use firewheel_core::diff::Notify;
    /// // Diffing `a` and `b` will produce an event
    /// let a = Notify::new(1);
    /// let b = Notify::new(1);
    ///
    /// // whereas `b` and `c` will not.
    /// let c = b.clone();
    /// ```
    pub fn new(value: T) -> Self {
        Self {
            value,
            counter: increment_counter(),
        }
    }

    /// Get this instance's unique ID.
    ///
    /// After each mutable dereference, this ID will be replaced
    /// with a new, unique value. For all practical purposes,
    /// the ID can be considered unique among all [`Notify`] instances.
    ///
    /// [`Notify`] IDs are guaranteed to never be 0, so it can be
    /// used as a sentinel value.
    #[inline(always)]
    pub fn id(&self) -> u64 {
        self.counter
    }

    /// Get mutable access to the inner value without updating the ID.
    pub fn as_mut_unsync(&mut self) -> &mut T {
        &mut self.value
    }

    /// Manually update the internal ID without modifying the internals.
    pub fn notify(&mut self) {
        self.counter = increment_counter();
    }
}

impl<T> Copy for Notify<T> where T: Copy {}

impl<T> AsRef<T> for Notify<T> {
    fn as_ref(&self) -> &T {
        &self.value
    }
}

impl<T> AsMut<T> for Notify<T> {
    fn as_mut(&mut self) -> &mut T {
        self.counter = increment_counter();

        &mut self.value
    }
}

impl<T: Default> Default for Notify<T> {
    fn default() -> Self {
        Self::new(T::default())
    }
}

impl<T> core::ops::Deref for Notify<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.value
    }
}

impl<T> core::ops::DerefMut for Notify<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.counter = increment_counter();

        &mut self.value
    }
}

// TODO: Once negative traits are stabilized, add extra implementations that don't allocate
// for types that implement `Into<T>` + `From<T>` where T is a primitive type.
impl<T: RealtimeClone + Send + Sync + 'static> Diff for Notify<T> {
    fn diff<E: super::EventQueue>(
        &self,
        baseline: &Self,
        path: super::PathBuilder,
        event_queue: &mut E,
    ) {
        if self.counter != baseline.counter {
            event_queue.push_param(ParamData::any(self.clone()), path);
        }
    }
}

impl<T: RealtimeClone + Send + Sync + 'static> Patch for Notify<T> {
    type Patch = Self;

    fn patch(data: ParamData, _: &[u32]) -> Result<Self::Patch, super::PatchError> {
        data.downcast_ref()
            .ok_or(super::PatchError::InvalidData)
            .cloned()
    }

    fn apply(&mut self, patch: Self::Patch) {
        *self = patch;
    }
}

impl<T: PartialEq> PartialEq for Notify<T> {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value && self.counter == other.counter
    }
}

#[cfg(test)]
mod test {
    use crate::diff::PathBuilder;

    use super::*;

    #[test]
    fn test_identical_write() {
        let baseline = Notify::new(0.5f32);
        let mut value = baseline.clone();

        let mut events = Vec::new();
        value.diff(&baseline, PathBuilder::default(), &mut events);
        assert_eq!(events.len(), 0);

        *value = 0.5f32;

        value.diff(&baseline, PathBuilder::default(), &mut events);
        assert_eq!(events.len(), 1);
    }
}
