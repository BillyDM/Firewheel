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

    NOTIFY_COUNTER.fetch_add(1, Ordering::Relaxed)
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

    pub(crate) fn from_raw(value: T, counter: u64) -> Self {
        Self { value, counter }
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

impl<T: Copy> Copy for Notify<T> {}

impl<T: RealtimeClone + Send + Sync + 'static> Diff
    for Notify<T>
{
    fn diff<E: super::EventQueue>(
        &self,
        baseline: &Self,
        path: super::PathBuilder,
        event_queue: &mut E,
    ) {
        if self.counter != baseline.counter {
            event_queue.push_param(
                ParamData::any(self.clone()),
                path,
            );
        }
    }
}

impl<T: RealtimeClone + Send + Sync + 'static> Patch
    for Notify<T>
{
    type Patch = Self;

    fn patch(
        data: &ParamData,
        _: &[u32],
    ) -> Result<Self::Patch, super::PatchError> {
        data.downcast_ref()
            .ok_or(super::PatchError::InvalidData)
            .cloned()
    }

    fn apply(&mut self, patch: Self::Patch) {
        *self = patch;
    }
}

impl<T> PartialEq for Notify<T> {
    fn eq(&self, other: &Self) -> bool {
        // under normal usage, it is not possible that the inner value
        // can change without incrementing the counter
        self.counter == other.counter
    }
}

#[cfg(test)]
mod test {
    use crate::diff::PathBuilder;

    use super::*;

    #[test]
    fn test_identical_write() {
        #[cfg(not(feature = "std"))]
        use bevy_platform::prelude::Vec;

        let baseline = Notify::new(0.5f32);
        let mut value = baseline;

        let mut events = Vec::new();
        value.diff(
            &baseline,
            PathBuilder::default(),
            &mut events,
        );
        assert_eq!(events.len(), 0);

        *value = 0.5f32;

        value.diff(
            &baseline,
            PathBuilder::default(),
            &mut events,
        );
        assert_eq!(events.len(), 1);
    }
}

#[cfg(feature = "bevy_reflect")]
mod reflect {
    use super::Notify;

    #[cfg(not(feature = "std"))]
    use bevy_platform::prelude::{Box, ToString};

    impl<T> bevy_reflect::GetTypeRegistration for Notify<T>
    where
        Notify<T>: ::core::any::Any + ::core::marker::Send + ::core::marker::Sync,
        T: Clone
            + Default
            + bevy_reflect::FromReflect
            + bevy_reflect::TypePath
            + bevy_reflect::MaybeTyped
            + bevy_reflect::__macro_exports::RegisterForReflection,
    {
        fn get_type_registration() -> bevy_reflect::TypeRegistration {
            let mut registration = bevy_reflect::TypeRegistration::of::<Self>();
            registration.insert:: <bevy_reflect::ReflectFromPtr>(bevy_reflect::FromType:: <Self> ::from_type());
            registration.insert::<bevy_reflect::ReflectFromReflect>(
                bevy_reflect::FromType::<Self>::from_type(),
            );
            registration
                .insert::<bevy_reflect::prelude::ReflectDefault>(
                    bevy_reflect::FromType::<Self>::from_type(),
                );
            registration
        }

        #[inline(never)]
        fn register_type_dependencies(registry: &mut bevy_reflect::TypeRegistry) {
            <T as bevy_reflect::__macro_exports::RegisterForReflection>::__register(registry);
            <u64 as bevy_reflect::__macro_exports::RegisterForReflection>::__register(registry);
        }
    }

    impl<T> bevy_reflect::Typed for Notify<T>
    where
        Notify<T>: ::core::any::Any + ::core::marker::Send + ::core::marker::Sync,
        T: Clone
            + bevy_reflect::FromReflect
            + bevy_reflect::TypePath
            + bevy_reflect::MaybeTyped
            + bevy_reflect::__macro_exports::RegisterForReflection,
    {
        #[inline]
        fn type_info() -> &'static bevy_reflect::TypeInfo {
            static CELL: bevy_reflect::utility::GenericTypeInfoCell =
                bevy_reflect::utility::GenericTypeInfoCell::new();
            CELL.get_or_insert::<Self, _>(|| {
                bevy_reflect::TypeInfo::Struct(
                    bevy_reflect::structs::StructInfo::new::<Self>(&[
                        bevy_reflect::NamedField::new::<T>("value").with_custom_attributes(
                            bevy_reflect::attributes::CustomAttributes::default(),
                        ),
                    ])
                    .with_custom_attributes(bevy_reflect::attributes::CustomAttributes::default())
                    .with_generics(bevy_reflect::Generics::from_iter([
                        bevy_reflect::GenericInfo::Type(bevy_reflect::TypeParamInfo::new::<T>(
                            // TODO: Use nicer path once bevy_reflect exposes it.
                            bevy_reflect::__macro_exports::alloc_utils::Cow::Borrowed("T"),
                        )),
                    ])),
                )
            })
        }
    }

    extern crate alloc;
    impl<T> bevy_reflect::TypePath for Notify<T>
    where
        Notify<T>: ::core::any::Any
            + ::core::marker::Send
            + ::core::marker::Sync,
        T: bevy_reflect::TypePath,
    {
        fn type_path() -> &'static str {
            static CELL: bevy_reflect::utility::GenericTypePathCell =
                bevy_reflect::utility::GenericTypePathCell::new();
            CELL.get_or_insert::<Self, _>(|| {
                ::core::ops::Add::<&str>::add(
                    ::core::ops::Add::<&str>::add(
                        ToString::to_string(::core::concat!(
                            ::core::concat!(
                                ::core::concat!(::core::module_path!(), "::"),
                                "Notify"
                            ),
                            "<"
                        )),
                        <T as bevy_reflect::TypePath>::type_path(),
                    ),
                    ">",
                )
            })
        }
        fn short_type_path() -> &'static str {
            static CELL: bevy_reflect::utility::GenericTypePathCell =
                bevy_reflect::utility::GenericTypePathCell::new();
            CELL.get_or_insert::<Self, _>(|| {
                ::core::ops::Add::<&str>::add(
                    ::core::ops::Add::<&str>::add(
                        ToString::to_string(::core::concat!("Notify", "<")),
                        <T as bevy_reflect::TypePath>::short_type_path(),
                    ),
                    ">",
                )
            })
        }
        fn type_ident() -> Option<&'static str> {
            Some("Notify")
        }
        fn crate_name() -> Option<&'static str> {
            Some(
                ::core::module_path!()
                    .split(':')
                    .next()
                    .unwrap(),
            )
        }
        fn module_path() -> Option<&'static str> {
            Some(::core::module_path!())
        }
    }

    impl<T> bevy_reflect::Reflect for Notify<T>
    where
        Notify<T>: ::core::any::Any + ::core::marker::Send + ::core::marker::Sync,
        T: Clone
            + bevy_reflect::FromReflect
            + bevy_reflect::TypePath
            + bevy_reflect::MaybeTyped
            + bevy_reflect::__macro_exports::RegisterForReflection,
    {
        #[inline]
        fn into_any(self: Box<Self>) -> Box<dyn ::core::any::Any> {
            self
        }
        #[inline]
        fn as_any(&self) -> &dyn ::core::any::Any {
            self
        }
        #[inline]
        fn as_any_mut(&mut self) -> &mut dyn ::core::any::Any {
            self
        }
        #[inline]
        fn into_reflect(self: Box<Self>) -> Box<dyn bevy_reflect::Reflect> {
            self
        }
        #[inline]
        fn as_reflect(&self) -> &dyn bevy_reflect::Reflect {
            self
        }
        #[inline]
        fn as_reflect_mut(&mut self) -> &mut dyn bevy_reflect::Reflect {
            self
        }
        #[inline]
        fn set(
            &mut self,
            value: Box<dyn bevy_reflect::Reflect>,
        ) -> Result<(), Box<dyn bevy_reflect::Reflect>> {
            *self = <dyn bevy_reflect::Reflect>::take(value)?;
            Ok(())
        }
    }

    impl<T> bevy_reflect::prelude::Struct for Notify<T>
    where
        Notify<T>: ::core::any::Any + ::core::marker::Send + ::core::marker::Sync,
        T: Clone
            + bevy_reflect::FromReflect
            + bevy_reflect::TypePath
            + bevy_reflect::MaybeTyped
            + bevy_reflect::__macro_exports::RegisterForReflection,
    {
        fn field(&self, name: &str) -> Option<&dyn bevy_reflect::PartialReflect> {
            match name {
                "value" => Some(&self.value),
                _ => None,
            }
        }

        fn field_mut(&mut self, name: &str) -> Option<&mut dyn bevy_reflect::PartialReflect> {
            match name {
                "value" => Some(self.as_mut()),
                _ => None,
            }
        }

        fn field_at(&self, index: usize) -> Option<&dyn bevy_reflect::PartialReflect> {
            match index {
                0usize => Some(&self.value),
                _ => None,
            }
        }

        fn field_at_mut(&mut self, index: usize) -> Option<&mut dyn bevy_reflect::PartialReflect> {
            match index {
                0usize => Some(self.as_mut()),
                _ => None,
            }
        }

        fn name_at(&self, index: usize) -> Option<&str> {
            match index {
                0usize => Some("value"),
                _ => None,
            }
        }

        fn field_len(&self) -> usize {
            1usize
        }

        fn iter_fields<'a>(&'a self) -> bevy_reflect::structs::FieldIter<'a> {
            bevy_reflect::structs::FieldIter::new(self)
        }

        fn to_dynamic_struct(&self) -> bevy_reflect::structs::DynamicStruct {
            let mut dynamic: bevy_reflect::structs::DynamicStruct = Default::default();
            dynamic.set_represented_type(bevy_reflect::PartialReflect::get_represented_type_info(
                self,
            ));
            dynamic.insert_boxed(
                "value",
                bevy_reflect::PartialReflect::to_dynamic(&self.value),
            );
            dynamic
        }
    }

    impl<T> bevy_reflect::PartialReflect for Notify<T>
    where
        Notify<T>: ::core::any::Any + ::core::marker::Send + ::core::marker::Sync,
        T: Clone
            + bevy_reflect::FromReflect
            + bevy_reflect::TypePath
            + bevy_reflect::MaybeTyped
            + bevy_reflect::__macro_exports::RegisterForReflection,
    {
        #[inline]
        fn get_represented_type_info(&self) -> Option<&'static bevy_reflect::TypeInfo> {
            Some(<Self as bevy_reflect::Typed>::type_info())
        }

        #[inline]
        fn try_apply(
            &mut self,
            value: &dyn bevy_reflect::PartialReflect,
        ) -> Result<(), bevy_reflect::ApplyError> {
            if let bevy_reflect::ReflectRef::Struct(struct_value) =
                bevy_reflect::PartialReflect::reflect_ref(value)
            {
                for (i, value) in ::core::iter::Iterator::enumerate(
                    bevy_reflect::prelude::Struct::iter_fields(struct_value),
                ) {
                    let name = bevy_reflect::prelude::Struct::name_at(struct_value, i).unwrap();
                    if let Some(v) = bevy_reflect::prelude::Struct::field_mut(self, name) {
                        bevy_reflect::PartialReflect::try_apply(v, value)?;
                    }
                }
            } else {
                return Result::Err(bevy_reflect::ApplyError::MismatchedKinds {
                    from_kind: bevy_reflect::PartialReflect::reflect_kind(value),
                    to_kind: bevy_reflect::ReflectKind::Struct,
                });
            }
            Ok(())
        }

        #[inline]
        fn reflect_kind(&self) -> bevy_reflect::ReflectKind {
            bevy_reflect::ReflectKind::Struct
        }

        #[inline]
        fn reflect_ref<'a>(&'a self) -> bevy_reflect::ReflectRef<'a> {
            bevy_reflect::ReflectRef::Struct(self)
        }

        #[inline]
        fn reflect_mut<'a>(&'a mut self) -> bevy_reflect::ReflectMut<'a> {
            bevy_reflect::ReflectMut::Struct(self)
        }

        #[inline]
        fn reflect_owned(self: Box<Self>) -> bevy_reflect::ReflectOwned {
            bevy_reflect::ReflectOwned::Struct(self)
        }

        #[inline]
        fn try_into_reflect(
            self: Box<Self>,
        ) -> Result<Box<dyn bevy_reflect::Reflect>, Box<dyn bevy_reflect::PartialReflect>> {
            Ok(self)
        }

        #[inline]
        fn try_as_reflect(&self) -> Option<&dyn bevy_reflect::Reflect> {
            Some(self)
        }

        #[inline]
        fn try_as_reflect_mut(&mut self) -> Option<&mut dyn bevy_reflect::Reflect> {
            Some(self)
        }

        #[inline]
        fn into_partial_reflect(self: Box<Self>) -> Box<dyn bevy_reflect::PartialReflect> {
            self
        }

        #[inline]
        fn as_partial_reflect(&self) -> &dyn bevy_reflect::PartialReflect {
            self
        }

        #[inline]
        fn as_partial_reflect_mut(&mut self) -> &mut dyn bevy_reflect::PartialReflect {
            self
        }

        fn reflect_partial_eq(&self, value: &dyn bevy_reflect::PartialReflect) -> Option<bool> {
            (bevy_reflect::structs::struct_partial_eq)(self, value)
        }

        #[inline]
        fn reflect_clone(
            &self,
        ) -> Result<Box<dyn bevy_reflect::Reflect>, bevy_reflect::ReflectCloneError> {
            Ok(Box::new(Clone::clone(self)))
        }
    }

    impl<T> bevy_reflect::FromReflect for Notify<T>
    where
        Notify<T>: ::core::any::Any + ::core::marker::Send + ::core::marker::Sync,
        T: Default
            + Clone
            + bevy_reflect::FromReflect
            + bevy_reflect::TypePath
            + bevy_reflect::MaybeTyped
            + bevy_reflect::__macro_exports::RegisterForReflection,
    {
        fn from_reflect(reflect: &dyn bevy_reflect::PartialReflect) -> Option<Self> {
            if let bevy_reflect::ReflectRef::Struct(ref_struct) =
                bevy_reflect::PartialReflect::reflect_ref(reflect)
            {
                let mut this = <Self as ::core::default::Default>::default();
                if let Some(field) = (|| {
                    <T as bevy_reflect::FromReflect>::from_reflect(bevy_reflect::prelude::Struct::field(
                        ref_struct, "value",
                    )?)
                })() {
                    this.value = field;
                }
                Some(this)
            } else {
                None
            }
        }
    }
}
