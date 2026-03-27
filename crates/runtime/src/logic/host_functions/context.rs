use crate::{
    errors::HostError,
    logic::{sys, VMHostFunctions, VMLogicError, VMLogicResult},
};
use calimero_primitives::{alias::Alias, common::DIGEST_SIZE, identity::PublicKey};

impl VMHostFunctions<'_> {
    /// Checks if a public key is a member.
    ///
    /// # Returns
    /// * `1` if the public key is a member of the context;
    /// * `0` if the public key is NOT a member of the context.
    pub fn context_is_member(&self, public_key_ptr: u64) -> VMLogicResult<u32> {
        let pk_buf = unsafe { self.read_guest_memory_typed::<sys::Buffer<'_>>(public_key_ptr)? };
        let public_key = *self.read_guest_memory_sized::<DIGEST_SIZE>(&pk_buf)?;

        let logic = self.borrow_logic();

        if let Some(host) = &logic.context_host {
            Ok(if host.is_member(&public_key) { 1 } else { 0 })
        } else {
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

        let data = borsh::to_vec(&members)
            .map_err(|_| VMLogicError::HostError(HostError::SerializationError))?;

        self.with_logic_mut(|logic| logic.registers.set(logic.limits, dest_register_id, data))?;

        Ok(())
    }

    /// Resolves a Context ID from an alias.
    ///
    /// # Returns
    /// * `1` if alias is found (writes 32-byte ID to `dest_register_id`).
    /// * `0` if alias is not found.
    pub fn context_resolve_alias(
        &mut self,
        alias_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<u32> {
        let alias_buf = unsafe { self.read_guest_memory_typed::<sys::Buffer<'_>>(alias_ptr)? };
        let alias_str = self.read_guest_memory_str(&alias_buf)?;

        let logic = self.borrow_logic();
        let node_client = logic
            .node_client
            .as_ref()
            .ok_or(VMLogicError::HostError(HostError::NodeClientNotAvailable))?;

        let context_id = logic.context.context_id;

        let alias: Alias<PublicKey> = Alias::try_from_str(alias_str)
            .map_err(|_| VMLogicError::HostError(HostError::AliasTooLong(alias_str.len())))?;

        match node_client.resolve_alias(alias, Some(context_id.into())) {
            Ok(Some(target_key)) => {
                let id_bytes = target_key.digest();

                self.with_logic_mut(|logic| {
                    logic
                        .registers
                        .set(logic.limits, dest_register_id, *id_bytes)
                })?;
                Ok(1)
            }
            Ok(None) => Ok(0),
            Err(_) => Err(VMLogicError::HostError(HostError::InvalidMemoryAccess)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logic::{
        tests::{prepare_guest_buf_descriptor, setup_vm, write_str, SimpleMockStorage},
        ContextHost, Cow, VMContext, VMLimits, VMLogic,
    };
    use wasmer::{AsStoreMut, Store};

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
    fn test_context_is_member() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();

        let member = [1u8; DIGEST_SIZE];
        let non_member = [2u8; DIGEST_SIZE];
        let mock_host = MockContextHost {
            members: vec![member],
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
            None,
            context,
            &limits,
            None,
            Some(Box::new(mock_host)),
        );
        logic.with_memory(memory);
        let host = logic.host_functions(store.as_store_mut());

        let pk_ptr = 100u64;
        let pk_buf_ptr = 16u64;

        host.borrow_memory().write(pk_ptr, &member).unwrap();
        prepare_guest_buf_descriptor(&host, pk_buf_ptr, pk_ptr, DIGEST_SIZE as u64);
        assert_eq!(host.context_is_member(pk_buf_ptr).unwrap(), 1);

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
            None,
            context,
            &limits,
            None,
            Some(Box::new(mock_host)),
        );
        logic.with_memory(memory);
        let mut host = logic.host_functions(store.as_store_mut());

        let reg_id = 1;
        host.context_members(reg_id)
            .expect("Host function call `context_members` failed");

        let data = host
            .borrow_logic()
            .registers
            .get(reg_id)
            .expect("Register should be set");
        let members: Vec<[u8; DIGEST_SIZE]> =
            borsh::from_slice(data).expect("Failed to deserialize context members");

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
            None,
            context,
            &limits,
            None,
            Some(Box::new(mock_host)),
        );
        logic.with_memory(memory);
        let mut host = logic.host_functions(store.as_store_mut());

        let reg_id = 1;
        host.context_members(reg_id).unwrap();

        let data = host
            .borrow_logic()
            .registers
            .get(reg_id)
            .expect("Register should be set");
        let members: Vec<[u8; DIGEST_SIZE]> =
            borsh::from_slice(data).expect("Failed to deserialize context members");

        assert!(members.is_empty());
    }

    #[test]
    fn test_context_is_member_without_context_host() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let host = logic.host_functions(store.as_store_mut());

        let public_key = [1u8; DIGEST_SIZE];
        let pk_ptr = 100u64;
        let pk_buf_ptr = 16u64;

        host.borrow_memory().write(pk_ptr, &public_key).unwrap();
        prepare_guest_buf_descriptor(&host, pk_buf_ptr, pk_ptr, DIGEST_SIZE as u64);

        let result = host.context_is_member(pk_buf_ptr).unwrap();
        assert_eq!(result, 0);
    }

    #[test]
    fn test_context_members_without_context_host() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let mut host = logic.host_functions(store.as_store_mut());

        let reg_id = 1;
        host.context_members(reg_id).unwrap();

        let data = host
            .borrow_logic()
            .registers
            .get(reg_id)
            .expect("Register should be set");
        let members: Vec<[u8; DIGEST_SIZE]> =
            borsh::from_slice(data).expect("Failed to deserialize");

        assert!(members.is_empty());
    }

    #[test]
    fn test_context_resolve_alias_without_node_client() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let mut host = logic.host_functions(store.as_store_mut());

        let alias = "my_alias";
        let alias_ptr = 100u64;
        write_str(&host, alias_ptr, alias);
        let alias_buf_ptr = 16u64;
        prepare_guest_buf_descriptor(&host, alias_buf_ptr, alias_ptr, alias.len() as u64);

        let dest_register_id = 1u64;
        let err = host
            .context_resolve_alias(alias_buf_ptr, dest_register_id)
            .unwrap_err();

        assert!(matches!(
            err,
            crate::logic::VMLogicError::HostError(HostError::NodeClientNotAvailable)
        ));
    }
}
