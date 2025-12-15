use crate::{
    errors::HostError,
    logic::{sys, ContextMutation, VMHostFunctions, VMLogicError, VMLogicResult},
};
use calimero_primitives::{alias::Alias, common::DIGEST_SIZE, identity::PublicKey};

impl VMHostFunctions<'_> {
    /// Requests the creation of a new context.
    ///
    /// Before the context request is sent, the function checks if the provided alias is available.
    /// If the alias is taken, request to create a new context is not created, and the function
    /// returns an error.
    ///
    /// # Arguments
    /// * `protocol_ptr` - Pointer to protocol string buffer.
    /// * `app_id_ptr` - Pointer to 32-byte Application ID buffer.
    /// * `args_ptr` - Pointer to initialization arguments buffer.
    /// * `alias_ptr` - Pointer to the buffer containing an optional alias.
    pub fn context_create(
        &mut self,
        protocol_ptr: u64,
        app_id_ptr: u64,
        args_ptr: u64,
        alias_ptr: u64,
    ) -> VMLogicResult<()> {
        let protocol_buf =
            unsafe { self.read_guest_memory_typed::<sys::Buffer<'_>>(protocol_ptr)? };
        let protocol = self.read_guest_memory_str(&protocol_buf)?.to_owned();

        let app_id_buf = unsafe { self.read_guest_memory_typed::<sys::Buffer<'_>>(app_id_ptr)? };
        let application_id = *self.read_guest_memory_sized::<DIGEST_SIZE>(&app_id_buf)?;

        let args_buf = unsafe { self.read_guest_memory_typed::<sys::Buffer<'_>>(args_ptr)? };
        let init_args = self.read_guest_memory_slice(&args_buf).to_vec();

        // Check if alias ptr is non-zero.
        let alias = if alias_ptr != 0 {
            let alias_buf = unsafe { self.read_guest_memory_typed::<sys::Buffer<'_>>(alias_ptr)? };
            if alias_buf.len() > 0 {
                Some(self.read_guest_memory_str(&alias_buf)?.to_owned())
            } else {
                None
            }
        } else {
            None
        };

        // Check if alias exists
        if let Some(alias_str) = &alias {
            let logic = self.borrow_logic();
            if let Some(node) = &logic.node_client {
                let context_id = logic.context.context_id;
                // We scope the alias to the current context to prevent collisions between apps
                let scoped_alias: Alias<PublicKey> =
                    Alias::try_from_str(alias_str).map_err(|_| {
                        VMLogicError::HostError(HostError::AliasTooLong(alias_str.len()))
                    })?;

                // We need to use `lookup_alias` instead of `resolve_alias` to avoid
                // false positives when the alias string is a valid `PublicKey`.
                if node
                    .lookup_alias(scoped_alias, Some(context_id.into()))
                    .is_ok_and(|opt| opt.is_some())
                {
                    return Err(VMLogicError::HostError(HostError::AliasAlreadyExists(
                        alias_str.clone(),
                    )));
                }
            }
        }

        self.with_logic_mut(|logic| {
            logic
                .context_mutations
                .push(ContextMutation::CreateContext {
                    protocol,
                    application_id,
                    init_args,
                    alias,
                });
        });

        Ok(())
    }

    /// Requests the deletion of a context.
    ///
    /// # Arguments
    /// * `context_id_ptr` - Pointer to 32-byte Context ID buffer.
    pub fn context_delete(&mut self, context_id_ptr: u64) -> VMLogicResult<()> {
        let ctx_buf = unsafe { self.read_guest_memory_typed::<sys::Buffer<'_>>(context_id_ptr)? };
        let context_id = *self.read_guest_memory_sized::<DIGEST_SIZE>(&ctx_buf)?;

        self.with_logic_mut(|logic| {
            logic
                .context_mutations
                .push(ContextMutation::DeleteContext { context_id });
        });

        Ok(())
    }

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

        // We are looking for a PublicKey alias scoped to this context
        let alias: Alias<PublicKey> = Alias::try_from_str(alias_str)
            .map_err(|_| VMLogicError::HostError(HostError::AliasTooLong(alias_str.len())))?;

        // Resolve against the current context scope
        match node_client.resolve_alias(alias, Some(context_id.into())) {
            Ok(Some(target_key)) => {
                // target_key is PublicKey, which wraps [u8; 32]
                // This bytes corresponds to the Child Context ID
                let id_bytes = target_key.digest();

                self.with_logic_mut(|logic| {
                    logic
                        .registers
                        .set(logic.limits, dest_register_id, *id_bytes)
                })?;
                Ok(1)
            }
            Ok(None) => Ok(0),
            // Some internal error occured.
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
    fn test_context_create_delete() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let mut host = logic.host_functions(store.as_store_mut());

        // Setup data
        let protocol = "near";
        let app_id = [1u8; DIGEST_SIZE];
        let args = vec![1, 2, 3];
        let context_id = [1u8; DIGEST_SIZE];
        let alias = "my_child_context";

        // Pointers
        let protocol_ptr = 300u64;
        let app_id_ptr = 400u64;
        let args_ptr = 500u64;
        let alias_ptr = 600u64;
        let ctx_ptr = 700u64;

        // Descriptors
        let protocol_buf = 10u64;
        let app_id_buf = 30u64;
        let args_buf = 70u64;
        let alias_buf = 120u64;
        let ctx_buf = 180u64;

        // Write memory
        write_str(&host, protocol_ptr, protocol);
        prepare_guest_buf_descriptor(&host, protocol_buf, protocol_ptr, protocol.len() as u64);

        host.borrow_memory().write(app_id_ptr, &app_id).unwrap();
        prepare_guest_buf_descriptor(&host, app_id_buf, app_id_ptr, DIGEST_SIZE as u64);

        host.borrow_memory().write(args_ptr, &args).unwrap();
        prepare_guest_buf_descriptor(&host, args_buf, args_ptr, args.len() as u64);

        write_str(&host, alias_ptr, alias);
        prepare_guest_buf_descriptor(&host, alias_buf, alias_ptr, alias.len() as u64);

        host.borrow_memory().write(ctx_ptr, &context_id).unwrap();
        prepare_guest_buf_descriptor(&host, ctx_buf, ctx_ptr, DIGEST_SIZE as u64);

        // Call ContextCreate
        host.context_create(protocol_buf, app_id_buf, args_buf, alias_buf)
            .unwrap();

        // Call ContextDelete
        host.context_delete(ctx_buf).unwrap();

        let mutations = &host.borrow_logic().context_mutations;
        assert_eq!(mutations.len(), 2);

        match &mutations[0] {
            ContextMutation::CreateContext {
                protocol: artifact_protocol,
                application_id: artifact_application_id,
                init_args: artifact_init_args,
                alias: artifact_alias,
            } => {
                assert_eq!(artifact_protocol, protocol);
                assert_eq!(artifact_application_id, &app_id);
                assert_eq!(artifact_init_args, &args);
                assert_eq!(*artifact_alias, Some(alias.to_string()));
            }
            _ => panic!("Wrong mutation type"),
        }

        match &mutations[1] {
            ContextMutation::DeleteContext { context_id: c } => {
                assert_eq!(c, &context_id);
            }
            _ => panic!("Wrong mutation type"),
        }
    }

    #[test]
    fn test_context_create_no_alias() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let mut host = logic.host_functions(store.as_store_mut());

        // Setup data
        let protocol = "near";
        let app_id = [1u8; DIGEST_SIZE];
        let args = vec![1, 2, 3];

        // Pointers & Descriptors
        let protocol_ptr = 100u64;
        let app_id_ptr = 200u64;
        let args_ptr = 300u64;

        let protocol_buf = 10u64;
        let app_id_buf = 30u64;
        let args_buf = 70u64;

        // Write memory
        write_str(&host, protocol_ptr, protocol);
        prepare_guest_buf_descriptor(&host, protocol_buf, protocol_ptr, protocol.len() as u64);

        host.borrow_memory().write(app_id_ptr, &app_id).unwrap();
        prepare_guest_buf_descriptor(&host, app_id_buf, app_id_ptr, DIGEST_SIZE as u64);

        host.borrow_memory().write(args_ptr, &args).unwrap();
        prepare_guest_buf_descriptor(&host, args_buf, args_ptr, args.len() as u64);

        // Pass 0 for alias pointer to signify None
        host.context_create(protocol_buf, app_id_buf, args_buf, 0)
            .unwrap();

        let mutations = &host.borrow_logic().context_mutations;
        match &mutations[0] {
            ContextMutation::CreateContext { alias, .. } => {
                assert!(alias.is_none());
            }
            _ => panic!("Wrong mutation type"),
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
        let host = logic.host_functions(store.as_store_mut());

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
