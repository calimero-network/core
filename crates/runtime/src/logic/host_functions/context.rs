use crate::{
    errors::HostError,
    logic::{sys, ContextMutation, VMHostFunctions, VMLogicError, VMLogicResult},
};
use calimero_primitives::common::DIGEST_SIZE;

impl VMHostFunctions<'_> {
    /// Requests adding a member to the current context.
    ///
    /// This is a write operation (intent). It does not happen immediately but is
    /// recorded in the execution outcome.
    ///
    /// # Arguments
    /// * `public_key_ptr` - Pointer to the 32-byte public key in guest memory.
    pub fn context_add_member(&mut self, public_key_ptr: u64) -> VMLogicResult<()> {
        let pk_buf = unsafe { self.read_guest_memory_typed::<sys::Buffer<'_>>(public_key_ptr)? };

        let public_key = *self.read_guest_memory_sized::<DIGEST_SIZE>(&pk_buf)?;

        self.with_logic_mut(|logic| {
            logic
                .context_mutations
                .push(ContextMutation::AddMember { public_key });
        });

        Ok(())
    }

    /// Requests removing a member from the current context.
    ///
    /// This is a write operation (intent). It does not happen immediately but is
    /// recorded in the execution outcome.
    ///
    /// # Arguments
    /// * `public_key_ptr` - Pointer to the 32-byte public key in guest memory.
    pub fn context_remove_member(&mut self, public_key_ptr: u64) -> VMLogicResult<()> {
        let pk_buf = unsafe { self.read_guest_memory_typed::<sys::Buffer<'_>>(public_key_ptr)? };

        let public_key = *self.read_guest_memory_sized::<DIGEST_SIZE>(&pk_buf)?;

        self.with_logic_mut(|logic| {
            logic
                .context_mutations
                .push(ContextMutation::RemoveMember { public_key });
        });

        Ok(())
    }

    /// Checks if a public key is a member.
    ///
    /// # Returns
    /// * `1` if the public key is a member of the context;
    /// * `0` if the public key is NOT a member of the context.
    pub fn context_is_member(&self, public_key_ptr: u64) -> VMLogicResult<u32> {
        let pk_buf = unsafe { self.read_guest_memory_typed::<sys::Buffer<'_>>(public_key_ptr)? };
        let public_key = *self.read_guest_memory_sized::<DIGEST_SIZE>(&pk_buf)?;

        let logic = self.borrow_logic();

        // Use the injected trait
        if let Some(host) = &logic.context_host {
            Ok(if host.is_member(&public_key) { 1 } else { 0 })
        } else {
            // If no host is provided (e.g. minimal tests), default to false or error
            // Returning 0 is safer than crashing
            Ok(0)
        }
    }

    /// Lists all members of the context.
    ///
    /// This operation serializes the list (`Vec<[u8;32]>`) using Borsh and writes it to the register.
    pub fn context_members(&mut self, dest_register_id: u64) -> VMLogicResult<()> {
        let members = if let Some(host) = &self.borrow_logic().context_host {
            host.members()
        } else {
            vec![]
        };

        // Serialize using Borsh
        let data = borsh::to_vec(&members)
            .map_err(|_| VMLogicError::HostError(HostError::SerializationError))?;

        self.with_logic_mut(|logic| logic.registers.set(logic.limits, dest_register_id, data))?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logic::{
        tests::{prepare_guest_buf_descriptor, setup_vm, SimpleMockStorage},
        ContextHost, Cow, VMContext, VMLimits, VMLogic,
    };
    use wasmer::{AsStoreMut, Store};

    // Mock implementation for testing
    #[derive(Debug)]
    struct MockContextHost {
        members: Vec<[u8; DIGEST_SIZE]>,
    }

    impl ContextHost for MockContextHost {
        fn is_member(&self, public_key: &[u8; DIGEST_SIZE]) -> bool {
            self.members.contains(public_key)
        }
        fn members(&self) -> Vec<[u8; DIGEST_SIZE]> {
            self.members.clone()
        }
    }

    #[test]
    fn test_context_add_member() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let mut host = logic.host_functions(store.as_store_mut());

        let public_key = [1u8; DIGEST_SIZE];
        let pk_ptr = 100u64;
        let pk_buf_ptr = 16u64;

        // Write PK to guest memory
        host.borrow_memory().write(pk_ptr, &public_key).unwrap();
        prepare_guest_buf_descriptor(&host, pk_buf_ptr, pk_ptr, DIGEST_SIZE as u64);

        // Call host function
        host.context_add_member(pk_buf_ptr).unwrap();

        // Verify logic state
        let mutations = &host.borrow_logic().context_mutations;
        assert_eq!(mutations.len(), 1);
        match mutations[0] {
            ContextMutation::AddMember { public_key: pk } => assert_eq!(pk, public_key),
            _ => panic!("Unexpected mutation type"),
        }
    }

    #[test]
    fn test_context_remove_member() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let mut host = logic.host_functions(store.as_store_mut());

        let public_key = [2u8; DIGEST_SIZE];
        let pk_ptr = 200u64;
        let pk_buf_ptr = 32u64;

        host.borrow_memory().write(pk_ptr, &public_key).unwrap();
        prepare_guest_buf_descriptor(&host, pk_buf_ptr, pk_ptr, DIGEST_SIZE as u64);

        host.context_remove_member(pk_buf_ptr).unwrap();

        let mutations = &host.borrow_logic().context_mutations;
        assert_eq!(mutations.len(), 1);
        match mutations[0] {
            ContextMutation::RemoveMember { public_key: pk } => assert_eq!(pk, public_key),
            _ => panic!("Unexpected mutation type"),
        }
    }

    #[test]
    fn test_context_is_member() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();

        let member = [1u8; DIGEST_SIZE];
        let non_member = [2u8; DIGEST_SIZE];
        let mock_host = MockContextHost {
            members: vec![member],
        };

        // Custom setup to inject mock host
        let context = crate::logic::VMContext::new(
            std::borrow::Cow::Borrowed(&[]),
            [0u8; DIGEST_SIZE],
            [0u8; DIGEST_SIZE],
        );
        let mut store = wasmer::Store::default();
        let memory =
            wasmer::Memory::new(&mut store, wasmer::MemoryType::new(1, None, false)).unwrap();

        let mut logic = crate::logic::VMLogic::new(
            &mut storage,
            context,
            &limits,
            None,
            Some(Box::new(mock_host)),
        );
        logic.with_memory(memory);
        let mut host = logic.host_functions(store.as_store_mut());

        let pk_ptr = 100u64;
        let pk_buf_ptr = 16u64;

        // Check member
        host.borrow_memory().write(pk_ptr, &member).unwrap();
        prepare_guest_buf_descriptor(&host, pk_buf_ptr, pk_ptr, DIGEST_SIZE as u64);
        assert_eq!(host.context_is_member(pk_buf_ptr).unwrap(), 1);

        // Check non-member
        host.borrow_memory().write(pk_ptr, &non_member).unwrap();
        assert_eq!(host.context_is_member(pk_buf_ptr).unwrap(), 0);
    }

    #[test]
    fn test_context_members() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();

        let member1 = [1u8; DIGEST_SIZE];
        let member2 = [2u8; DIGEST_SIZE];
        let member3 = [3u8; DIGEST_SIZE];
        let mock_host = MockContextHost {
            members: vec![member1, member2, member3],
        };

        let context = crate::logic::VMContext::new(
            std::borrow::Cow::Borrowed(&[]),
            [0u8; DIGEST_SIZE],
            [0u8; DIGEST_SIZE],
        );
        let mut store = wasmer::Store::default();
        let memory =
            wasmer::Memory::new(&mut store, wasmer::MemoryType::new(1, None, false)).unwrap();

        let mut logic = crate::logic::VMLogic::new(
            &mut storage,
            context,
            &limits,
            None,
            Some(Box::new(mock_host)),
        );
        logic.with_memory(memory);
        let mut host = logic.host_functions(store.as_store_mut());

        let reg_id = 1;
        // Ask the host to write the members list to the register 1.
        host.context_members(reg_id)
            .expect("Host function call `context_members` failed");

        // Verify register content
        let data = host
            .borrow_logic()
            .registers
            .get(reg_id)
            .expect("Register should be set");
        let members: Vec<[u8; DIGEST_SIZE]> =
            borsh::from_slice(data).expect("Failed to deserialize context members");

        // We should receive 3 members back.
        assert_eq!(members.len(), 3);
        assert!(members.contains(&member1));
        assert!(members.contains(&member2));
        assert!(members.contains(&member3));
    }

    #[test]
    fn test_context_members_empty() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();

        let mock_host = MockContextHost { members: vec![] };

        let context =
            crate::logic::VMContext::new(std::borrow::Cow::Borrowed(&[]), [0u8; 32], [0u8; 32]);
        let mut store = wasmer::Store::default();
        let memory =
            wasmer::Memory::new(&mut store, wasmer::MemoryType::new(1, None, false)).unwrap();

        let mut logic = crate::logic::VMLogic::new(
            &mut storage,
            context,
            &limits,
            None,
            Some(Box::new(mock_host)),
        );
        logic.with_memory(memory);
        let mut host = logic.host_functions(store.as_store_mut());

        let reg_id = 1;
        // Ask the host to write the members list to the register 1.
        host.context_members(reg_id).unwrap();

        // Verify register content.
        let data = host
            .borrow_logic()
            .registers
            .get(reg_id)
            .expect("Register should be set");
        let members: Vec<[u8; DIGEST_SIZE]> =
            borsh::from_slice(data).expect("Failed to deserialize context members");

        // We should receive an empty vec of members.
        assert!(members.is_empty());
    }
}
