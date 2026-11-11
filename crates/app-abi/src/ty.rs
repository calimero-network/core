//! ABI type strings (`string`, `u64`, `option<T>`, `list<T>`, `tuple<A, B>`, a Rust type's
//! name) parsed into [`Type`].
//!
//! `#[app::logic]` writes the SDK's own names for the types it converts (`string`, `bytes`,
//! `option<..>`, `list<..>`, `tuple<..>`, `unit`) and any other type as its Rust spelling with
//! the spaces taken out (`BTreeMap<String,String>`, `[u8;32]`, `Page<Booking>`). Both spellings
//! are read here; a Rust type the SDK does not describe further (a record, a serde struct or
//! enum, an alias) is [`Type::Named`].

use std::fmt;

/// An integer type, by its Rust name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Int {
    U8,
    U16,
    U32,
    U64,
    U128,
    Usize,
    I8,
    I16,
    I32,
    I64,
    I128,
    Isize,
}

impl Int {
    fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "u8" => Self::U8,
            "u16" => Self::U16,
            "u32" => Self::U32,
            "u64" => Self::U64,
            "u128" => Self::U128,
            "usize" => Self::Usize,
            "i8" => Self::I8,
            "i16" => Self::I16,
            "i32" => Self::I32,
            "i64" => Self::I64,
            "i128" => Self::I128,
            "isize" => Self::Isize,
            _ => return None,
        })
    }

    /// The Rust name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::U8 => "u8",
            Self::U16 => "u16",
            Self::U32 => "u32",
            Self::U64 => "u64",
            Self::U128 => "u128",
            Self::Usize => "usize",
            Self::I8 => "i8",
            Self::I16 => "i16",
            Self::I32 => "i32",
            Self::I64 => "i64",
            Self::I128 => "i128",
            Self::Isize => "isize",
        }
    }

    /// Whether JSON carries it as a decimal string (D20): `u64` and `i64`.
    #[must_use]
    pub const fn is_string(self) -> bool {
        matches!(self, Self::U64 | Self::I64)
    }

    #[must_use]
    pub const fn signed(self) -> bool {
        matches!(
            self,
            Self::I8 | Self::I16 | Self::I32 | Self::I64 | Self::I128 | Self::Isize
        )
    }

    /// The range of values JSON can carry: `usize`/`isize` are 32-bit (apps run on wasm32),
    /// and `u128`/`i128` go through serde_json's 64-bit numbers, so a larger value does not
    /// encode.
    #[must_use]
    pub const fn range(self) -> (i128, i128) {
        match self {
            Self::U8 => (0, u8::MAX as i128),
            Self::U16 => (0, u16::MAX as i128),
            Self::U32 | Self::Usize => (0, u32::MAX as i128),
            Self::U64 | Self::U128 => (0, u64::MAX as i128),
            Self::I8 => (i8::MIN as i128, i8::MAX as i128),
            Self::I16 => (i16::MIN as i128, i16::MAX as i128),
            Self::I32 | Self::Isize => (i32::MIN as i128, i32::MAX as i128),
            Self::I64 | Self::I128 => (i64::MIN as i128, i64::MAX as i128),
        }
    }
}

/// An ABI type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Type {
    /// `()`: `null` in JSON. A method that returns nothing has `returns: null`.
    Unit,
    Bool,
    Int(Int),
    /// `f32` (`false`) or `f64` (`true`).
    Float(bool),
    Char,
    String,
    /// `Vec<u8>`: base64url without padding in JSON (D20).
    Bytes,
    Option(Box<Type>),
    List(Box<Type>),
    /// `BTreeSet`/`HashSet`: a JSON array of distinct values.
    Set(Box<Type>),
    Tuple(Vec<Type>),
    /// `[T; N]`: a JSON array of exactly `N`.
    Array(Box<Type>, u64),
    /// `BTreeMap`/`HashMap`: a JSON object; serde writes the keys as strings.
    Map(Box<Type>, Box<Type>),
    /// An app type the ABI names but does not describe (version 1 has no type section).
    Named(String),
}

impl Type {
    /// Parses an ABI type string. A string that is not a type this module knows is kept as
    /// [`Type::Named`], never refused: the ABI stays readable whatever a newer SDK writes.
    #[must_use]
    pub fn parse(text: &str) -> Self {
        let tokens = tokenize(text);
        let mut p = Parser { tokens, at: 0 };
        match p.ty() {
            Some(ty) if p.at == p.tokens.len() => ty,
            _ => Self::Named(text.replace(' ', "")),
        }
    }

    /// Whether a caller may leave the argument out: an `Option` decodes a missing field as
    /// `None`.
    #[must_use]
    pub const fn is_optional(&self) -> bool {
        matches!(self, Self::Option(_))
    }
}

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unit => f.write_str("unit"),
            Self::Bool => f.write_str("bool"),
            Self::Int(i) => f.write_str(i.name()),
            Self::Float(false) => f.write_str("f32"),
            Self::Float(true) => f.write_str("f64"),
            Self::Char => f.write_str("char"),
            Self::String => f.write_str("string"),
            Self::Bytes => f.write_str("bytes"),
            Self::Option(t) => write!(f, "option<{t}>"),
            Self::List(t) => write!(f, "list<{t}>"),
            Self::Set(t) => write!(f, "set<{t}>"),
            Self::Tuple(ts) => {
                f.write_str("tuple<")?;
                for (i, t) in ts.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{t}")?;
                }
                f.write_str(">")
            }
            Self::Array(t, n) => write!(f, "[{t};{n}]"),
            Self::Map(k, v) => write!(f, "map<{k}, {v}>"),
            Self::Named(n) => f.write_str(n),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Token {
    /// A path (`std::collections::BTreeMap`), an identifier or a number.
    Word(String),
    Punct(char),
}

fn tokenize(text: &str) -> Vec<Token> {
    let mut out = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            let _ = chars.next();
        } else if c.is_alphanumeric() || c == '_' || c == ':' || c == '\'' {
            let mut word = String::new();
            while let Some(&c) = chars.peek() {
                if !(c.is_alphanumeric() || c == '_' || c == ':' || c == '\'') {
                    break;
                }
                word.push(c);
                let _ = chars.next();
            }
            out.push(Token::Word(word));
        } else {
            out.push(Token::Punct(c));
            let _ = chars.next();
        }
    }
    out
}

struct Parser {
    tokens: Vec<Token>,
    at: usize,
}

/// Generic nesting deeper than this is kept as a name rather than recursed into.
const MAX_DEPTH: usize = 32;

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.at)
    }

    fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(&Token::Punct(c)) {
            self.at += 1;
            return true;
        }
        false
    }

    fn ty(&mut self) -> Option<Type> {
        self.ty_at(0)
    }

    fn ty_at(&mut self, depth: usize) -> Option<Type> {
        if depth > MAX_DEPTH {
            return None;
        }
        let start = self.at;
        if self.eat('&') {
            // `&'a mut T`: the reference is not on the wire.
            while let Some(Token::Word(w)) = self.peek() {
                if !(w.starts_with('\'') || w == "mut") {
                    break;
                }
                self.at += 1;
            }
            return self.ty_at(depth + 1);
        }
        if self.eat('(') {
            let mut items = Vec::new();
            let mut trailing = false;
            while !self.eat(')') {
                items.push(self.ty_at(depth + 1)?);
                trailing = self.eat(',');
                if !trailing && self.peek() != Some(&Token::Punct(')')) {
                    return None;
                }
            }
            return Some(match items.len() {
                0 => Type::Unit,
                1 if !trailing => items.pop()?,
                _ => Type::Tuple(items),
            });
        }
        if self.eat('[') {
            let inner = self.ty_at(depth + 1)?;
            if self.eat(']') {
                return Some(slice_of(inner));
            }
            if !self.eat(';') {
                return None;
            }
            let Some(Token::Word(n)) = self.peek().cloned() else {
                return None;
            };
            self.at += 1;
            if !self.eat(']') {
                return None;
            }
            return Some(Type::Array(Box::new(inner), n.parse().ok()?));
        }
        let Some(Token::Word(path)) = self.peek().cloned() else {
            return None;
        };
        self.at += 1;
        let name = path.rsplit("::").next().unwrap_or(&path).to_owned();
        let mut args = Vec::new();
        if self.eat('<') {
            loop {
                // A lifetime argument (`Cow<'a, str>`) is not a type.
                if let Some(Token::Word(w)) = self.peek() {
                    if w.starts_with('\'') {
                        self.at += 1;
                        let _ = self.eat(',');
                        continue;
                    }
                }
                args.push(self.ty_at(depth + 1)?);
                if self.eat('>') {
                    break;
                }
                if !self.eat(',') {
                    return None;
                }
            }
        }
        Some(self.known(&name, args).unwrap_or_else(|| {
            let text: String = self.tokens[start..self.at]
                .iter()
                .map(|t| match t {
                    Token::Word(w) => w.clone(),
                    Token::Punct(c) => c.to_string(),
                })
                .collect();
            Type::Named(text)
        }))
    }

    /// The types the SDK and serde give a fixed JSON form, by their last path segment.
    fn known(&self, name: &str, mut args: Vec<Type>) -> Option<Type> {
        let one = |args: &mut Vec<Type>| (args.len() == 1).then(|| args.remove(0));
        Some(match (name, args.len()) {
            ("unit", 0) => Type::Unit,
            ("bool", 0) => Type::Bool,
            ("f32", 0) => Type::Float(false),
            ("f64", 0) => Type::Float(true),
            ("char", 0) => Type::Char,
            ("string" | "String" | "str", 0) => Type::String,
            ("bytes", 0) => Type::Bytes,
            ("option" | "Option", 1) => Type::Option(Box::new(one(&mut args)?)),
            ("list" | "Vec" | "VecDeque", 1) => slice_of(one(&mut args)?),
            ("BTreeSet" | "HashSet" | "IndexSet", 1) => Type::Set(Box::new(one(&mut args)?)),
            ("Box" | "Rc" | "Arc" | "Cow", 1) => one(&mut args)?,
            ("tuple", n) if n > 0 => Type::Tuple(args),
            ("BTreeMap" | "HashMap" | "IndexMap", 2) => {
                let value = args.pop()?;
                let key = args.pop()?;
                Type::Map(Box::new(key), Box::new(value))
            }
            (other, 0) => Type::Int(Int::parse(other)?),
            _ => return None,
        })
    }
}

/// `Vec<u8>` and `[u8]` are bytes; any other element makes a list.
fn slice_of(inner: Type) -> Type {
    if inner == Type::Int(Int::U8) {
        return Type::Bytes;
    }
    Type::List(Box::new(inner))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(n: &str) -> Type {
        Type::Named(n.to_owned())
    }

    #[test]
    fn sdk_spellings() {
        assert_eq!(Type::parse("string"), Type::String);
        assert_eq!(Type::parse("bytes"), Type::Bytes);
        assert_eq!(Type::parse("unit"), Type::Unit);
        assert_eq!(Type::parse("u64"), Type::Int(Int::U64));
        assert_eq!(
            Type::parse("option<u64>"),
            Type::Option(Box::new(Type::Int(Int::U64)))
        );
        assert_eq!(
            Type::parse("list<Booking>"),
            Type::List(Box::new(named("Booking")))
        );
        assert_eq!(
            Type::parse("tuple<u32, bool>"),
            Type::Tuple(vec![Type::Int(Int::U32), Type::Bool])
        );
    }

    #[test]
    fn rust_spellings() {
        assert_eq!(
            Type::parse("BTreeMap<String,String>"),
            Type::Map(Box::new(Type::String), Box::new(Type::String))
        );
        assert_eq!(
            Type::parse("std::collections::HashMap<String,Vec<Item>>"),
            Type::Map(
                Box::new(Type::String),
                Box::new(Type::List(Box::new(named("Item"))))
            )
        );
        assert_eq!(
            Type::parse("[u8;32]"),
            Type::Array(Box::new(Type::Int(Int::U8)), 32)
        );
        assert_eq!(Type::parse("Vec<u8>"), Type::Bytes);
        assert_eq!(Type::parse("Box<str>"), Type::String);
        assert_eq!(Type::parse("Cow<'a,str>"), Type::String);
        assert_eq!(
            Type::parse("BTreeSet<u32>"),
            Type::Set(Box::new(Type::Int(Int::U32)))
        );
        assert_eq!(Type::parse("()"), Type::Unit);
    }

    #[test]
    fn app_types_keep_their_names() {
        assert_eq!(Type::parse("Page<Booking>"), named("Page<Booking>"));
        assert_eq!(
            Type::parse("crate::types::Room"),
            named("crate::types::Room")
        );
        assert_eq!(Type::parse("Either<u8,String>"), named("Either<u8,String>"));
    }

    #[test]
    fn what_does_not_parse_is_a_name() {
        assert_eq!(Type::parse("option<"), named("option<"));
        assert_eq!(Type::parse("fn(u8)"), named("fn(u8)"));
        let deep = format!("{}u8{}", "list<".repeat(40), ">".repeat(40));
        assert!(matches!(Type::parse(&deep), Type::Named(_)));
    }

    #[test]
    fn display_round_trips_sdk_spellings() {
        for t in [
            "option<list<string>>",
            "tuple<u32, bool>",
            "bytes",
            "Booking",
        ] {
            assert_eq!(Type::parse(t).to_string(), t);
        }
    }
}
