use super::Map;
use crate::store::entry::commit;
use crate::store::{base, env};

#[test]
fn it_works() {
    env::should_debug(true);

    let mut map = Map::new();

    dbg!(&map);

    map.insert(1, "b".to_owned());
    map.insert(1, "a".to_owned());
    map.insert(1, "c".to_owned());

    let d = map.get(&1);

    dbg!(&d);

    dbg!(&map);

    commit();

    let bytes = borsh::to_vec(&map).unwrap();

    env::storage_inspect();

    // drop(map);

    // let mut map = borsh::from_slice::<Map<i32, String>>(&bytes).unwrap();

    // dbg!(&map.len());

    // map.insert(40, "d".to_owned());

    dbg!(&map);
}
