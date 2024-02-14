mod content_hash;

extern crate proc_macro;

use quote::quote;
use syn::{parse_macro_input, DeriveInput};

/// Derives the `ContentHash` trait for a struct by calling `ContentHash::hash`
/// on each of the struct members in the order that they're declared. All
/// members of the struct must implement the `ContentHash` trait.
#[proc_macro_derive(ContentHash)]
pub fn derive_content_hash(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let input = parse_macro_input!(input as DeriveInput);

    // The name of the struct.
    let name = &input.ident;

    // Generate an expression to hash each of the fields in the struct.
    let hash_impl = content_hash::generate_hash_impl(&input.data);

    // Handle structs and enums with generics.
    let generics = content_hash::add_trait_bounds(input.generics);
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

    let expanded = quote! {
        #[automatically_derived]
        impl #impl_generics ::jj_lib::content_hash::ContentHash for #name #ty_generics
        #where_clause {
            fn hash(&self, state: &mut impl digest::Update) {
                #hash_impl
            }
        }
    };
    expanded.into()
}
