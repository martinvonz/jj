use proc_macro2::TokenStream;
use quote::{quote, quote_spanned};
use syn::spanned::Spanned;
use syn::{Data, Fields, Index};

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
