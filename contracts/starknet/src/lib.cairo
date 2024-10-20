pub mod types;
pub mod context_config_interface;

// Export the types and interfaces
pub use types::{Application, Context, Config, Capability, Signed, ContextId, RequestKind, Request, ContextIdentity, ContextRequestKind, ContextRequest};
pub use context_config_interface::{IContextConfigs, IContextConfigsDispatcher, IContextConfigsDispatcherTrait};

#[starknet::contract]
pub mod ContextConfig {
    use starknet::get_block_timestamp;
    use core::pedersen::pedersen;
    use core::ecdsa::check_ecdsa_signature;
    use starknet::storage::{Map, StoragePointerReadAccess, StoragePointerWriteAccess};
    use context_config::types::{Application, Context, Config, Capability, Signed, ContextId, RequestKind, Request, ContextIdentity, ContextRequestKind};

    #[storage]
    struct Storage {
        contexts: Map::<ContextId, Context>,
        config: Config,
        privileges: Map::<felt252, bool>,
        context_members: Map::<(ContextId, u32), ContextIdentity>,
        context_member_indices: Map::<(ContextId, ContextIdentity), u32>,
    }

    const DEFAULT_VALIDITY_THRESHOLD_MS: u64 = 10_000;
    const MIN_VALIDITY_THRESHOLD_MS: u64 = 5_000;

    #[constructor]
    fn constructor(ref self: ContractState) {
        self.config.write(Config { validity_threshold_ms: DEFAULT_VALIDITY_THRESHOLD_MS });
    }

    #[abi(embed_v0)]
    impl ContextConfigsImpl of super::context_config_interface::IContextConfigs<ContractState> {

        fn application(self: @ContractState, context_id: ContextId) -> Application {
            // Read the context from storage
            let context = self.contexts.read(context_id);
            
            // Return the application associated with this context
            context.application
        }

        fn members(self: @ContractState, context_id: ContextId, offset: u32, length: u32) -> Array<ContextIdentity> {
            // Read the context from storage
            let context = self.contexts.read(context_id);
            
            // Initialize an array to store members
            let mut members = ArrayTrait::new();
            
            // Get the total number of members
            let total_members = context.member_count;
            
            // Calculate the end index, ensuring we don't exceed the total members
            let end = min(offset + length, total_members);
            
            // Iterate through the members, starting from offset + 1 (to account for 1-based indexing)
            let mut i = offset + 1; // Start from offset + 1
            loop {
                if i > end {
                    break;
                }
                let member = self.context_members.read((context_id, i));
                members.append(member);
                i += 1;
            };
            
            members
        }

        fn privileges(self: @ContractState, context_id: ContextId, identities: Array<ContextIdentity>) -> Array<(ContextIdentity, Array<Capability>)> {
            let mut result = ArrayTrait::new();
            let context = self.contexts.read(context_id);

            if identities.len() == 0 {
                // Return privileges for all members
                let mut i: u32 = 0;
                loop {
                    if i >= context.member_count {
                        break;
                    }
                    let member = self.context_members.read((context_id, i));
                    let capabilities = self.get_member_capabilities(context_id, member);
                    if capabilities.len() > 0 {
                        result.append((member, capabilities));
                    }
                    i += 1;
                };
            } else {
                // Return privileges for specified identities
                let mut i = 0;
                loop {
                    if i >= identities.len() {
                        break;
                    }
                    let identity = *identities.at(i);
                    let capabilities = self.get_member_capabilities(context_id, identity);
                    if capabilities.len() > 0 {
                        result.append((identity, capabilities));
                    }
                    i += 1;
                };
            }

            result
        }
        
        fn mutate(ref self: ContractState, signed_request: Signed<Request>) {
            // assert(self.verify_signature(@signed_request), 'Invalid signature');
        
            // Deserialize the payload
            let mut serialized = signed_request.payload.span();
            let request: Request = Serde::deserialize(ref serialized).unwrap();

            let current_timestamp = get_block_timestamp();
            let config = self.config.read();
            assert(
                current_timestamp - request.timestamp_ms <= config.validity_threshold_ms,
                'Request expired'
            );
        
            match request.kind {
                RequestKind::Context(context_request) => {
                    match context_request.kind {
                        ContextRequestKind::Add((author_id, application)) => {
                            self.add_context(request.signer_id, context_request.context_id, author_id, application);
                        },
                        ContextRequestKind::UpdateApplication(application) => {
                            self.update_application(request.signer_id, context_request.context_id, application);
                        },
                        ContextRequestKind::AddMembers(members) => {
                            self.add_members(request.signer_id, context_request.context_id, members);
                        },
                        ContextRequestKind::RemoveMembers(members) => {
                            self.remove_members(request.signer_id, context_request.context_id, members);
                        },
                        ContextRequestKind::Grant(capabilities) => {
                            self.grant(request.signer_id, context_request.context_id, capabilities);
                        },
                        ContextRequestKind::Revoke(capabilities) => {
                            self.revoke(request.signer_id, context_request.context_id, capabilities);
                        },
                    }
                },
            }
        }
    }

    fn min(a: u32, b: u32) -> u32 {
        if a < b { a } else { b }
    }

    #[generate_trait]
    impl SignatureVerifier of SignatureVerifierTrait {
        fn verify_signature(self: @ContractState, signed_request: @Signed<Request>) -> bool {
            // Hash the payload using Pedersen hash
            let mut hash = 0;
            let mut span = signed_request.payload.span();
            loop {
                match span.pop_front() {
                    Option::Some(byte) => {
                        hash = pedersen(hash, (*byte).into());
                    },
                    Option::None => { break; },
                }
            };
    
            // Verify the signature
            let (signature_r, signature_s) = signed_request.signature;
            check_ecdsa_signature(
                hash,  // message hash
                *signed_request.public_key,  // public key
                *signature_r,  // r
                *signature_s   // s
            )
        }
    }

    #[generate_trait]
    impl ContextHelpers of ContextHelpersTrait {
        fn add_context(
            ref self: ContractState,
            signer_id: ContextIdentity,
            context_id: ContextIdentity,
            author_id: ContextIdentity,
            application: Application
        ) {
            // Verify signer
            assert(signer_id == context_id, 'Invalid signer');
        
            // Check if context already exists
            let existing_context = self.contexts.read(context_id);
            assert(existing_context.member_count == 0, 'Context already exists');
        
            // Create new context
            let new_context = Context {
                application: application,
                member_count: 1, // Start with one member (author)
            };
            self.contexts.write(context_id, new_context);
        
            // Add author as first member
            self.add_member(context_id, author_id);
        
            // Grant initial privileges to author
            self.grant_privilege(context_id, author_id, Capability::ManageApplication);
            self.grant_privilege(context_id, author_id, Capability::ManageMembers);
        
            // Log context creation (you'd need to implement logging for Starknet)
            // self.emit(ContextCreated { context_id: context_id });
        }

        fn add_member(
            ref self: ContractState,
            context_id: felt252,
            member_id: ContextIdentity
        ) {
            // Read the current context
            let mut context = self.contexts.read(context_id);
            
            // Check if the member already exists
            let existing_index = self.context_member_indices.read((context_id, member_id));
            assert(existing_index == 0, 'Member already exists');
    
            // Add the new member
            let new_index = context.member_count + 1;
            self.context_members.write((context_id, new_index), member_id);
            self.context_member_indices.write((context_id, member_id), new_index);
    
            // Update the member count
            context.member_count += 1;
            self.contexts.write(context_id, context);
    
            // Optionally, emit an event
            // self.emit(MemberAdded { context_id: context_id, member_id: member_id });
        }

        fn update_application(
            ref self: ContractState,
            signer_id: ContextIdentity,
            context_id: felt252,
            new_application: Application
        ) {
            // Check if the context exists
            let mut context = self.contexts.read(context_id);
            assert(context.member_count > 0, 'Context does not exist');
    
            // Check if the signer has the necessary permissions
            assert(
                self.has_privilege(context_id, signer_id, Capability::ManageApplication), 
                'Unauthorized'
            );
    
            // Store the old application ID for logging
            // let old_application_id = context.application.id;
    
            // Update the context's application
            context.application = new_application;
            self.contexts.write(context_id, context);
    
            // // Log the update (Note: Starknet doesn't have built-in logging, so we'll use events)
            // self.emit(ApplicationUpdated {
            //     context_id: context_id,
            //     old_application_id: old_application_id,
            //     new_application_id: new_application.id
            // });
        }

        fn add_members(
            ref self: ContractState,
            signer_id: ContextIdentity,
            context_id: felt252,
            members: Array<ContextIdentity>
        ) {
            // Check if the context exists
            let mut context = self.contexts.read(context_id);
            assert(context.member_count > 0, 'Context does not exist');
    
            // Check if the signer has the necessary permissions
            assert(
                self.has_privilege(context_id, signer_id, Capability::ManageMembers), 
                'Unauthorized'
            );
    
            // Add members
            let mut i: u32 = 0;
            loop {
                match members.get(i.into()) {
                    Option::Some(member) => {
                        let member = *member.unbox();
                        // Check if the member already exists
                        let existing_index = self.context_member_indices.read((context_id, member));
                        if existing_index == 0 {
                            // Add the new member
                            let new_index = context.member_count + 1;
                            self.context_members.write((context_id, new_index), member);
                            self.context_member_indices.write((context_id, member), new_index);
                            
                            // Update the member count
                            context.member_count += 1;
                            
                            // Emit an event for the added member
                            // self.emit(MemberAdded { context_id: context_id, member_id: member });
                        }
                        i += 1;
                    },
                    Option::None => { break; },
                }
            };
    
            // Update the context with the new member count
            self.contexts.write(context_id, context);
        }

        fn remove_members(
            ref self: ContractState,
            signer_id: ContextIdentity,
            context_id: felt252,
            members: Array<ContextIdentity>
        ) {
            // Check if the context exists
            let mut context = self.contexts.read(context_id);
            assert(context.member_count > 0, 'Context does not exist');
        
            // Check if the signer has the necessary permissions
            assert(
                self.has_privilege(context_id, signer_id, Capability::ManageMembers), 
                'Unauthorized'
            );
        
            // Remove members
            let mut i: u32 = 0;
            loop {
                match members.get(i) {
                    Option::Some(member_box) => {
                        let member = *member_box.unbox();
                        // Check if the member exists
                        let member_index = self.context_member_indices.read((context_id, member));
                        if member_index != 0 {
                            // Remove the member
                            self.context_member_indices.write((context_id, member), 0);
                            
                            // Decrease the member count
                            context.member_count -= 1;
                            
                            // Revoke all privileges for this member in this context
                            self.revoke_all_privileges(context_id, member);
                            
                            // Emit an event for the removed member
                            // self.emit(MemberRemoved { context_id: context_id, member_id: member });
                        }
                        i += 1;
                    },
                    Option::None => { break; },
                }
            };
        
            // Update the context with the new member count
            self.contexts.write(context_id, context);
        }

        fn grant(
            ref self: ContractState,
            signer_id: ContextIdentity,
            context_id: felt252,
            capabilities: Array<(ContextIdentity, Capability)>
        ) {
            // Check if the context exists
            let context = self.contexts.read(context_id);
            assert(context.member_count > 0, 'Context does not exist');
    
            // Check if the signer has the necessary permissions
            assert(
                self.has_privilege(context_id, signer_id, Capability::ManageMembers), 
                'Unauthorized'
            );
    
            // Grant capabilities
            let mut i: u32 = 0;
            loop {
                match capabilities.get(i) {
                    Option::Some(capability_box) => {
                        let (identity, capability) = *capability_box.unbox();
                        
                        // Check if the identity is a member of the context
                        let member_index = self.context_member_indices.read((context_id, identity));
                        assert(member_index != 0, 'Not a member of the context');
    
                        // Grant the capability
                        self.grant_privilege(context_id, identity, capability);
    
                        // Emit an event for the granted capability
                        // self.emit(CapabilityGranted { 
                        //     context_id: context_id, 
                        //     member_id: identity, 
                        //     capability: capability 
                        // });
    
                        i += 1;
                    },
                    Option::None => { break; },
                }
            };
        }

        fn revoke(
            ref self: ContractState,
            signer_id: ContextIdentity,
            context_id: felt252,
            capabilities: Array<(ContextIdentity, Capability)>
        ) {
            // Check if the context exists
            let context = self.contexts.read(context_id);
            assert(context.member_count > 0, 'Context does not exist');
    
            // Check if the signer has the necessary permissions
            assert(
                self.has_privilege(context_id, signer_id, Capability::ManageMembers), 
                'Unauthorized'
            );
    
            // Revoke capabilities
            let mut i: u32 = 0;
            loop {
                match capabilities.get(i) {
                    Option::Some(capability_box) => {
                        let (identity, capability) = *capability_box.unbox();
                        
                        // Revoke the capability
                        self.revoke_privilege(context_id, identity, capability);
    
                        // Emit an event for the revoked capability
                        // self.emit(CapabilityRevoked { 
                        //     context_id: context_id, 
                        //     member_id: identity, 
                        //     capability: capability 
                        // });
    
                        i += 1;
                    },
                    Option::None => { break; },
                }
            };
        }
    }

    #[generate_trait]
    impl PrivilegeHelpers of PrivilegeHelpersTrait {
        fn has_privilege(
            self: @ContractState,
            context_id: felt252,
            member_id: ContextIdentity,
            capability: Capability
        ) -> bool {
            let privilege_key = self.create_privilege_key(context_id, member_id, capability);
            self.privileges.read(privilege_key)
        }

        fn get_member_capabilities(self: @ContractState, context_id: ContextId, member: ContextIdentity) -> Array<Capability> {
            let mut capabilities = ArrayTrait::new();
            if self.has_privilege(context_id, member, Capability::ManageApplication) {
                capabilities.append(Capability::ManageApplication);
            }
            if self.has_privilege(context_id, member, Capability::ManageMembers) {
                capabilities.append(Capability::ManageMembers);
            }
            capabilities
        }

        fn grant_privilege(
            ref self: ContractState,
            context_id: felt252,
            member_id: ContextIdentity,
            capability: Capability
        ) {
            let privilege_key = self.create_privilege_key(context_id, member_id, capability);
            self.privileges.write(privilege_key, true);
        }

        fn revoke_privilege(
            ref self: ContractState,
            context_id: felt252,
            member_id: ContextIdentity,
            capability: Capability
        ) {
            let privilege_key = self.create_privilege_key(context_id, member_id, capability);
            self.privileges.write(privilege_key, false);
        }

        fn create_privilege_key(
            self: @ContractState,
            context_id: felt252,
            member_id: ContextIdentity,
            capability: Capability
        ) -> felt252 {
            // This is a simplified way to create a unique key. In a real implementation,
            // you might want to use a more sophisticated hashing method.
            let capability_felt: felt252 = capability.into();
            context_id + member_id + capability_felt
        }

        fn revoke_all_privileges(
            ref self: ContractState,
            context_id: felt252,
            member_id: ContextIdentity
        ) {
            // Revoke ManageMembers privilege
            self.revoke_privilege(context_id, member_id, Capability::ManageMembers);
            
            // Revoke ManageApplication privilege
            self.revoke_privilege(context_id, member_id, Capability::ManageApplication);
        }
    }
}
