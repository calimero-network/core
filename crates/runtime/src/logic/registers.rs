use std::collections::hash_map::{Entry, HashMap};

use crate::errors::HostError;
use crate::logic::{VMLimits, VMLogicResult};

const REGISTER_SIZE: u64 = size_of::<u64>() as u64;

#[derive(Debug, Default)]
pub struct Registers {
    inner: HashMap<u64, Box<[u8]>>,
    total_size: u64,
}

impl Registers {
    pub fn get(&self, id: u64) -> VMLogicResult<&[u8]> {
        self.inner
            .get(&id)
            .map(|v| &**v)
            .ok_or_else(|| HostError::InvalidRegisterId { id }.into())
    }

    pub fn get_len(&self, id: u64) -> Option<u64> {
        self.inner.get(&id).map(|v| v.len() as u64)
    }

    pub fn set<T>(&mut self, limits: &VMLimits, id: u64, data: T) -> VMLogicResult<()>
    where
        T: Into<Box<[u8]>> + AsRef<[u8]>,
    {
        let register_len = self.inner.len();
        let entry = self.inner.entry(id);

        let mut func = || {
            let len = data.as_ref().len() as u64;

            (len <= *limits.max_register_size).then_some(())?;

            let new_usage = REGISTER_SIZE.checked_add(len)?;

            let evicted_usage = match &entry {
                Entry::Occupied(entry) => REGISTER_SIZE.checked_add(entry.get().len() as u64)?,
                Entry::Vacant(_) => ((register_len as u64) < limits.max_registers).then_some(0)?,
            };

            let new_total_usage = self
                .total_size
                .checked_sub(evicted_usage)?
                .checked_add(new_usage)?;

            (new_total_usage <= limits.max_registers_capacity).then_some(())?;

            self.total_size = new_total_usage;

            Some(())
        };

        func().ok_or(HostError::InvalidMemoryAccess)?;

        match entry {
            Entry::Occupied(mut entry) => {
                drop(entry.insert(data.into()));
            }
            Entry::Vacant(entry) => {
                let _ = entry.insert(data.into());
            }
        }

        Ok(())
    }
}
