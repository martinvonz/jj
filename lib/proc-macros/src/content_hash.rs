use proc_macro2::{Ident, TokenStream};
use quote::{format_ident, quote, quote_spanned};
use syn::spanned::Spanned;
use syn::{parse_quote, Data, Field, Fields, GenericParam, Generics, Index, Type};

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
        // Generates a match statement with a match arm and hash implementation
        // for each of the variants in the enum.
        Data::Enum(ref data) => {
            let match_hash_statements = data.variants.iter().enumerate().map(|(i, v)| {
                let variant_id = &v.ident;
                match &v.fields {
                    Fields::Named(fields) => {
                        let bindings = enum_bindings(fields.named.iter());
                        let hash_statements =
                            hash_statements_for_enum_fields(i, fields.named.iter());
                        quote_spanned! {v.span() =>
                            Self::#variant_id{ #(#bindings),* } => {
                                #(#hash_statements)*
                            }
                        }
                    }
                    Fields::Unnamed(fields) => {
                        let bindings = enum_bindings(fields.unnamed.iter());
                        let hash_statements =
                            hash_statements_for_enum_fields(i, fields.unnamed.iter());
                        quote_spanned! {v.span() =>
                            Self::#variant_id( #(#bindings),* ) => {
                                #(#hash_statements)*
                            }
                        }
                    }
                    Fields::Unit => {
                        let ix = index_to_ordinal(i);
                        quote_spanned! {v.span() =>
                            Self::#variant_id => {
                                ::jj_lib::content_hash::ContentHash::hash(&#ix, state);
                            }
                        }
                    }
                }
            });
            quote! {
                match self {
                    #(#match_hash_statements)*
                }
            }
        }
        Data::Union(_) => unimplemented!("ContentHash cannot be derived for unions."),
    }
}

// The documentation for `ContentHash` specifies that the hash impl for each
// enum variant should hash the ordinal number of the enum variant as a little
// endian u32 before hashing the variant's fields, if any.
fn index_to_ordinal(ix: usize) -> u32 {
    u32::try_from(ix).expect("The number of enum variants overflows a u32.")
}

fn enum_bindings_with_type<'a>(fields: impl IntoIterator<Item = &'a Field>) -> Vec<(Type, Ident)> {
    fields
        .into_iter()
        .enumerate()
        .map(|(i, f)| {
            // If the field is named, use the name, otherwise generate a placeholder name.
            (
                f.ty.clone(),
                f.ident.clone().unwrap_or(format_ident!("field_{}", i)),
            )
        })
        .collect::<Vec<_>>()
}

fn enum_bindings<'a>(fields: impl IntoIterator<Item = &'a Field>) -> Vec<Ident> {
    enum_bindings_with_type(fields)
        .into_iter()
        .map(|(_, b)| b)
        .collect()
}

fn hash_statements_for_enum_fields<'a>(
    index: usize,
    fields: impl IntoIterator<Item = &'a Field>,
) -> Vec<TokenStream> {
    let ix = index_to_ordinal(index);
    let typed_bindings = enum_bindings_with_type(fields);
    let mut hash_statements = Vec::with_capacity(typed_bindings.len() + 1);
    hash_statements.push(quote! {::jj_lib::content_hash::ContentHash::hash(&#ix, state);});
    for (ty, b) in typed_bindings.iter() {
        hash_statements.push(quote_spanned! {b.span() =>
            <#ty as ::jj_lib::content_hash::ContentHash>::hash(#b, state);
        });
    }

    hash_statements
}
