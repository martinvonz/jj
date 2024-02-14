use proc_macro2::TokenStream;
use quote::{quote, quote_spanned};
use syn::spanned::Spanned;
use syn::{parse_quote, Data, Fields, GenericParam, Generics, Index};

pub fn add_trait_bounds(mut generics: Generics) -> Generics {
    for param in &mut generics.params {
        if let GenericParam::Type(ref mut type_param) = *param {
            type_param
                .bounds
                .push(parse_quote!(::jj_lib::content_hash::ContentHash));
        }
    }
    generics
}

pub fn generate_hash_impl(data: &Data) -> TokenStream {
    match *data {
        Data::Struct(ref data) => match data.fields {
            Fields::Named(ref fields) => {
                let hash_statements = fields.named.iter().map(|f| {
                    let field_name = &f.ident;
                    let ty = &f.ty;
                    quote_spanned! {ty.span()=>
                        <#ty as ::jj_lib::content_hash::ContentHash>::hash(
                            &self.#field_name, state);
                    }
                });
                quote! {
                    #(#hash_statements)*
                }
            }
            Fields::Unnamed(ref fields) => {
                let hash_statements = fields.unnamed.iter().enumerate().map(|(i, f)| {
                    let index = Index::from(i);
                    let ty = &f.ty;
                    quote_spanned! {ty.span() =>
                        <#ty as ::jj_lib::content_hash::ContentHash>::hash(&self.#index, state);
                    }
                });
                quote! {
                    #(#hash_statements)*
                }
            }
            Fields::Unit => {
                quote! {}
            }
        },
        _ => unimplemented!("ContentHash can only be derived for structs."),
    }
}
