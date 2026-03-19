//! A simple garbage collector which collects resources dropped on a
//! realtime thread and safely deallocates them on another thread.
//!
//! The performance characteristics of the [`ArcGc`], [`OwnedGc`], and
//! [`OwnedGcUnsized`] smart pointers are equivalant to [`Arc`]  when
//! reading (but constructing them is a bit more expensive).
//!
//! This crate also contains optional triple buffer types for syncing
//! data (enable with the `triple_buffer` feature).
//!
//! # Example
//!
//! ```rust
//! # use firewheel_core::collector::*;
//! # use std::time::Duration;
//! let value_1 = ArcGc::new(String::from("foo"));
//! // Same as `ArcGc` but for `!Sync` data.
//! let value_2 = OwnedGc::new(String::from("bar"));
//!
//! // A simulated "realtime thread"
//! let rt_thread = std::thread::spawn(move || {
//!     std::thread::sleep(Duration::from_millis(15));
//!
//!     // Dropping the values on the realtime thread is realtime-safe
//!     // because the contents are automatically collected and
//!     // deallocated on a separate non-realtime thread.
//!     let _ = value_1;
//!     let _ = value_2;
//! });
//!
//! // A simulated update loop on the main thread
//! for _ in 0..4 {
//!     // Call `GlobalRtGc::collect()` periodically to deallocate
//!     // any resources that were dropped on the realtime thread.
//!     GlobalRtGc::collect();
//!
//!     std::thread::sleep(Duration::from_millis(15));
//! }
//! ```

use core::{
    any::Any,
    cell::UnsafeCell,
    fmt::Debug,
    ops::{Deref, DerefMut},
    pin::Pin,
    ptr::NonNull,
};

use bevy_platform::{
    prelude::{Box, Vec},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};

struct CollectorState {
    registry: Mutex<Vec<Box<dyn StrongCount + 'static>>>,
    any_dropped: AtomicBool,
}

impl CollectorState {
    const fn new() -> Self {
        Self {
            registry: Mutex::new(Vec::new()),
            any_dropped: AtomicBool::new(false),
        }
    }

    fn register<T: ?Sized + 'static>(&self, data: Arc<T>)
    where
        Arc<T>: StrongCount,
    {
        self.registry.lock().unwrap().push(Box::new(data));
    }

    /// Indicate that data has been dropped.
    fn remove<T: ?Sized>(&self, data: &Arc<T>) {
        if Arc::strong_count(data) == 2 {
            // Relaxed ordering should be sufficient since the collector can always
            // drop it on the next collect cycle.
            self.any_dropped.store(true, Ordering::Relaxed);
        }
    }

    fn collect(&self) {
        // Relaxed ordering should be sufficient since the collector can
        // always drop resources on the next collect cycle.
        if self.any_dropped.load(Ordering::Relaxed) {
            self.any_dropped.store(false, Ordering::Relaxed);

            self.registry.lock().unwrap().retain(|ptr| ptr.count() > 1);
        }
    }

    fn any_dropped(&self) -> bool {
        self.any_dropped.load(Ordering::Relaxed)
    }

    fn num_allocations(&self) -> usize {
        self.registry.lock().unwrap().len()
    }
}

/// A trait which describes a garbage collector which collects resources
/// dropped on a realtime thread and safely deallocates them on another
/// thread.
pub trait Collector: Send + Sync {
    /// Register this data with the garbage collector.
    fn register<T>(&self, data: Arc<T>)
    where
        T: ?Sized + Send + Sync + 'static,
        Arc<T>: StrongCount;

    /// Called in [`ArcGc`]'s `Drop` implementation.
    ///
    /// This can be used to indicate that garbage-collected
    /// items should be checked for pruning.
    fn remove<T>(&self, data: &Arc<T>)
    where
        T: ?Sized + Send + Sync + 'static,
        Arc<T>: StrongCount;
}

/// A simple garbage collector which collects resources dropped on a
/// realtime thread and safely deallocates them on another thread.
///
/// This is the default collector for [`ArcGc`], [`OwnedGc`], and
/// [`OwnedGcUnsized`].
///
/// This uses global statics, so registration and collection runs may block.
#[derive(Default, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GlobalRtGc;

static GLOBAL_COLLECTOR: CollectorState = CollectorState::new();

impl GlobalRtGc {
    /// Returns `true` if any allocation has been dropped since the
    /// last call to [`GlobalRtGc::collect`].
    pub fn any_dropped() -> bool {
        GLOBAL_COLLECTOR.any_dropped()
    }

    /// The total number of allocations currently active in this
    /// garbage collector.
    pub fn num_allocations() -> usize {
        GLOBAL_COLLECTOR.num_allocations()
    }

    /// Collect and drop all unused [`ArcGc`] resources.
    pub fn collect() {
        GLOBAL_COLLECTOR.collect();
    }
}

impl Collector for GlobalRtGc {
    fn register<T>(&self, data: Arc<T>)
    where
        T: ?Sized + Send + Sync + 'static,
        Arc<T>: StrongCount,
    {
        GLOBAL_COLLECTOR.register(data);
    }

    fn remove<T>(&self, data: &Arc<T>)
    where
        T: ?Sized + Send + Sync + 'static,
        Arc<T>: StrongCount,
    {
        GLOBAL_COLLECTOR.remove(data);
    }
}

/// A trait for type-erasing `Arc<T>` types.
pub trait StrongCount: Send + Sync {
    fn count(&self) -> usize;
}

impl<T: Send + Sync + ?Sized> StrongCount for Arc<T> {
    fn count(&self) -> usize {
        Arc::strong_count(self)
    }
}

/// A realtime-safe wrapper around [`Arc`] that when dropped, automatically
/// has its contents collected and later deallocated on another non-realtime
/// thread.
///
/// The performance characteristics of [`ArcGc`] are equivalant to [`Arc`]
/// when reading (but constructing an [`ArcGc`] is a bit more expensive).
///
/// Equality checking between instances of [`ArcGc`] relies _only_ on
/// pointer equivalence. If you need to evaluate the equality of the
/// values contained by [`ArcGc`], you'll need to be careful to ensure you
/// explicitly take references of the inner data.
///
/// # Example
///
/// ```rust
/// # use rtgc::*;
/// # use std::time::Duration;
/// let value: ArcGc<String> = ArcGc::new(String::from("foo"));
///
/// // A simulated "realtime thread"
/// let rt_thread = std::thread::spawn(move || {
///     std::thread::sleep(Duration::from_millis(15));
///
///     // Dropping the values on the realtime thread is realtime-safe
///     // because the contents are automatically collected and
///     // deallocated on a separate non-realtime thread.
///     let _ = value;
/// });
///
/// // A simulated update loop on the main thread
/// for _ in 0..4 {
///     // Call `GlobalRtGc::collect()` periodically to deallocate
///     // any resources that were dropped on the realtime thread.
///     GlobalRtGc::collect();
///
///     std::thread::sleep(Duration::from_millis(15));
/// }
/// ```
pub struct ArcGc<T: ?Sized + Send + Sync + 'static, C: Collector = GlobalRtGc> {
    data: Arc<T>,
    collector: C,
}

impl<T: Send + Sync + 'static> ArcGc<T, GlobalRtGc> {
    /// Construct a new [`ArcGc`].
    ///
    /// ```
    /// # use rtgc::*;
    /// let value: ArcGc<String> = ArcGc::new(String::from("foo"));
    /// ```
    pub fn new(value: T) -> Self {
        let data = Arc::new(value);

        GLOBAL_COLLECTOR.register(Arc::clone(&data));

        Self {
            data,
            collector: GlobalRtGc::default(),
        }
    }
}

impl<T: ?Sized + Send + Sync + 'static> ArcGc<T, GlobalRtGc> {
    /// Construct a new [`ArcGc`] with _unsized_ data, such as `[T]` or `dyn Trait`.
    ///
    /// ```
    /// # use rtgc::*;
    /// # use std::sync::Arc;
    /// let value_1: ArcGc<[f32]> = ArcGc::new_unsized(
    ///     || Arc::<[f32]>::from([1.0, 2.0, 3.0]),
    /// );
    ///
    /// trait Foo: Send + Sync {}
    /// struct Bar {}
    /// impl Foo for Bar {}
    ///
    /// let value_2: ArcGc<dyn Foo> = ArcGc::new_unsized(
    ///     || Arc::new(Bar {}) as Arc<dyn Foo>,
    /// );
    /// ```
    pub fn new_unsized(f: impl FnOnce() -> Arc<T>) -> Self {
        let data = f();

        GLOBAL_COLLECTOR.register(Arc::clone(&data));

        Self {
            data,
            collector: GlobalRtGc::default(),
        }
    }
}

impl ArcGc<dyn Any + Send + Sync + 'static, GlobalRtGc> {
    /// Construct a type-erased [`ArcGc`].
    ///
    /// ```
    /// # use rtgc::*;
    /// # use std::sync::Arc;
    /// # use std::any::Any;
    /// let value: ArcGc<dyn Any + Send + Sync + 'static> =
    ///     ArcGc::new_any(String::from("foo"));
    /// ```
    pub fn new_any<T: Send + Sync + 'static>(value: T) -> Self {
        ArcGc::<dyn Any + Send + Sync + 'static, GlobalRtGc>::new_unsized(|| {
            let a: Arc<dyn Any + Send + Sync + 'static> = Arc::new(value);
            a
        })
    }
}

impl<T: ?Sized + Send + Sync + 'static, C: Collector> ArcGc<T, C> {
    /// A wrapper around [std::sync::Arc::ptr_eq].
    #[inline(always)]
    pub fn ptr_eq(this: &Self, other: &Self) -> bool {
        Arc::ptr_eq(&this.data, &other.data)
    }
}

impl<T: ?Sized + Send + Sync + 'static, C: Collector> Deref for ArcGc<T, C> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.data
    }
}

impl<T: ?Sized + Send + Sync + 'static, C: Collector> Drop for ArcGc<T, C> {
    fn drop(&mut self) {
        self.collector.remove(&self.data);
    }
}

impl<T: ?Sized + Send + Sync + 'static, C: Collector + Clone> Clone for ArcGc<T, C> {
    fn clone(&self) -> Self {
        Self {
            data: Arc::clone(&self.data),
            collector: self.collector.clone(),
        }
    }
}

impl<T: ?Sized + Send + Sync + 'static, C: Collector> PartialEq for ArcGc<T, C> {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.data, &other.data)
    }
}

impl<T: ?Sized + Send + Sync + 'static, C: Collector> Eq for ArcGc<T, C> {}

impl<T: Debug + ?Sized + Send + Sync + 'static, C: Collector> Debug for ArcGc<T, C> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        Debug::fmt(ArcGc::deref(&self), f)
    }
}

/// A realtime-safe `!Sync` resource that when dropped, automatically has
/// its contents collected and later deallocated on another non-realtime
/// thread.
///
/// The performance characteristics of [`OwnedGc`] are equivalant to [`Arc`]
/// when reading (but constructing an [`OwnedGc`] is a bit more expensive).
///
/// # Example
///
/// ```rust
/// # use rtgc::*;
/// # use std::time::Duration;
/// let value: OwnedGc<String> = OwnedGc::new(String::from("foo"));
///
/// // A simulated "realtime thread"
/// let rt_thread = std::thread::spawn(move || {
///     std::thread::sleep(Duration::from_millis(15));
///
///     // Dropping the values on the realtime thread is realtime-safe
///     // because the contents are automatically collected and
///     // deallocated on a separate non-realtime thread.
///     let _ = value;
/// });
///
/// // A simulated update loop on the main thread
/// for _ in 0..4 {
///     // Call `GlobalRtGc::collect()` periodically to deallocate
///     // any resources that were dropped on the realtime thread.
///     GlobalRtGc::collect();
///
///     std::thread::sleep(Duration::from_millis(15));
/// }
/// ```
pub struct OwnedGc<T: ?Sized + Send + 'static, C: Collector = GlobalRtGc> {
    data: ArcGc<OwnedGcWrapper<T>, C>,
}

impl<T: Send + 'static> OwnedGc<T, GlobalRtGc> {
    /// Construct a new [`OwnedGc`].
    pub fn new(value: T) -> Self {
        Self {
            data: ArcGc::<_, GlobalRtGc>::new(OwnedGcWrapper(UnsafeCell::new(value))),
        }
    }
}

impl<T: ?Sized + Send + 'static, C: Collector> OwnedGc<T, C> {
    /// Get an immutable reference to the owned value.
    pub fn get(&self) -> &T {
        // # Safety
        //
        // `OwnedGc` doesn't implement `Clone`, and the internal `ArcGc` is hidden
        // from the user, so the only two `ArcGc`s to the underlying data that can
        // exist are the one in this struct instance and the one stored in the
        // collector. The collector never uses the data (it only drops it), and so
        // it is gauranteed that the underlying data can only be accessed by one
        // thread at a time.
        //
        // Also, `OwnedGc::get_mut` borrows `self` as mutable, ensuring that
        // mutable borrowing rules will be upheld.
        unsafe { &*UnsafeCell::get(&(*self.data).0) }
    }

    /// Get a mutable reference to the owned value.
    pub fn get_mut(&mut self) -> &mut T {
        // # Safety
        //
        // `OwnedGc` doesn't implement `Clone`, and the internal `ArcGc` is hidden
        // from the user, so the only two `ArcGc`s to the underlying data that can
        // exist are the one in this struct instance and the one stored in the
        // collector. The collector never uses the data (it only drops it), and so
        // it is gauranteed that the underlying data can only be accessed by one
        // thread at a time.
        //
        // Also, `OwnedGc::get_mut` borrows `self` as mutable, ensuring that
        // mutable borrowing rules will be upheld.
        unsafe { &mut *UnsafeCell::get(&(*self.data).0) }
    }
}

impl<T: Send + 'static, C: Collector> OwnedGc<T, C> {
    /// Swap the internal value with the given one.
    pub fn swap(&mut self, data: &mut T) {
        core::mem::swap(self.get_mut(), data);
    }
}

impl<T: ?Sized + Send + 'static, C: Collector> Deref for OwnedGc<T, C> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        self.get()
    }
}

impl<T: ?Sized + Send + 'static, C: Collector> DerefMut for OwnedGc<T, C> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.get_mut()
    }
}

impl<T: Debug + ?Sized + Send + 'static, C: Collector> Debug for OwnedGc<T, C> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        Debug::fmt(self.get(), f)
    }
}

#[repr(transparent)]
struct OwnedGcWrapper<T: ?Sized + Send + 'static>(UnsafeCell<T>);

// # Safety
//
// `OwnedGc` doesn't implement `Clone`, and the internal `ArcGc` is hidden
// from the user, so the only two `ArcGc`s to the underlying data that can
// exist are the one in this struct instance and the one stored in the
// collector. The collector never uses the data (it only drops it), and so
// it is gauranteed that the underlying data can only be accessed by one
// thread at a time.
unsafe impl<T: ?Sized + Send + 'static> Sync for OwnedGcWrapper<T> {}

/// A realtime-safe `!Sync` resource that when dropped, automatically has
/// its contents collected and later deallocated on another non-realtime thread.
///
/// This is similar to [`OwnedGc`], except that it avoids double-indirection for
/// boxed types.
///
/// The performance characteristics of [`OwnedGcUnsized`] are equivalant to [`Arc`]
/// when reading (but constructing an [`OwnedGcUnsized`] is a bit more expensive).
///
/// TODO: This is a workaround until
/// [`CoerceUnsized`](https://doc.rust-lang.org/std/ops/trait.CoerceUnsized.html)
/// is stabilized.
///
/// # Example
///
/// ```rust
/// # use rtgc::*;
/// # use std::time::Duration;
/// let value_1: OwnedGcUnsized<[f32]> = OwnedGcUnsized::new_unsized(
///     vec![0.0, 1.0, 2.0].into_boxed_slice(),
/// );
///
/// trait Foo: Send {}
/// struct Bar {}
/// impl Foo for Bar {}
///
/// let value_2: OwnedGcUnsized<dyn Foo> = OwnedGcUnsized::new_unsized(
///     Box::new(Bar {}),
/// );
///
/// // A simulated "realtime thread"
/// let rt_thread = std::thread::spawn(move || {
///     std::thread::sleep(Duration::from_millis(15));
///
///     // Dropping the values on the realtime thread is realtime-safe
///     // because the contents are automatically collected and
///     // deallocated on a separate non-realtime thread.
///     let _ = value_1;
///     let _ = value_2;
/// });
///
/// // A simulated update loop on the main thread
/// for _ in 0..4 {
///     // Call `GlobalRtGc::collect()` periodically to deallocate
///     // any resources that were dropped on the realtime thread.
///     GlobalRtGc::collect();
///
///     std::thread::sleep(Duration::from_millis(15));
/// }
/// ```
pub struct OwnedGcUnsized<T: ?Sized + Send + 'static, C: Collector = GlobalRtGc> {
    _owned: OwnedGc<Pin<Box<T>>, C>,
    ptr: NonNull<T>,
}

impl<T: ?Sized + Send + 'static> OwnedGcUnsized<T, GlobalRtGc> {
    /// Construct a new [`OwnedGcUnsized`] with _unsized_ data, such as `[T]` or `dyn Trait`.
    ///
    /// ```
    /// # use rtgc::*;
    /// # use std::sync::Arc;
    /// let value_1: OwnedGcUnsized<[f32]> = OwnedGcUnsized::new_unsized(
    ///     vec![0.0, 1.0, 2.0].into_boxed_slice(),
    /// );
    ///
    /// trait Foo: Send {}
    /// struct Bar {}
    /// impl Foo for Bar {}
    ///
    /// let value_2: OwnedGcUnsized<dyn Foo> = OwnedGcUnsized::new_unsized(
    ///     Box::new(Bar {}),
    /// );
    /// ```
    pub fn new_unsized(data: Box<T>) -> Self {
        let pinned = Box::into_pin(data);
        let ptr = NonNull::from_ref(pinned.deref());

        Self {
            _owned: OwnedGc::new(pinned),
            ptr,
        }
    }
}

impl<T: ?Sized + Send + 'static, C: Collector> OwnedGcUnsized<T, C> {
    /// Get an immutable reference to the owned value.
    pub fn get(&self) -> &T {
        // # Safety
        //
        // The underlying data is pinned into place, so this pointer is always valid.
        //
        // `OwnedGcUnsized` doesn't implement `Clone`, and the internal `ArcGc` is
        // hidden from the user, so the only two `ArcGc`s to the underlying data that
        // can exist are the one in this struct instance and the one stored in the
        // collector. The collector never uses the data (it only drops it), and so
        // it is gauranteed that the underlying data can only be accessed by one
        // thread at a time.
        //
        // Additionally, the internal `OwnedGc` is hidden from the user and never
        // gets dereferenced, and `OwnedGcUnsized::get_mut` borrows `self` as
        // mutable, so all mutable borrowing rules are gauranteed to be upheld.
        unsafe { self.ptr.as_ref() }
    }

    /// Get a mutable reference to the owned value.
    pub fn get_mut(&mut self) -> &mut T {
        // # Safety
        //
        // The underlying data is pinned into place, so this pointer is always valid.
        //
        // `OwnedGcUnsized` doesn't implement `Clone`, and the internal `ArcGc` is
        // hidden from the user, so the only two `ArcGc`s to the underlying data that
        // can exist are the one in this struct instance and the one stored in the
        // collector. The collector never uses the data (it only drops it), and so
        // it is gauranteed that the underlying data can only be accessed by one
        // thread at a time.
        //
        // Additionally, the internal `OwnedGc` is hidden from the user and never
        // gets dereferenced, and `OwnedGcUnsized::get_mut` borrows `self` as
        // mutable, so all mutable borrowing rules are gauranteed to be upheld.
        unsafe { self.ptr.as_mut() }
    }
}

// # Safety
//
// `OwnedGcUnsized` doesn't implement `Clone`, and the internal `ArcGc` is
// hidden from the user, so the only two `ArcGc`s to the underlying data that
// can exist are the one in this struct instance and the one stored in the
// collector. The collector never uses the data (it only drops it), and so
// it is gauranteed that the underlying data can only be accessed by one
// thread at a time.
//
// Additionally, the internal `OwnedGc` is hidden from the user and never
// gets dereferenced, and `OwnedGcUnsized::get_mut` borrows `self` as
// mutable, so all mutable borrowing rules are gauranteed to be upheld.
//
// Also, we specify that both the collector and the underlying data itself
// must implement `Send`.
unsafe impl<T: ?Sized + Send + 'static, C: Collector> Send for OwnedGcUnsized<T, C> {}

impl<T: ?Sized + Send + 'static, C: Collector> Deref for OwnedGcUnsized<T, C> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        self.get()
    }
}

impl<T: ?Sized + Send + 'static, C: Collector> DerefMut for OwnedGcUnsized<T, C> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.get_mut()
    }
}

impl<T: Debug + ?Sized + Send + 'static, C: Collector> Debug for OwnedGcUnsized<T, C> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        Debug::fmt(self.get(), f)
    }
}

#[cfg(test)]
mod test {
    use super::*;

    // This will conflict with other tests that use the garbage collector
    // when running multiple tests in parallel. So mark this as "ignored",
    // and then test with `cargo test --locked -- --ignored` in a separate
    // CI step.
    #[test]
    #[ignore]
    fn global_rt_garbage_collector() {
        // Test drop -----------------------------------------------

        assert_eq!(GLOBAL_COLLECTOR.num_allocations(), 0);

        let value = ArcGc::new(1);

        assert_eq!(GLOBAL_COLLECTOR.num_allocations(), 1);
        assert_eq!(GLOBAL_COLLECTOR.any_dropped(), false);

        GLOBAL_COLLECTOR.collect();

        assert_eq!(GLOBAL_COLLECTOR.num_allocations(), 1);
        assert_eq!(GLOBAL_COLLECTOR.any_dropped(), false);

        drop(value);

        // Even though we've dropped the "last reference,"
        // the inner drop won't be called until we do garbage
        // collection.
        assert_eq!(GLOBAL_COLLECTOR.num_allocations(), 1);
        assert_eq!(GLOBAL_COLLECTOR.any_dropped(), true);

        GLOBAL_COLLECTOR.collect();

        assert_eq!(GLOBAL_COLLECTOR.num_allocations(), 0);
        assert_eq!(GLOBAL_COLLECTOR.any_dropped(), false);

        // Test unsized --------------------------------------------

        let value = ArcGc::new_unsized(|| Arc::<[i32]>::from([1, 2, 3]));

        assert_eq!(GLOBAL_COLLECTOR.num_allocations(), 1);
        assert_eq!(GLOBAL_COLLECTOR.any_dropped(), false);

        GLOBAL_COLLECTOR.collect();

        assert_eq!(GLOBAL_COLLECTOR.num_allocations(), 1);
        assert_eq!(GLOBAL_COLLECTOR.any_dropped(), false);

        drop(value);

        assert_eq!(GLOBAL_COLLECTOR.num_allocations(), 1);
        assert_eq!(GLOBAL_COLLECTOR.any_dropped(), true);

        GLOBAL_COLLECTOR.collect();

        assert_eq!(GLOBAL_COLLECTOR.num_allocations(), 0);
        assert_eq!(GLOBAL_COLLECTOR.any_dropped(), false);
    }
}
