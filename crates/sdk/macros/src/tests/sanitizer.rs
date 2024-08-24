use quote::quote;
use syn::{parse_quote, Ident, Path, Type};

use super::*;

#[test]
fn test_self_sanitizer_simple() {
    let ty = quote! { Self };
    let my_custom_type: Path = parse_quote! { crate::MyCustomType<'a> };

    let mut sanitizer = parse2::<Sanitizer<'_>>(ty).unwrap();

    let cases = [(Case::Self_, Action::ReplaceWith(&my_custom_type))];

    let outcome = sanitizer.sanitize(&cases);

    outcome.check().unwrap();

    assert_eq!(outcome.len(), 1);
    assert_eq!(outcome.count(&Case::Self_), 1);

    let expected = quote! { crate::MyCustomType<'a> };

    assert_eq!(
        sanitizer.to_token_stream().to_string(),
        expected.to_string()
    );
}

#[test]
fn test_ident_sanitizer_complex() {
    let ty = quote! { &Some<Really<[Complex, Deep, Self, Type], Of, &mut Self>> };
    let my_custom_type: Path = parse_quote! { crate::MyCustomType<'a> };
    let complex_type: Ident = parse_quote! { Complex };
    let other_type: Type = parse_quote! { Box<dyn MyTrait<OtherType>> };
    let really_type: Ident = parse_quote! { Really };
    let deep_type: Ident = parse_quote! { Deep };

    let mut sanitizer = parse2::<Sanitizer<'_>>(ty).unwrap();

    let cases = [
        (Case::Self_, Action::ReplaceWith(&my_custom_type)),
        (
            Case::Ident(Some(&complex_type)),
            Action::ReplaceWith(&other_type),
        ),
        (
            Case::Ident(Some(&really_type)),
            Action::ReplaceWith(&complex_type),
        ),
        (Case::Ident(None), Action::ReplaceWith(&really_type)),
        (
            Case::Ident(Some(&deep_type)),
            Action::Forbid(ParseError::UseOfReservedIdent),
        ),
    ];

    let outcome = sanitizer.sanitize(&cases);

    if let Err(err) = outcome.check() {
        let mut errs = err.into_iter();
        assert_eq!(
            errs.next().unwrap().to_string(),
            ParseError::UseOfReservedIdent.to_string()
        );
    }

    assert_eq!(outcome.len(), 4);
    assert_eq!(outcome.count(&Case::Self_), 2);
    assert_eq!(outcome.count(&Case::Ident(Some(&complex_type))), 1);
    assert_eq!(outcome.count(&Case::Ident(None)), 4);
    assert_eq!(outcome.count(&Case::Ident(Some(&really_type))), 1);

    let expected = quote! {
        &Really<
            Complex<
                [
                    Box<
                        dyn MyTrait<OtherType>
                    >,
                    Really,
                    crate::MyCustomType<'a>,
                    Really
                ],
                Really,
                &mut crate::MyCustomType<'a>
            >>
    };

    assert_eq!(
        sanitizer.to_token_stream().to_string(),
        expected.to_string()
    );
}

#[test]
fn test_self_sanitizer_noop() {
    let ty = quote! { &Some<Really<[Complex, Deep, Self, Type], Of, &mut Self>> };

    let mut sanitizer = parse2::<Sanitizer<'_>>(ty.clone()).unwrap();

    let outcome = sanitizer.sanitize(&[]);

    outcome.check().unwrap();

    assert_eq!(outcome.len(), 0);

    assert_eq!(sanitizer.to_token_stream().to_string(), ty.to_string());
}

#[test]
fn test_lifetime_sanitizer_simple() {
    let ty = quote! { &'a Some<'a, Complex<&&&Deep, &Type>> };
    let replace_with = Lifetime::new("'static", Span::call_site());

    let mut sanitizer = parse2::<Sanitizer<'_>>(ty).unwrap();

    let cases = [(Case::Lifetime(None), Action::ReplaceWith(&replace_with))];

    let outcome = sanitizer.sanitize(&cases);

    outcome.check().unwrap();

    assert_eq!(outcome.len(), 1);
    assert_eq!(outcome.count(&Case::Lifetime(None)), 6);

    let expected =
        quote! { &'static Some<'static, Complex<&'static &'static &'static Deep, &'static Type>> };

    assert_eq!(
        sanitizer.to_token_stream().to_string(),
        expected.to_string()
    );
}

#[test]
fn test_lifetime_sanitizer_specialized() {
    let ty = quote! { &'a Some<'a, Complex<&&&Deep, &'b Type>> };
    let a_lifetime = Lifetime::new("'a", Span::call_site());
    let b_lifetime = Lifetime::new("'b", Span::call_site());
    let static_lifetime = Lifetime::new("'static", Span::call_site());

    let mut sanitizer = parse2::<Sanitizer<'_>>(ty).unwrap();

    let cases = [
        (
            Case::Lifetime(Some(&a_lifetime)),
            Action::ReplaceWith(&b_lifetime),
        ),
        (Case::Lifetime(None), Action::ReplaceWith(&static_lifetime)),
        (
            Case::Lifetime(Some(&b_lifetime)),
            Action::ReplaceWith(&a_lifetime),
        ),
    ];

    let outcome = sanitizer.sanitize(&cases);

    outcome.check().unwrap();

    assert_eq!(outcome.len(), 2);
    assert_eq!(outcome.count(&Case::Lifetime(None)), 4);
    assert_eq!(outcome.count(&Case::Lifetime(Some(&a_lifetime))), 2);
    assert_eq!(outcome.count(&Case::Lifetime(Some(&b_lifetime))), 0);

    let expected = quote! { &'b Some<'b, Complex<&'static &'static &'static Deep, &'static Type>> };

    assert_eq!(
        sanitizer.to_token_stream().to_string(),
        expected.to_string()
    );
}

#[test]
fn test_lifetime_sanitizer_complex() {
    let ty = quote! { &'a Some<'a, Complex<&&&Deep, &Type, Box<dyn MyTrait<'b, Output = (&str, &'b str)> + 'b>>> };
    let a_lifetime = Lifetime::new("'a", Span::call_site());
    let b_lifetime = Lifetime::new("'b", Span::call_site());
    let static_lifetime = Lifetime::new("'static", Span::call_site());

    let mut sanitizer = parse2::<Sanitizer<'_>>(ty).unwrap();

    let cases = [
        (
            Case::Lifetime(Some(&a_lifetime)),
            Action::ReplaceWith(&b_lifetime),
        ),
        (
            Case::Lifetime(Some(&b_lifetime)),
            Action::Forbid(ParseError::UseOfReservedLifetime),
        ),
        (Case::Lifetime(None), Action::ReplaceWith(&static_lifetime)),
    ];

    let outcome = sanitizer.sanitize(&cases);

    if let Err(err) = outcome.check() {
        let mut errs = err.into_iter();
        assert_eq!(
            errs.next().unwrap().to_string(),
            ParseError::UseOfReservedLifetime.to_string()
        );
    }

    assert_eq!(outcome.len(), 3);
    assert_eq!(outcome.count(&Case::Lifetime(Some(&a_lifetime))), 2);
    assert_eq!(outcome.count(&Case::Lifetime(Some(&b_lifetime))), 3);
    assert_eq!(outcome.count(&Case::Lifetime(None)), 5);

    let expected = quote! {
        &'b Some<
            'b,
            Complex<
                &'static &'static &'static Deep,
                &'static Type,
                Box<dyn MyTrait<'b, Output = (&'static str, &'b str)> + 'b>>
            >
    };

    assert_eq!(
        sanitizer.to_token_stream().to_string(),
        expected.to_string()
    );
}

#[test]
fn test_lifetime_sanitizer_noop() {
    let ty = quote! { &'a Some<'a, Complex<&&&Deep, &Type>> };

    let mut sanitizer = parse2::<Sanitizer<'_>>(ty.clone()).unwrap();

    let outcome = sanitizer.sanitize(&[]);

    outcome.check().unwrap();

    assert_eq!(outcome.len(), 0);

    assert_eq!(sanitizer.to_token_stream().to_string(), ty.to_string());
}
