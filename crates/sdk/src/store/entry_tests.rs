use std::borrow::Cow;

use super::{commit, Entry};
use crate::store::env;

#[test]
fn it_works() {
    let a = Entry::new(10);
    let b = Entry::new("hellur");
    let c = Entry::new((a, b));

    dbg!(&c);

    commit();

    env::should_debug(true);

    let res = borsh::to_vec(&c).unwrap();
    dbg!(&res);

    std::mem::forget(c);

    let c = borsh::from_slice::<Entry<(Entry<i32>, Entry<String>)>>(&res).unwrap();

    dbg!(&c);

    dbg!(c.item());

    let (a, b) = c.item().unwrap();

    dbg!(a);
    dbg!(a.item());

    dbg!(a);
    dbg!(b.item());

    // c.flush(true).unwrap();

    env::storage_inspect();
}
