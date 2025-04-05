extern crate proc_macro;

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;

mod diff;
mod firewheel_manifest;
mod patch;

#[proc_macro_derive(Diff, attributes(diff))]
pub fn derive_diff(input: TokenStream) -> TokenStream {
    diff::derive_diff(input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

#[proc_macro_derive(Patch, attributes(diff))]
pub fn derive_patch(input: TokenStream) -> TokenStream {
    patch::derive_patch(input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

fn get_paths() -> (syn::Path, TokenStream2) {
    let firewheel_path =
        firewheel_manifest::FirewheelManifest::default().get_path("firewheel_core");
    let diff_path = quote! { #firewheel_path::diff };

    (firewheel_path, diff_path)
}

fn should_skip(attrs: &[syn::Attribute]) -> bool {
    let mut skip = false;
    for attr in attrs {
        if attr.path().is_ident("diff") {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("skip") {
                    skip = true;
                }

                Ok(())
            })
            .expect("infallible operation");
        }
    }

    skip
}

fn struct_fields(data: &syn::Fields) -> impl Iterator<Item = (syn::Member, &syn::Type)> {
    // NOTE: a trivial optimization would be to automatically
    // flatten structs with only a single field so their
    // paths can be one index shorter.
    data.iter()
        .enumerate()
        .filter(|(_, f)| !should_skip(&f.attrs))
        .map(|(i, f)| (as_member(f.ident.as_ref(), i), &f.ty))
}

fn as_member(ident: Option<&syn::Ident>, index: usize) -> syn::Member {
    ident.map_or_else(
        || syn::Member::from(index),
        |ident| syn::Member::Named(ident.clone()),
    )
}

#[derive(Default)]
struct TypeSet<'a>(Vec<&'a syn::Type>);

impl<'a> TypeSet<'a> {
    pub fn insert(&mut self, ty: &'a syn::Type) -> bool {
        // let already_exists = self.0.iter().any(|existing| match (ty, existing) {
        //     (syn::Type::Path(a), syn::Type::Path(b)) => {
        //         // If we want a concise set of type bounds, we'll
        //         // need additional syn features -- I don't want to write this myself.
        //         a.qself == b.qself
        //             && a.path.segments.len() == b.path.segments.len()
        //             && a.path
        //                 .segments
        //                 .iter()
        //                 .zip(&b.path.segments)
        //                 .all(|(a, b)| {
        //                     a.arguments
        //                 })
        //     }
        //     _ => false,
        // });

        // if already_exists {
        //     return false;
        // }

        self.0.push(ty);
        true
    }
}

impl<'a> IntoIterator for TypeSet<'a> {
    type Item = &'a syn::Type;
    type IntoIter = <Vec<&'a syn::Type> as IntoIterator>::IntoIter;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'a> core::ops::Deref for TypeSet<'a> {
    type Target = [&'a syn::Type];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
