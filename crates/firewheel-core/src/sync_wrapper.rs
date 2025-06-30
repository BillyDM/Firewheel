/// A wrapper around a non-`Sync` type to make it `Sync`.
///
/// The mechanism is very simple. [`SyncWrapper`] prevents
/// shared borrowing of the inner type. The only way to get
/// to the inner data is through `take` or `get_mut`. Both methods
/// can only be called with a mutable reference to [`SyncWrapper`],
/// so it's not possible that the inner value can be observed
/// simultaneously in multiple threads.
#[derive(Debug)]
pub struct SyncWrapper<T>(Option<T>);

impl<T> SyncWrapper<T> {
    /// Construct a new [`SyncWrapper`].
    pub fn new(value: T) -> Self {
        Self(Some(value))
    }

    /// Move out the inner value.
    ///
    /// If this has already been called on this thread or elsewhere,
    /// this will return `None`.
    pub fn take(&mut self) -> Option<T> {
        self.0.take()
    }

    /// Obtain a mutable reference to the inner value.
    ///
    /// If `take` has been called previously, this returns `None`.
    pub fn get_mut(&mut self) -> Option<&mut T> {
        self.0.as_mut()
    }
}

/// # Safety
///
/// [`SyncWrapper`] prevents
/// shared borrowing of the inner type. The only way to get
/// to the inner data is through `take` or `get_mut`. Both methods
/// can only be called with a mutable reference to [`SyncWrapper`],
/// so it's not possible that the inner value can be observed
/// simultaneously in multiple threads.
///
/// Therefore, this implementation is safe.
///
/// For further reference, see the [standard library's
/// implementation of `Sync` on `core::sync::Mutex`](https://doc.rust-lang.org/src/std/sync/poison/mutex.rs.html#189),
/// as well as the [`get_mut` method](https://doc.rust-lang.org/src/std/sync/poison/mutex.rs.html#557-581).
unsafe impl<T: Send> Sync for SyncWrapper<T> {}
