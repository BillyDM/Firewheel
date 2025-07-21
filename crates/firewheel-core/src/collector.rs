//! Garbage-collected smart pointer.

use core::cell::UnsafeCell;
use std::{
    any::Any,
    fmt::Debug,
    ops::{Deref, DerefMut},
};

use bevy_platform::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

/// A wrapper around `Arc` that automatically collects resources
/// from the audio thread and drops them on the main thread.
///
/// The performance characteristics and stack size of [`ArcGc`] are
/// similar to [`Arc`], except that the default [`GlobalCollector`]
/// acquires a mutex lock once during construction.
///
/// Equality checking between instances of [`ArcGc`] relies _only_ on
/// pointer equivalence. If you need to evaluate the equality of the
/// values contained by [`ArcGc`], you'll need to be careful to ensure you
/// explicitly take references of the inner data.
#[derive(Debug, Hash)]
pub struct ArcGc<T: ?Sized + Send + Sync + 'static, C: Collector = GlobalCollector> {
    data: Arc<T>,
    collector: C,
}

impl<T: Send + Sync + 'static> ArcGc<T> {
    /// Construct a new [`ArcGc`].
    pub fn new(value: T) -> Self {
        let data = Arc::new(value);

        let collector = GlobalCollector::default();
        collector.register(Arc::clone(&data));

        Self { data, collector }
    }
}

impl<T: ?Sized + Send + Sync + 'static> ArcGc<T> {
    /// Construct a new [`ArcGc`] with _unsized_ data, such as `[T]`.
    ///
    /// ```
    /// # use firewheel_core::collector::ArcGc;
    /// # use bevy_platform::sync::Arc;
    /// let value = ArcGc::new_unsized(|| Arc::<[i32]>::from([1, 2, 3]));
    /// ```
    pub fn new_unsized(f: impl FnOnce() -> Arc<T>) -> Self {
        let data = f();

        let collector = GlobalCollector::default();
        collector.register(Arc::clone(&data));

        Self { data, collector }
    }
}

impl ArcGc<dyn Any + Send + Sync + 'static> {
    pub fn new_any<T: Send + Sync + 'static>(value: T) -> Self {
        ArcGc::new_unsized(|| {
            let a: Arc<dyn Any + Send + Sync + 'static> = Arc::new(value);
            a
        })
    }
}

impl<T: Send + Sync + 'static, C: Collector> ArcGc<T, C> {
    /// Construct a new [`ArcGc`] with a custom collector.
    pub fn new_in(value: T, collector: C) -> Self {
        let data = Arc::new(value);

        collector.register(Arc::clone(&data));

        Self { data, collector }
    }
}

impl<T: ?Sized + Send + Sync + 'static, C: Collector> ArcGc<T, C> {
    /// Construct a new [`ArcGc`] with _unsized_ data and a custom collector.
    pub fn new_unsized_in(f: impl FnOnce() -> Arc<T>, collector: C) -> Self {
        let data = f();

        collector.register(Arc::clone(&data));

        Self { data, collector }
    }
}

impl<T: ?Sized + Send + Sync + 'static, C: Collector> ArcGc<T, C> {
    /// A wrapper around [bevy_platform::sync::Arc::ptr_eq].
    #[inline(always)]
    pub fn ptr_eq(this: &Self, other: &Self) -> bool {
        Arc::ptr_eq(&this.data, &other.data)
    }
}

impl<T: ?Sized + Send + Sync + 'static> From<Arc<T>> for ArcGc<T> {
    fn from(value: Arc<T>) -> Self {
        ArcGc::new_unsized(|| value)
    }
}

impl<T: ?Sized + Send + Sync + 'static, C: Collector> core::ops::Deref for ArcGc<T, C> {
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

impl<T: ?Sized + Send + Sync + 'static, C: Collector + Clone> PartialEq for ArcGc<T, C> {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.data, &other.data)
    }
}

impl<T: ?Sized + Send + Sync + 'static, C: Collector + Clone> Eq for ArcGc<T, C> {}

/// An owned resource that automatically collects dropped resources from the
/// audio thread and drops them on the main thread.
pub struct OwnedGc<T: ?Sized + Send + 'static, C: Collector = GlobalCollector> {
    data: ArcGc<OwnedGcWrapper<T>, C>,
}

impl<T: Send + 'static> OwnedGc<T> {
    /// Construct a new [`OwnedGc`].
    pub fn new(value: T) -> Self {
        Self {
            data: ArcGc::new(OwnedGcWrapper {
                data: UnsafeCell::new(value),
            }),
        }
    }
}

// TODO: Maybe add [`OwnedGc::new_unsized`] and [`OwnedGc::new_any`] methods once
// [`CoerceUnsized`](https://doc.rust-lang.org/std/ops/trait.CoerceUnsized.html)
// is stabilized?

impl<T: ?Sized + Send + 'static> OwnedGc<T> {
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
        unsafe { &*UnsafeCell::get(&(*self.data).data) }
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
        unsafe { &mut *UnsafeCell::get(&(*self.data).data) }
    }
}

impl<T: Send + 'static> OwnedGc<T> {
    /// Swap the internal value with the given one.
    pub fn swap(&mut self, data: &mut T) {
        core::mem::swap(self.get_mut(), data);
    }
}

impl<T: ?Sized + Send + 'static> Deref for OwnedGc<T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        self.get()
    }
}

impl<T: ?Sized + Send + 'static> DerefMut for OwnedGc<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.get_mut()
    }
}

impl<T: Debug + ?Sized + Send + 'static> Debug for OwnedGc<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Debug::fmt(self.get(), f)
    }
}

struct OwnedGcWrapper<T: ?Sized + Send + 'static> {
    data: UnsafeCell<T>,
}

// # Safety
//
// `OwnedGc` doesn't implement `Clone`, and the internal `ArcGc` is hidden
// from the user, so the only two `ArcGc`s to the underlying data that can
// exist are the one in this struct instance and the one stored in the
// collector. The collector never uses the data (it only drops it), and so
// it is gauranteed that the underlying data can only be accessed by one
// thread at a time.
unsafe impl<T: ?Sized + Send + 'static> Sync for OwnedGcWrapper<T> {}

/// The default garbage collector for [`ArcGc`].
///
/// This uses global statics, so registration and collection
/// runs may block. If you need particular characteristics, consider
/// providing a custom collector.
///
/// To collect all default-constructed [`ArcGc`] instances, simply
/// construct an instance of [`GlobalCollector`] and call
/// [`Collector::collect`].
///
/// ```
/// use firewheel_core::collector::{GlobalCollector, Collector};
///
/// GlobalCollector.collect();
/// ```
#[derive(Default, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GlobalCollector;

static REGISTRY: Mutex<Vec<Box<dyn StrongCount + 'static>>> = Mutex::new(Vec::new());
static ANY_PTR_DROPPED: AtomicBool = AtomicBool::new(false);

impl Collector for GlobalCollector {
    fn register<T>(&self, data: Arc<T>)
    where
        T: ?Sized + Send + Sync + 'static,
        Arc<T>: StrongCount,
    {
        register(&REGISTRY, data)
    }

    fn remove<T>(&self, data: &Arc<T>)
    where
        T: ?Sized + Send + Sync + 'static,
        Arc<T>: StrongCount,
    {
        remove(data, &ANY_PTR_DROPPED)
    }

    fn collect(&self) {
        collect(&REGISTRY, &ANY_PTR_DROPPED)
    }
}

/// Garbage collection utilities.
pub trait Collector {
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

    /// Collect and drop all unused [`ArcGc`] resources.
    fn collect(&self);
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

/// Collect and drop all unused [`ArcGc`] resources.
fn register<T: ?Sized + 'static>(
    registry: &Mutex<Vec<Box<dyn StrongCount + 'static>>>,
    data: Arc<T>,
) where
    Arc<T>: StrongCount,
{
    registry.lock().unwrap().push(Box::new(data));
}

/// Indicate that data has been dropped.
fn remove<T: ?Sized>(data: &Arc<T>, any_dropped: &AtomicBool) {
    if Arc::strong_count(data) == 2 {
        // Relaxed ordering should be sufficient since the collector can always
        // drop it on the next collect cycle.
        any_dropped.store(true, Ordering::Relaxed);
    }
}

/// Collect and drop all unused [`ArcGc`] resources.
fn collect(registry: &Mutex<Vec<Box<dyn StrongCount + 'static>>>, any_dropped: &AtomicBool) {
    // Relaxed ordering should be sufficient since the collector can always
    // drop resources on the next collect cycle.
    if any_dropped.load(Ordering::Relaxed) {
        any_dropped.store(false, Ordering::Relaxed);

        let mut registry = registry.lock().unwrap();

        registry.retain(|ptr| ptr.count() > 1);
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[derive(Default, Clone)]
    struct LocalCollector {
        registry: Arc<Mutex<Vec<Box<dyn StrongCount + 'static>>>>,
        any_dropped: Arc<AtomicBool>,
    }

    impl Collector for LocalCollector {
        fn register<T>(&self, data: Arc<T>)
        where
            T: ?Sized + Send + Sync + 'static,
            Arc<T>: StrongCount,
        {
            register(&self.registry, data)
        }

        fn remove<T>(&self, data: &Arc<T>)
        where
            T: ?Sized + Send + Sync + 'static,
            Arc<T>: StrongCount,
        {
            remove(data, &self.any_dropped)
        }

        fn collect(&self) {
            collect(&self.registry, &self.any_dropped)
        }
    }

    impl LocalCollector {
        fn size(&self) -> usize {
            self.registry.lock().unwrap().len()
        }

        fn any_dropped(&self) -> bool {
            self.any_dropped.load(Ordering::Relaxed)
        }
    }

    #[test]
    fn test_drop_works() {
        let collector = LocalCollector::default();

        assert_eq!(collector.size(), 0);

        let value = ArcGc::new_in(1, collector.clone());

        assert_eq!(collector.size(), 1);
        assert_eq!(collector.any_dropped(), false);

        collector.collect();

        assert_eq!(collector.size(), 1);
        assert_eq!(collector.any_dropped(), false);

        drop(value);

        // Even though we've dropped the "last reference,"
        // the inner drop won't be called until we do garbage
        // collection.
        assert_eq!(collector.size(), 1);
        assert_eq!(collector.any_dropped(), true);

        collector.collect();

        assert_eq!(collector.size(), 0);
        assert_eq!(collector.any_dropped(), false);
    }

    #[test]
    fn test_unsized_works() {
        let collector = LocalCollector::default();

        assert_eq!(collector.size(), 0);

        let value = ArcGc::new_unsized_in(|| Arc::<[i32]>::from([1, 2, 3]), collector.clone());

        assert_eq!(collector.size(), 1);
        assert_eq!(collector.any_dropped(), false);

        collector.collect();

        assert_eq!(collector.size(), 1);
        assert_eq!(collector.any_dropped(), false);

        drop(value);

        assert_eq!(collector.size(), 1);
        assert_eq!(collector.any_dropped(), true);

        collector.collect();

        assert_eq!(collector.size(), 0);
        assert_eq!(collector.any_dropped(), false);
    }
}
