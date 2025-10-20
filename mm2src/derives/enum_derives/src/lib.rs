use proc_macro::{self, TokenStream};
use proc_macro2::{Ident, Span, TokenStream as TokenStream2};
use quote::quote;
use std::fmt;
use syn::Meta::List;
use syn::{
    parse_macro_input, Data, DataEnum, DeriveInput, Error, Field, Fields, ImplGenerics, Type, TypeGenerics, WhereClause,
};
use syn::{Attribute, NestedMeta, Variant};

mod from_inner;
mod from_stringify;
mod from_trait;

const ENUM_FROM_INNER_IDENT: &str = "EnumFromInner";
const ENUM_VARIANT_LIST_IDENT: &str = "EnumVariantList";

/// Implements `From<Inner>` trait for the given enumeration.
///
/// # Usage
///
/// ```rust
/// use enum_derives::EnumFromInner;
///
/// #[derive(EnumFromInner)]
/// enum FooBar {
///     #[from_inner]
///     Foo(i32),
///     #[from_inner]
///     Bar(&'static str),
/// }
///
/// match FooBar::from(10i32) {
///     FooBar::Foo(10) => (),
///     _ => panic!(),
/// }
/// match FooBar::from("Hello, world") {
///     FooBar::Bar("Hello, world") => (),
///     _ => panic!(),
/// }
/// ```
#[proc_macro_derive(EnumFromInner, attributes(from_inner))]
pub fn enum_from_inner(input: TokenStream) -> TokenStream {
    let input: DeriveInput = parse_macro_input!(input);
    match derive_enum_from_macro(input, MacroAttr::FromInner) {
        Ok(output) => output,
        Err(e) => e.into(),
    }
}

/// Implements `From<Inner>` trait for the given enumeration.
///
/// # Usage
///
/// ```rust
/// use enum_derives::EnumFromTrait;
///
/// #[derive(EnumFromTrait)]
/// enum FooBar {
///     #[from_trait(Foo::foo)]
///     Foo(i32),
///     #[from_trait(Bar::bar)]
///     Bar(&'static str),
/// }
///
/// trait Foo {
///     fn foo(num: i32) -> Self;
/// }
///
/// trait Bar {
///     fn bar(str: &'static str) -> Self;
/// }
///
/// match FooBar::foo(10) {
///     FooBar::Foo(10) => (),
///     _ => panic!(),
/// }
/// match FooBar::bar("Hello, world") {
///     FooBar::Bar("Hello, world") => (),
///     _ => panic!(),
/// }
/// ```
#[proc_macro_derive(EnumFromTrait, attributes(from_trait))]
pub fn enum_from_trait(input: TokenStream) -> TokenStream {
    let input: DeriveInput = parse_macro_input!(input);
    match derive_enum_from_macro(input, MacroAttr::FromTrait) {
        Ok(output) => output,
        Err(e) => e.into(),
    }
}

/// `EnumFromStringify` is very useful for generating `From<T>` trait from one enum to another enum,
/// this procedural macro is used to convert any type that implements `Display` trait to a enum variant with the `String` inner type only.
///
/// ### USAGE:
///
/// ```ignore
/// use enum_derives::EnumFromStringify;
/// use std::fmt::{Display, Formatter};
/// use std::io::{Error, ErrorKind};
///
/// // E.G, this converts from Bar, Man to FooBar::Bar(String)
/// #[derive(Debug, EnumFromStringify, PartialEq, Eq)]
/// pub enum FooBar {
///     #[from_stringify("u64", "std::io::Error")]
///     Bar(String),
/// }
///
/// #[test]
/// fn test_from_stringify() {
/// let num = 6500u64;
/// let expected: FooBar = num.into();
/// assert_eq!(FooBar::Bar(num.to_string()), expected);
///
/// let err = Error::new(ErrorKind::Other, "oh no!");
/// let actual = FooBar::Bar(err.to_string());
/// let expected: FooBar = err.into();
/// assert_eq!(actual, expected);
/// }
///  ```
#[proc_macro_derive(EnumFromStringify, attributes(from_stringify))]
pub fn derive(input: TokenStream) -> TokenStream {
    let ast = parse_macro_input!(input as DeriveInput);
    match derive_enum_from_macro(ast, MacroAttr::FromStringify) {
        Ok(output) => output,
        Err(e) => e.into(),
    }
}

/// `EnumVariantList` is a procedural macro used to generate a method that returns a vector containing all variants of an enum.
/// This macro is intended for use with simple enums (enums without associated data or complex structures).
///
/// ### USAGE:
///
/// ```rust
/// use enum_derives::EnumVariantList;
///
/// #[derive(EnumVariantList)]
/// enum Chain {
///     Avalanche,
///     Bsc,
///     Eth,
///     Fantom,
///     Polygon,
/// }
///
///#[test]
///fn test_enum_variant_list() {
///    let all_chains = Chain::variant_list();
///    assert_eq!(all_chains, vec![
///        Chain::Avalanche,
///        Chain::Bsc,
///        Chain::Eth,
///        Chain::Fantom,
///        Chain::Polygon
///    ]);
///}
/// ```
#[proc_macro_derive(EnumVariantList)]
pub fn enum_variant_list(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = input.ident;

    let variants = match input.data {
        Data::Enum(DataEnum { variants, .. }) => variants,
        Data::Struct(_) => return CompileError::expected_enum(ENUM_VARIANT_LIST_IDENT, "struct").into(),
        Data::Union(_) => return CompileError::expected_enum(ENUM_VARIANT_LIST_IDENT, "union").into(),
    };

    let variant_list: Vec<_> = variants.iter().map(|v| &v.ident).collect();

    let expanded = quote! {
        impl #name {
            pub fn variant_list() -> Vec<#name> {
                vec![ #( #name::#variant_list ),* ]
            }
        }
    };

    TokenStream::from(expanded)
}

#[allow(clippy::enum_variant_names)]
#[derive(Clone, Copy)]
enum MacroAttr {
    /// `from_inner` attribute of the `EnumFromInner` derive macro.
    FromInner,
    /// `from_trait` attribute of the `EnumFromTrait` derive macro.
    FromTrait,
    /// `from_stringify` attribute of the `EnumFromStringify` derive macro.
    FromStringify,
}

impl fmt::Display for MacroAttr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MacroAttr::FromInner => write!(f, "from_inner"),
            MacroAttr::FromTrait => write!(f, "from_trait"),
            MacroAttr::FromStringify => write!(f, "from_stringify"),
        }
    }
}

struct CompileError(String);

impl CompileError {
    fn expected_enum(macro_ident: &str, found: &str) -> CompileError {
        CompileError(format!("'{macro_ident}' cannot be implement for a {found}"))
    }

    fn expected_unnamed_inner(attr: MacroAttr) -> CompileError {
        CompileError(format!(
            "'{attr}' attribute must be used for a variant with one unnamed inner type"
        ))
    }

    fn expected_one_attr_on_variant(attr: MacroAttr) -> CompileError {
        CompileError(format!("An enum variant can have only one '{attr}' attribute"))
    }

    fn attr_must_be_used(attr: MacroAttr) -> CompileError {
        CompileError(format!("'{attr}' must be used at least once"))
    }

    fn expected_string_inner_ident(attr: MacroAttr) -> CompileError {
        CompileError(format!("'{attr}' Expected String as inner ident"))
    }

    fn parsing_error(attr: MacroAttr, err: String) -> CompileError {
        CompileError(format!("'{attr}' Error occured while parsing str. Error: {err}"))
    }
}

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

/// An information about the derive ident.
struct IdentCtx<'a> {
    ident: &'a Ident,
    impl_generics: ImplGenerics<'a>,
    type_generics: TypeGenerics<'a>,
    where_clause: Option<&'a WhereClause>,
}

impl<'a> From<&'a DeriveInput> for IdentCtx<'a> {
    fn from(input: &'a DeriveInput) -> Self {
        let (impl_generics, type_generics, where_clause) = input.generics.split_for_impl();
        IdentCtx {
            ident: &input.ident,
            impl_generics,
            type_generics,
            where_clause,
        }
    }
}

/// Represents an unnamed inner field aka `new type`.
struct UnnamedInnerField<'a> {
    field: &'a Field,
}

impl<'a> UnnamedInnerField<'a> {
    /// Try to get an unnamed inner field of the given `variant`.
    /// `attr` is an attribute identifier that is used to generate correct error message.
    fn try_from_variant(variant: &'a Variant, attr: MacroAttr) -> Result<Self, CompileError> {
        match variant.fields {
            Fields::Unnamed(ref fields) if fields.unnamed.len() == 1 => Ok(UnnamedInnerField {
                field: &fields.unnamed[0],
            }),
            _ => Err(CompileError::expected_unnamed_inner(attr)),
        }
    }

    /// Get a type of the field.
    fn ty(&self) -> &Type {
        &self.field.ty
    }
}

/// An implementation of `EnumFromInner` and `EnumFromTrait` macros.
fn derive_enum_from_macro(input: DeriveInput, attr: MacroAttr) -> Result<TokenStream, CompileError> {
    let enumeration = match input.data {
        Data::Enum(ref enumeration) => enumeration,
        Data::Struct(_) => return Err(CompileError::expected_enum(ENUM_FROM_INNER_IDENT, "struct")),
        Data::Union(_) => return Err(CompileError::expected_enum(ENUM_FROM_INNER_IDENT, "union")),
    };

    let ctx = IdentCtx::from(&input);

    let mut impls = Vec::with_capacity(enumeration.variants.len());
    for variant in enumeration.variants.iter() {
        let maybe_impl = match attr {
            MacroAttr::FromInner => from_inner::impl_from_inner(&ctx, variant)?,
            MacroAttr::FromTrait => from_trait::impl_from_trait(&ctx, variant)?,
            MacroAttr::FromStringify => from_stringify::impl_from_stringify(&ctx, variant)?,
        };
        if let Some(variant_impl) = maybe_impl {
            impls.push(variant_impl);
        }
    }

    if impls.is_empty() {
        return Err(CompileError::attr_must_be_used(attr));
    }

    let output = quote! {
        #(#impls)*
    };

    Ok(wrap_const(output))
}

fn wrap_const(code: TokenStream2) -> TokenStream {
    let output = quote! {
        const _: () = {
            #code
        };
    };
    output.into()
}

/// Get the meta information about the given `attr`.
pub(crate) fn get_attr_meta(attr: &Attribute, attr_ident: MacroAttr) -> Vec<NestedMeta> {
    if !attr.path.is_ident(&attr_ident.to_string()) {
        return Vec::new();
    }

    match attr.parse_meta() {
        // A meta list is like the `serde(tag = "...")` in `#[serde(tag = "...")]`
        // or `serde(untagged)` in `#[serde(untagged)]`
        Ok(List(meta)) => meta.nested.into_iter().collect(),
        _ => Vec::new(),
    }
}
