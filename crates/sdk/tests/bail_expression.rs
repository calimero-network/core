#![allow(unused_crate_dependencies, reason = "False positives")]
#![allow(
    clippy::tests_outside_test_module,
    reason = "Allowable in integration tests"
)]
// Apps build with `-D warnings`, and from Rust 1.99 a macro from another crate
// whose body ends in `;` warns when it is invoked in expression position. This
// file is that other crate, so on 1.99+ it fails to build if `bail!` regains
// the `;`. Older toolchains do not know the lint, hence `unknown_lints`.
#![allow(unknown_lints)]
#![deny(semicolon_in_expressions_from_non_local_macros)]

use calimero_sdk::__bail__ as bail;
use calimero_sdk::app;

fn as_match_arm(seat: &str) -> app::Result<u8> {
    match seat {
        "white" => Ok(0),
        "black" => Ok(1),
        _ => bail!("a seat is either `white` or `black`"),
    }
}

fn as_block_tail(found: bool) -> app::Result<()> {
    if found {
        return Ok(());
    }
    bail!("unknown document")
}

fn as_value(claimable: Option<&'static str>) -> app::Result<&'static str> {
    let reason = match claimable {
        Some(reason) => reason,
        None => bail!("there is no draw to claim in this position"),
    };
    Ok(reason)
}

fn as_statement(empty: bool) -> app::Result<()> {
    if empty {
        bail!("nothing to do");
    }
    Ok(())
}

#[test]
fn bail_works_as_an_expression_and_a_statement() {
    assert_eq!(as_match_arm("black").unwrap(), 1);
    assert!(as_match_arm("red").is_err());
    assert!(as_block_tail(true).is_ok());
    assert!(as_block_tail(false).is_err());
    assert_eq!(as_value(Some("threefold")).unwrap(), "threefold");
    assert!(as_value(None).is_err());
    assert!(as_statement(false).is_ok());
    assert!(as_statement(true).is_err());
}
