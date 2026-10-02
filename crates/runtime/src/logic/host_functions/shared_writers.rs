//! Writer sets of `SharedStorage` cells: the guest reads a cell's current set
//! from the host, which resolves it from governance, and asks for a rotation
//! by recording it on the run's outcome for the node to publish.

use borsh::BorshDeserialize;
use calimero_storage::shared_writers::{CellWriters, RotationRefusal, SharedRotation};

use crate::errors::HostError;
use crate::logic::{sys, VMHostFunctions, VMLogicResult, DIGEST_SIZE};

/// The most rotations one run may record, which bounds the ops a run can cost the group.
pub(super) const MAX_SHARED_ROTATIONS_PER_RUN: usize = 64;

impl VMHostFunctions<'_> {
    /// Puts the writers of the `SharedStorage` cell at `src_cell_ptr` in register
    /// `dest_register_id`, as the host resolves them at this run's governance cut.
    ///
    /// # Returns
    ///
    /// * `0` if they cannot be resolved; the register is untouched and the guest must fail closed.
    /// * `1` if no rotation took effect; the register is untouched and the set stored with the cell stands.
    /// * `2` if the cell was rotated; the register holds `borsh(BTreeMap<AccountId, OpMask>)`.
    ///
    /// # Errors
    ///
    /// * `HostError::InvalidMemoryAccess` if the cell id is not 32 bytes or memory access fails.
    pub fn shared_writers(
        &mut self,
        src_cell_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<u32> {
        // SAFETY: `sys::Buffer<'_>` is a vetted `GuestAbiType` ABI descriptor (a `#[repr(C)]`
        //         layout of `u64`-shaped fields), so reinterpreting the guest bytes as
        //         it is sound; the guest SDK wrote a well-formed instance at this
        //         offset and the read is bounds-checked. See `read_guest_memory_typed`.
        let cell_buf = unsafe { self.read_guest_memory_typed::<sys::Buffer<'_>>(src_cell_ptr)? };
        let cell = *self.read_guest_memory_sized::<DIGEST_SIZE>(&cell_buf)?;

        match self.borrow_logic().storage.shared_writers(&cell) {
            None => Ok(0),
            Some(CellWriters::Genesis) => Ok(1),
            Some(CellWriters::Rotated(writers)) => {
                let bytes = borsh::to_vec(&writers).map_err(|_| HostError::SerializationError)?;
                self.with_logic_mut(|logic| {
                    logic.registers.set(logic.limits, dest_register_id, bytes)
                })?;
                Ok(2)
            }
        }
    }

    /// Records the `borsh(SharedRotation)` at `src_rotation_ptr` on the run's outcome.
    /// Nothing is stored: the node publishes each rotation as a governance op.
    ///
    /// # Errors
    ///
    /// * `HostError::DeserializationError` if the bytes are not exactly one rotation.
    /// * `HostError::InvalidSharedRotation` if either set is empty or the cell is not a cell id,
    ///   since the node could not publish it.
    /// * `HostError::SharedWritersOverflow` if either set is over the bound.
    /// * `HostError::SharedRotationsOverflow` if the run already recorded the most it may.
    /// * `HostError::InvalidMemoryAccess` if memory access fails for the descriptor buffer.
    pub fn shared_writers_rotate(&mut self, src_rotation_ptr: u64) -> VMLogicResult<()> {
        // SAFETY: as in `shared_writers` above.
        let buf = unsafe { self.read_guest_memory_typed::<sys::Buffer<'_>>(src_rotation_ptr)? };
        let bytes = self.read_guest_memory_slice(&buf)?;
        let rotation =
            SharedRotation::try_from_slice(bytes).map_err(|_| HostError::DeserializationError)?;

        match rotation.refusal() {
            None => {}
            Some(RotationRefusal::TooManyWriters) => {
                return Err(HostError::SharedWritersOverflow.into())
            }
            Some(RotationRefusal::EmptySet | RotationRefusal::NotACell) => {
                return Err(HostError::InvalidSharedRotation.into())
            }
        }
        if self.borrow_logic().shared_rotations.len() >= MAX_SHARED_ROTATIONS_PER_RUN {
            return Err(HostError::SharedRotationsOverflow.into());
        }
        self.with_logic_mut(|logic| logic.shared_rotations.push(rotation));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use calimero_account::AccountId;
    use calimero_storage::address::Id;
    use calimero_storage::collections::cell_id;
    use calimero_storage::entities::OpMask;
    use calimero_storage::shared_writers::{
        CellWriters, SharedRotation, Writers, MAX_WRITERS_PER_ROTATION,
    };
    use wasmer::{AsStoreMut, Store};

    use super::MAX_SHARED_ROTATIONS_PER_RUN;
    use crate::errors::HostError;
    use crate::logic::tests::{prepare_guest_buf_descriptor, setup_vm, SimpleMockStorage};
    use crate::logic::{Cow, VMContext, VMLimits, VMLogic, VMLogicError, DIGEST_SIZE};

    const CELL_DESC: u64 = 10;
    const CELL_AT: u64 = 200;
    const ROTATION_DESC: u64 = 40;
    const ROTATION_AT: u64 = 400;
    const REGISTER: u64 = 1;

    fn writers(accounts: impl IntoIterator<Item = u16>) -> Writers {
        accounts
            .into_iter()
            .map(|n| {
                let mut bytes = [0u8; 32];
                bytes[..2].copy_from_slice(&n.to_le_bytes());
                (AccountId::from(bytes), OpMask::FULL)
            })
            .collect()
    }

    fn cell(seed: u8, prior: &Writers) -> Id {
        cell_id(Id::new([seed; 32]), prior)
    }

    fn rotation(seed: u8, prior: Writers, new: Writers) -> SharedRotation {
        SharedRotation {
            cell: cell(seed, &prior),
            prior,
            new,
        }
    }

    fn put(host: &crate::logic::VMHostFunctions<'_>, desc: u64, at: u64, bytes: &[u8]) {
        host.borrow_memory().write(at, bytes).unwrap();
        prepare_guest_buf_descriptor(host, desc, at, bytes.len() as u64);
    }

    fn record(
        host: &mut crate::logic::VMHostFunctions<'_>,
        rotation: &[u8],
    ) -> Result<(), VMLogicError> {
        put(host, ROTATION_DESC, ROTATION_AT, rotation);
        host.shared_writers_rotate(ROTATION_DESC)
    }

    /// The answer the host gives for cell `[7; 32]` and what it leaves in the register.
    fn ask(storage: &mut SimpleMockStorage) -> (u32, Option<Vec<u8>>) {
        let limits = VMLimits::default();
        let (mut logic, mut store) = setup_vm!(storage, &limits, vec![]);
        let mut host = logic.host_functions(store.as_store_mut());
        put(&host, CELL_DESC, CELL_AT, &[7u8; 32]);
        let answer = host.shared_writers(CELL_DESC, REGISTER).unwrap();
        let register = host
            .borrow_logic()
            .registers
            .get(REGISTER)
            .ok()
            .map(<[u8]>::to_vec);
        (answer, register)
    }

    #[test]
    fn a_cell_no_rotation_touched_reads_as_genesis() {
        let mut storage = SimpleMockStorage::new();
        assert_eq!(ask(&mut storage), (1, None));
    }

    #[test]
    fn a_rotated_cell_reads_as_its_writers() {
        let rotated = writers([1, 2]);
        let mut storage = SimpleMockStorage::new();
        storage.fold_writers([7; 32], Some(CellWriters::Rotated(rotated.clone())));
        let (answer, register) = ask(&mut storage);
        assert_eq!(answer, 2);
        assert_eq!(register, Some(borsh::to_vec(&rotated).unwrap()));
    }

    #[test]
    fn a_cell_the_backend_cannot_resolve_reads_as_unresolvable() {
        let mut storage = SimpleMockStorage::new();
        storage.fold_writers([7; 32], None);
        assert_eq!(ask(&mut storage), (0, None));
    }

    #[test]
    fn a_cell_id_that_is_not_32_bytes_is_a_memory_error() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let mut host = logic.host_functions(store.as_store_mut());
        put(&host, CELL_DESC, CELL_AT, &[7u8; 31]);
        assert!(matches!(
            host.shared_writers(CELL_DESC, REGISTER),
            Err(VMLogicError::HostError(HostError::InvalidMemoryAccess))
        ));
    }

    #[test]
    fn a_rotation_request_is_recorded_on_the_run() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let mut host = logic.host_functions(store.as_store_mut());
        let first = rotation(7, writers([1]), writers([1, 2]));
        let second = rotation(8, writers([3]), writers([4]));
        record(&mut host, &borsh::to_vec(&first).unwrap()).unwrap();
        record(&mut host, &borsh::to_vec(&second).unwrap()).unwrap();
        assert_eq!(host.borrow_logic().shared_rotations, vec![first, second]);
    }

    #[test]
    fn recorded_rotations_ride_the_outcome() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, _store) = setup_vm!(&mut storage, &limits, vec![]);
        let asked = rotation(7, writers([1]), writers([1, 2]));
        logic.shared_rotations = vec![asked.clone()];
        assert_eq!(logic.finish(None).shared_rotations, vec![asked]);
    }

    #[test]
    fn a_run_that_rotates_nothing_has_no_rotations() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (logic, _store) = setup_vm!(&mut storage, &limits, vec![]);
        assert!(logic.finish(None).shared_rotations.is_empty());
    }

    #[test]
    fn a_malformed_rotation_is_a_deserialization_error() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let mut host = logic.host_functions(store.as_store_mut());

        let good = borsh::to_vec(&rotation(7, writers([1]), writers([2]))).unwrap();
        let mut trailing = good.clone();
        trailing.push(0);
        for bad in [vec![1, 2, 3], good[..good.len() - 1].to_vec(), trailing] {
            assert!(matches!(
                record(&mut host, &bad),
                Err(VMLogicError::HostError(HostError::DeserializationError))
            ));
        }
        assert!(host.borrow_logic().shared_rotations.is_empty());
    }

    #[test]
    fn a_rotation_the_node_could_not_publish_is_refused_when_asked() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let mut host = logic.host_functions(store.as_store_mut());

        let good = rotation(7, writers([1]), writers([2]));
        let not_a_cell = SharedRotation {
            cell: Id::new([7; 32]),
            ..good.clone()
        };
        let no_new = rotation(7, writers([1]), writers([]));
        let no_prior = rotation(7, writers([]), writers([2]));
        for bad in [not_a_cell, no_new, no_prior] {
            assert!(matches!(
                record(&mut host, &borsh::to_vec(&bad).unwrap()),
                Err(VMLogicError::HostError(HostError::InvalidSharedRotation))
            ));
        }
        assert!(host.borrow_logic().shared_rotations.is_empty());
        record(&mut host, &borsh::to_vec(&good).unwrap()).expect("a well-formed one is taken");
    }

    #[test]
    fn a_rotation_with_more_writers_than_the_bound_is_refused() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let mut host = logic.host_functions(store.as_store_mut());

        let at_bound = writers(0..u16::try_from(MAX_WRITERS_PER_ROTATION).unwrap());
        let over = writers(0..u16::try_from(MAX_WRITERS_PER_ROTATION + 1).unwrap());
        record(
            &mut host,
            &borsh::to_vec(&rotation(7, at_bound.clone(), at_bound.clone())).unwrap(),
        )
        .expect("a set at the bound is taken");
        for bad in [
            rotation(7, over.clone(), at_bound.clone()),
            rotation(7, at_bound, over),
        ] {
            assert!(matches!(
                record(&mut host, &borsh::to_vec(&bad).unwrap()),
                Err(VMLogicError::HostError(HostError::SharedWritersOverflow))
            ));
        }
        assert_eq!(host.borrow_logic().shared_rotations.len(), 1);
    }

    #[test]
    fn a_run_records_at_most_the_bound_of_rotations() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let mut host = logic.host_functions(store.as_store_mut());

        let bytes = borsh::to_vec(&rotation(7, writers([1]), writers([2]))).unwrap();
        for _ in 0..MAX_SHARED_ROTATIONS_PER_RUN {
            record(&mut host, &bytes).expect("within the bound");
        }
        assert!(matches!(
            record(&mut host, &bytes),
            Err(VMLogicError::HostError(HostError::SharedRotationsOverflow))
        ));
        assert_eq!(
            host.borrow_logic().shared_rotations.len(),
            MAX_SHARED_ROTATIONS_PER_RUN
        );
    }

    #[test]
    fn rotations_in_a_run_are_kept_in_the_order_they_were_asked() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let mut host = logic.host_functions(store.as_store_mut());
        let seeds = [3u8, 1, 2];
        for seed in seeds {
            let bytes = borsh::to_vec(&rotation(seed, writers([1]), writers([2]))).unwrap();
            record(&mut host, &bytes).unwrap();
        }
        let cells: Vec<Id> = host
            .borrow_logic()
            .shared_rotations
            .iter()
            .map(|r| r.cell)
            .collect();
        let expected: Vec<Id> = seeds
            .iter()
            .map(|seed| cell(*seed, &writers([1])))
            .collect();
        assert_eq!(cells, expected);
    }
}
