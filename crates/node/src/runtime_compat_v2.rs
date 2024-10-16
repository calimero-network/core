use calimero_runtime::store::{Key, Storage, Value};
use calimero_storage::address::{Id, Path};
use calimero_storage::entities::Element;
use calimero_storage::interface::Interface;

#[derive(Debug)]
pub struct RuntimeCompatStore {
    pub interface: Interface,
}

impl RuntimeCompatStore {
    fn key_as_id(&self, key: &Key) -> Option<Id> {
        let mut id = [0; 16];

        (key.len() == id.len()).then_some(())?;

        id.copy_from_slice(&key);

        Some(id.into())
    }
}

impl Storage for RuntimeCompatStore {
    fn get(&self, key: &Key) -> Option<Vec<u8>> {
        let id = self.key_as_id(key)?;

        let element = self.interface.find_by_id(id).ok()??;

        Some(element.data.0)
    }

    fn set(&mut self, key: Key, value: Value) -> Option<Value> {
        let id = self.key_as_id(&key)?;

        let mut old = None;

        let mut element = match self.interface.find_by_id(id).ok()? {
            Some(mut element) => {
                old = Some(std::mem::take(&mut element.data.0));
                element
            }
            None => Element::new(&Path::new("::").ok()?), // ??
        };

        element.data.0 = value;

        old
    }

    fn has(&self, key: &Key) -> bool {
        self.get(key).is_some()
    }
}
