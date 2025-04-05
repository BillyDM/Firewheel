use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{quote, quote_spanned};
use syn::spanned::Spanned;

use crate::{get_paths, struct_fields, TypeSet};

pub fn derive_diff(input: TokenStream) -> syn::Result<TokenStream2> {
    let input: syn::DeriveInput = syn::parse(input)?;
    let identifier = &input.ident;
    let (firewheel_path, diff_path) = get_paths();

    let (impl_generics, ty_generics, where_generics) = input.generics.split_for_impl();

    fn generate_where(
        where_clause: Option<&syn::WhereClause>,
        bounds: &[TokenStream2],
    ) -> TokenStream2 {
        match where_clause {
            Some(wg) => {
                quote! {
                    #wg
                    #(#bounds,)*
                }
            }
            None => {
                quote! {
                    where #(#bounds,)*
                }
            }
        }
    }

    let (body, where_generics) = match &input.data {
        syn::Data::Struct(data) => {
            let DiffOutput { body, bounds } = DiffOutput::from_struct(data, &diff_path)?;

            (body, generate_where(where_generics, &bounds))
        }
        syn::Data::Enum(data) => {
            let DiffOutput { body, bounds } =
                DiffOutput::from_enum(identifier, data, &firewheel_path, &diff_path)?;

            (body, generate_where(where_generics, &bounds))
        }
        syn::Data::Union(_) => {
            return Err(syn::Error::new(
                input.span(),
                "`Diff` cannot be derived on unions.",
            ));
        }
    };

    Ok(quote! {
        impl #impl_generics #diff_path::Diff for #identifier #ty_generics #where_generics {
            fn diff<__E: #diff_path::EventQueue>(&self, baseline: &Self, path: #diff_path::PathBuilder, event_queue: &mut __E) {
                #body
            }
        }
    })
}

struct DiffOutput {
    body: TokenStream2,
    bounds: Vec<TokenStream2>,
}

impl DiffOutput {
    pub fn from_struct(
        data: &syn::DataStruct,
        diff_path: &TokenStream2,
    ) -> syn::Result<DiffOutput> {
        let fields: Vec<_> = struct_fields(&data.fields).collect();

        let arms = fields.iter().enumerate().map(|(i, (identifier, _))| {
            let index = i as u32;
            quote! {
                self.#identifier.diff(&baseline.#identifier, path.with(#index), event_queue);
            }
        });

        let mut types = TypeSet::default();
        for field in &fields {
            types.insert(field.1);
        }

        Ok(DiffOutput {
            body: quote! { #(#arms)* },
            bounds: types
                .into_iter()
                .map(move |ty| {
                    let span = ty.span();
                    quote_spanned! {span=> #ty: #diff_path::Diff }
                })
                .collect(),
        })
    }

    // This is a fair bit more complicated because we need to account for
    // three kinds of variants _and_ we need to be able to construct variants
    // with all required data at once in addition to fine-grained diffing.
    pub fn from_enum(
        identifier: &syn::Ident,
        data: &syn::DataEnum,
        firewheel_path: &syn::Path,
        _: &TokenStream2,
    ) -> syn::Result<DiffOutput> {
        // trivial unit enum
        if data.variants.iter().all(|v| v.fields.is_empty()) {
            let diff_arms = data.variants.iter().enumerate().map(|(i, variant)| {
                let index = i as u32;
                let variant_ident = &variant.ident;

                quote! {
                    (#identifier::#variant_ident, #identifier::#variant_ident) => {}
                    (#identifier::#variant_ident, _) => {
                        event_queue.push_param(
                            #firewheel_path::event::ParamData::U32(#index),
                            path,
                        );
                    }
                }
            });

            let body = quote! {
                match (self, baseline) {
                    #(#diff_arms)*
                }
            };

            return Ok(DiffOutput {
                body,
                bounds: vec![],
            });
        }

        let body = quote! {
            if self != baseline {
                event_queue.push_param(
                    #firewheel_path::event::ParamData::any(<#identifier as ::core::clone::Clone>::clone(self)),
                    path,
                );
            }
        };

        let span = identifier.span();
        Ok(DiffOutput {
            body,
            bounds: vec![quote_spanned! {span=>
                #identifier: ::core::cmp::PartialEq
                        + ::core::clone::Clone
                        + ::core::marker::Send
                        + ::core::marker::Sync
                        + 'static
            }],
        })
    }
}
