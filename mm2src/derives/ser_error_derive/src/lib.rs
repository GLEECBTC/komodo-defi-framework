use proc_macro::{self, TokenStream};
use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::quote;
use syn::Meta::{List, NameValue, Path};
use syn::NestedMeta;
use syn::{parse_macro_input, Data, DeriveInput, Error};

use ser_error::CONTENT;
use ser_error::TAG;

const SERDE_IDENT: &str = "serde";
const TAG_ATTR: &str = "tag";
const CONTENT_ATTR: &str = "content";
const UNTAGGED_ATTR: &str = "untagged";

macro_rules! compile_err {
    ($($arg:tt)*) => {
        $crate::CompileError(format!($($arg)*))
    };
}

struct CompileError(String);

impl From<CompileError> for TokenStream {
    fn from(e: CompileError) -> Self {
        TokenStream2::from(e).into()
    }
}

impl From<CompileError> for TokenStream2 {
    fn from(e: CompileError) -> Self {
        Error::new(Span::call_site(), e.0).to_compile_error()
    }
}

/// Use the same `serde` attributes as in the `serde-derive` crate to check if the container satisfies the following statements:
///
/// # Struct not supported yet!
///
/// # Enum
///
/// A enum must have `serde(tag = "error_type", content = "error_data")` attributes.
#[proc_macro_derive(SerializeErrorType, attributes(serde))]
pub fn serialize_error_type(input: TokenStream) -> TokenStream {
    let input: DeriveInput = parse_macro_input!(input);

    match input.data {
        Data::Enum(_) => match check_enum_attributes(&input) {
            Ok(()) => (),
            Err(e) => return e.into(),
        },
        Data::Struct(_) => return compile_err!("'SerializeErrorType' cannot be implement for a struct yet").into(),
        Data::Union(_) => return compile_err!("'SerializeErrorType' cannot be implement for a union").into(),
    }

    let ident = input.ident;
    let (impl_generics, type_generics, where_clause) = input.generics.split_for_impl();

    let output = quote! {
        #[automatically_derived]
        impl #impl_generics ser_error::__private::SerializeErrorTypeImpl for #ident #type_generics #where_clause {}
    };

    wrap_const(output)
}

fn check_enum_attributes(input: &DeriveInput) -> Result<(), CompileError> {
    let mut tag = None;
    let mut content = None;

    for meta_item in input.attrs.iter().flat_map(get_serde_meta_items) {
        match meta_item {
            NestedMeta::Meta(NameValue(m)) if m.path.is_ident(TAG_ATTR) => tag = Some(parse_lit_str(TAG_ATTR, m.lit)?),
            NestedMeta::Meta(NameValue(m)) if m.path.is_ident(CONTENT_ATTR) => {
                content = Some(parse_lit_str(CONTENT_ATTR, m.lit)?)
            },
            NestedMeta::Meta(Path(word)) if word.is_ident(UNTAGGED_ATTR) => {
                return Err(compile_err!(
                    "'SerializeErrorType' can be implemented for tagged enum only"
                ));
            },
            _ => (),
        }
    }

    match tag.as_deref() {
        Some(TAG) => (),
        Some(tag) => {
            return Err(compile_err!(
                "'SerializeErrorType': expected tag = \"{}\", found tag = \"{}\"",
                TAG,
                tag
            ))
        },
        None => {
            return Err(compile_err!(
                "'SerializeErrorType': expected #[serde(tag = \"{}\")]",
                TAG
            ))
        },
    };
    match content.as_deref() {
        Some(CONTENT) => (),
        Some(content) => {
            return Err(compile_err!(
                "'SerializeErrorType': expected content = \"{}\", found content = \"{}\"",
                CONTENT,
                content
            ))
        },
        None => {
            return Err(compile_err!(
                "'SerializeErrorType': expected #[serde(content = \"{}\")]",
                CONTENT
            ))
        },
    }
    Ok(())
}

fn get_serde_meta_items(attr: &syn::Attribute) -> Vec<syn::NestedMeta> {
    if !attr.path.is_ident(SERDE_IDENT) {
        return Vec::new();
    }

    match attr.parse_meta() {
        // A meta list is like the `serde(tag = "...")` in `#[serde(tag = "...")]`
        // or `serde(untagged)` in `#[serde(untagged)]`
        Ok(List(meta)) => meta.nested.into_iter().collect(),
        _ => Vec::new(),
    }
}

fn parse_lit_str(attr_ident: &str, lit: syn::Lit) -> Result<String, CompileError> {
    match lit {
        syn::Lit::Str(lit) => Ok(lit.value()),
        _ => Err(compile_err!(
            "expected serde '{}' attribute to be a string: `{} = \"...\"`",
            attr_ident,
            attr_ident
        )),
    }
}

fn wrap_const(code: TokenStream2) -> TokenStream {
    let output = quote! {
        const _: () = {
            extern crate ser_error;
            #code
        };
    };
    output.into()
}
