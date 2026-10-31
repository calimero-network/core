//! Python's value semantics, for the cluster scenario runner (`scenario.rs`).
//!
//! Cluster scenarios were converted from merobox workflows, whose `assert` and
//! `json_assert` statements and `{{var}}` placeholders were evaluated by Python
//! (`merobox/cloud_v2/assertions.py`, `workflow.py`): `str(x)` of a dict is its
//! repr (`{'a': 1}`), `==` treats `1` and `1.0` (and `True`) as equal, and an
//! embedded placeholder is spliced in as `json.dumps(x)`. A statement that held
//! there holds here only if these are the same, so they are reproduced exactly.

use serde_json::Value;

/// Python's `repr(s)` of a string.
pub fn repr_str(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::with_capacity(s.len() + 2);
    out.push(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\x{:02x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// Python's `repr(f)` of a float.
pub fn repr_float(f: f64) -> String {
    if f.is_nan() {
        return "nan".into();
    }
    if f.is_infinite() {
        return if f > 0.0 { "inf" } else { "-inf" }.into();
    }
    let a = f.abs();
    if a != 0.0 && !(1e-4..1e16).contains(&a) {
        // `1e+16`, `1.5e-05`: Python's exponent has a sign and at least two digits.
        let s = format!("{f:e}");
        let (mantissa, exp) = s.split_once('e').unwrap_or((&s, "0"));
        let (sign, digits) = exp.strip_prefix('-').map_or(("+", exp), |d| ("-", d));
        return format!("{mantissa}e{sign}{digits:0>2}");
    }
    let s = format!("{f}");
    if s.contains('.') {
        s
    } else {
        format!("{s}.0")
    }
}

fn repr_number(n: &serde_json::Number) -> String {
    if n.is_f64() {
        repr_float(n.as_f64().unwrap_or(0.0))
    } else {
        n.to_string()
    }
}

/// Python's `repr(x)` of the object `json.loads` would make of `v`.
pub fn repr(v: &Value) -> String {
    match v {
        Value::Null => "None".into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::Number(n) => repr_number(n),
        Value::String(s) => repr_str(s),
        Value::Array(items) => {
            let parts: Vec<String> = items.iter().map(repr).collect();
            format!("[{}]", parts.join(", "))
        }
        Value::Object(map) => {
            let parts: Vec<String> = map
                .iter()
                .map(|(k, v)| format!("{}: {}", repr_str(k), repr(v)))
                .collect();
            format!("{{{}}}", parts.join(", "))
        }
    }
}

/// Python's `str(x)`: a string as itself, anything else as its repr.
pub fn str_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => repr(other),
    }
}

fn dumps_str(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 || (c as u32) > 0x7e => {
                let mut buf = [0u16; 2];
                for unit in c.encode_utf16(&mut buf) {
                    out.push_str(&format!("\\u{unit:04x}"));
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

fn dumps_into(v: &Value, out: &mut String) {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => {
            if n.is_f64() {
                let f = n.as_f64().unwrap_or(0.0);
                out.push_str(&match repr_float(f).as_str() {
                    "nan" => "NaN".to_owned(),
                    "inf" => "Infinity".to_owned(),
                    "-inf" => "-Infinity".to_owned(),
                    s => s.to_owned(),
                });
            } else {
                out.push_str(&n.to_string());
            }
        }
        Value::String(s) => dumps_str(s, out),
        Value::Array(items) => {
            out.push('[');
            for (i, x) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                dumps_into(x, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            out.push('{');
            for (i, (k, x)) in map.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                dumps_str(k, out);
                out.push_str(": ");
                dumps_into(x, out);
            }
            out.push('}');
        }
    }
}

/// Python's `json.dumps(x)` with its defaults (`", "` and `": "`, ASCII only).
pub fn dumps(v: &Value) -> String {
    let mut out = String::new();
    dumps_into(v, &mut out);
    out
}

/// A number (or bool) as Python compares it: `True == 1 == 1.0`.
fn as_num(v: &Value) -> Option<f64> {
    match v {
        Value::Bool(b) => Some(f64::from(u8::from(*b))),
        Value::Number(n) => n.as_f64(),
        _ => None,
    }
}

/// Python's `a == b` between two objects `json.loads` would make.
pub fn eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) if !x.is_f64() && !y.is_f64() => x == y,
        (Value::Null, Value::Null) => true,
        (Value::String(x), Value::String(y)) => x == y,
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(p, q)| eq(p, q))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| eq(v, w)))
        }
        _ => match (as_num(a), as_num(b)) {
            (Some(x), Some(y)) => x == y,
            _ => false,
        },
    }
}

/// Python's `float(x)`, when it does not raise.
pub fn float(v: &Value) -> Option<f64> {
    match v {
        Value::String(s) => {
            let t = s.trim();
            // Python accepts `_` between digits (`1_000`).
            let cleaned: String = if t.contains('_') {
                let ok = t
                    .split('_')
                    .all(|p| !p.is_empty() && p.chars().next().is_some_and(|c| c.is_ascii_digit()));
                if !ok {
                    return None;
                }
                t.replace('_', "")
            } else {
                t.to_owned()
            };
            cleaned.parse::<f64>().ok()
        }
        other => as_num(other),
    }
}

/// Python's `json.loads(s)`, when it does not raise.
pub fn loads(s: &str) -> Option<Value> {
    serde_json::from_str(s).ok()
}

/// Python's answers for a few values, checked before any scenario runs.
pub fn self_check() {
    use serde_json::json;
    {
        assert_eq!(
            repr(&json!({"a": 1, "b": [true, null, "x'y"], "c": 1.0})),
            r#"{'a': 1, 'b': [True, None, "x'y"], 'c': 1.0}"#
        );
        assert_eq!(
            dumps(&json!({"a": [1, 2], "\u{e9}": "\u{fc}"})),
            "{\"a\": [1, 2], \"\\u00e9\": \"\\u00fc\"}"
        );
        assert_eq!(repr_float(1e16), "1e+16");
        assert_eq!(repr_float(0.00001), "1e-05");
        assert!(eq(&json!(1), &json!(1.0)));
        assert!(eq(&json!(true), &json!(1)));
        assert!(!eq(&json!("1"), &json!(1)));
        assert_eq!(float(&json!(" 3 ")), Some(3.0));
    }
}
