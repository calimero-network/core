//! Arguments of `#[app::tee]`.

use syn::meta::ParseNestedMeta;
use syn::{Attribute, LitStr, Meta};

/// The period an `#[app::tee(every = "..")]` attribute declares, in seconds,
/// or `None` for a bare `#[app::tee]`.
pub(crate) fn parse_tee_args(attr: &Attribute) -> syn::Result<Option<u64>> {
    if !matches!(attr.meta, Meta::List(_)) {
        return Ok(None);
    }
    let mut every_secs = None;
    attr.parse_nested_meta(|meta| parse_arg(&meta, &mut every_secs))?;
    Ok(every_secs)
}

/// Parse one argument of `#[app::tee(..)]` into `every_secs`.
fn parse_arg(meta: &ParseNestedMeta<'_>, every_secs: &mut Option<u64>) -> syn::Result<()> {
    if !meta.path.is_ident("every") {
        return Err(meta.error("unknown `#[app::tee]` argument; expected `every = \"..\"`"));
    }
    if every_secs.is_some() {
        return Err(meta.error("`every` is given twice"));
    }
    let lit: LitStr = meta.value()?.parse()?;
    *every_secs = Some(parse_period(&lit.value()).map_err(|msg| syn::Error::new(lit.span(), msg))?);
    Ok(())
}

/// Seconds in a period written as a positive whole number and a unit: `30s`,
/// `5m`, `2h` or `1d`.
fn parse_period(text: &str) -> Result<u64, String> {
    let bad = || {
        format!("`{text}` is not a period; write a whole number and a unit, like `30s`, `5m`, `2h` or `1d`")
    };
    let split = text.find(|c: char| !c.is_ascii_digit()).ok_or_else(bad)?;
    let (count, unit) = text.split_at(split);
    let count: u64 = count.parse().map_err(|_| bad())?;
    let scale = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 3_600,
        "d" => 86_400,
        _ => return Err(bad()),
    };
    match count.checked_mul(scale) {
        Some(0) => Err("a TEE timer's period must be at least `1s`".to_owned()),
        Some(secs) => Ok(secs),
        None => Err(bad()),
    }
}

#[cfg(test)]
mod tests {
    use super::parse_period;

    #[test]
    fn periods_parse_in_every_unit() {
        assert_eq!(parse_period("30s"), Ok(30));
        assert_eq!(parse_period("5m"), Ok(300));
        assert_eq!(parse_period("2h"), Ok(7_200));
        assert_eq!(parse_period("1d"), Ok(86_400));
    }

    #[test]
    fn malformed_or_empty_periods_are_refused() {
        for bad in ["", "30", "s", "0s", "1.5m", "-1s", "10 s", "3w"] {
            assert!(parse_period(bad).is_err(), "{bad:?} must be refused");
        }
    }
}
