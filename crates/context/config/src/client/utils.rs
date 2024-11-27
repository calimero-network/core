use core::fmt::{self, Write};

pub fn humanize_iter<I>(iter: I) -> String
where
    I: IntoIterator<Item: fmt::Display>,
{
    let mut res = String::new();

    let mut iter = iter.into_iter().peekable();

    while let Some(item) = iter.next() {
        if !res.is_empty() {
            if iter.peek().is_some() {
                res.push_str(", ");
            } else {
                res.push_str(" and ");
            }
        }

        write!(res, "{}", item).expect("infallible");
    }

    res
}
