#[cfg(not(target_arch = "wasm32"))]
use mock as imp;
#[cfg(target_arch = "wasm32")]
use vm as imp;

type Key = [u8; 32];

pub fn storage_read(key: Key) -> Option<Vec<u8>> {
    imp::storage_read(key)
}

pub fn storage_write(key: Key, value: Vec<u8>) {
    let _ = imp::storage_write(key, value);
}

pub fn storage_remove(key: Key) {
    let _ = imp::storage_remove(key);
}

pub fn panic_str(msg: &str) -> ! {
    imp::panic_str(msg)
}

pub fn random_bytes(buf: &mut [u8]) {
    imp::random_bytes(buf);
}

pub fn time_now() -> u64 {
    imp::time_now()
}

#[cfg(target_arch = "wasm32")]
mod vm {
    use super::*;
    use crate::env;

    pub fn storage_read(key: Key) -> Option<Vec<u8>> {
        env::storage_read(&*key)
    }

    pub fn storage_write(key: Key, value: Vec<u8>) {
        let _ = env::storage_write(&*key, &value);
    }

    pub fn storage_remove(key: Key) {
        let _ = env::storage_remove(&*key);
    }

    pub fn panic_str(msg: &str) -> ! {
        env::panic_str(msg)
    }

    pub fn random_bytes(buf: &mut [u8]) {
        env::random_bytes(buf);
    }

    pub fn time_now() -> u64 {
        env::time_now()
    }
}

#[cfg(test)]
pub fn should_debug(yes: bool) {
    imp::should_debug(yes);
}

#[cfg(test)]
pub fn storage_inspect() {
    imp::storage_inspect();
}

#[cfg(not(target_arch = "wasm32"))]
mod mock {
    use std::cell::RefCell;
    use std::collections::BTreeMap;

    use rand::RngCore;

    use super::*;

    thread_local! {
        static DEBUG: RefCell<bool> = RefCell::new(false);
        static STORAGE: RefCell<BTreeMap<Key, Vec<u8>>> = RefCell::default();
    }

    pub fn should_debug(yes: bool) {
        DEBUG.with(|debug| *debug.borrow_mut() = yes);
    }

    pub fn storage_inspect() {
        STORAGE.with(|storage| {
            for (key, value) in storage.borrow().iter() {
                println!("{:?} -> {:?}", key, value);
            }
        });
    }

    pub fn storage_read(key: Key) -> Option<Vec<u8>> {
        STORAGE.with(|storage| {
            let value = storage.borrow_mut().get(&key).cloned();

            if DEBUG.with(|debug| *debug.borrow()) {
                println!(
                    "\x1b[33mstorage_read\x1b[0m({:?}) --[contains]-> {:?}",
                    key, value
                );
            }

            value
        })
    }

    pub fn storage_write(key: Key, value: Vec<u8>) {
        STORAGE.with(|storage| {
            if DEBUG.with(|debug| *debug.borrow()) {
                println!("\x1b[33mstorage_write\x1b[0m({:?}, {:?})", key, value);
            }

            let _ignored = storage.borrow_mut().insert(key, value);
        });
    }

    pub fn storage_remove(key: Key) {
        STORAGE.with(|storage| {
            let evicted = storage.borrow_mut().remove(&key);

            if DEBUG.with(|debug| *debug.borrow()) {
                println!(
                    "\x1b[33mstorage_remove\x1b[0m({:?}) --[evicted]-> {:?}",
                    key, evicted
                );
            }
        });
    }

    pub fn panic_str(msg: &str) -> ! {
        panic!("{}", msg)
    }

    pub fn random_bytes(buf: &mut [u8]) {
        rand::thread_rng().fill_bytes(buf);

        if DEBUG.with(|debug| *debug.borrow()) {
            println!("\x1b[33mrandom_bytes\x1b[0m({:?})", buf);
        }
    }

    pub fn time_now() -> u64 {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;

        if DEBUG.with(|debug| *debug.borrow()) {
            println!("\x1b[33mtime_now\x1b[0m() --[now]-> {:?}", now);
        }

        now
    }
}
