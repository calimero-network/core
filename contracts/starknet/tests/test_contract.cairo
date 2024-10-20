#[cfg(test)]
mod tests {
    use starknet::{ContractAddress, get_block_timestamp};
    use snforge_std::{
        declare, ContractClassTrait, DeclareResultTrait, 
        start_cheat_caller_address, stop_cheat_caller_address,
    };
    use context_config::{
        Application, Capability, Signed, ContextId, RequestKind, Request,ContextIdentity, ContextRequestKind, ContextRequest,
        IContextConfigsDispatcher, IContextConfigsDispatcherTrait,
    };
    use core::traits::Into;
    use core::array::ArrayTrait;
    use core::clone::Clone;
    use core::byte_array::ByteArray;

    fn deploy_contract(name: ByteArray) -> ContractAddress {
        let contract = declare(name).unwrap().contract_class();

        let (contract_address, _) = contract.deploy(@array![]).unwrap();
        
        contract_address
    }

    // Function to sign a message
    // fn sign_message(message: &Array<felt252>, private_key: felt252) -> (felt252, felt252) {
    //     let (r, s) = starkware.crypto.signature.signature::sign(
    //         pedersen_hash::compute(&message), 
    //         private_key
    //     );
    //     return (r, s);
    // }

    #[test]
    fn test_application() {
        // Deploy the contract
        let contract_address = deploy_contract("ContextConfig");

        // Create a dispatcher
        let dispatcher = IContextConfigsDispatcher { contract_address };

        // Create a context ID
        let context_id: ContextId = 0x1f446d0850b5779b50c1e30ead2e5609614e94fe5d5598aa5459ee73c4f3604.into();

        // Call the application function
        let application = dispatcher.application(context_id);

        // Assert that we can retrieve an application (even if it's empty)
        assert(application.id == 0, 'Unexpected application ID');
        assert(application.blob == 0, 'Unexpected application blob');
        assert(application.size == 0, 'Unexpected application size');
        assert(application.source == "", 'Unexpected application source');
        assert(application.metadata == "", 'Unexpected application metadata');
    }

    #[test]
    fn test_add_context() {
        // Deploy the contract
        let contract_address = deploy_contract("ContextConfig");
        // Create a dispatcher
        let dispatcher = IContextConfigsDispatcher { contract_address };

        // Create test data
        let context_id: ContextId = 0x1f446d0850b5779b50c1e30ead2e5609614e94fe5d5598aa5459ee73c4f3604.into();
        let author_id: ContextIdentity = 0x660ad6d4b87091520b5505433340abdd181a00856443010fa799f945d2dd5da.into();
        let application = Application {
            id: 0x11f5f7b82d573b270a053c016cd16c20e128229d757014c458e561679c42baf.into(),
            blob: 0x11f5f7b82d573b270a053c016cd16c20e128229d757014c458e561679c42baf.into(),
            size: 0,
            source: "https://calimero.io",
            metadata: "Some metadata",
        };

        // Store the application values for later comparison
        let app_id = application.id;
        let app_blob = application.blob;
        let app_size = application.size;
        let app_source = application.source.clone();
        let app_metadata = application.metadata.clone();

        // Create a signed request
        let request = Request {
            signer_id: context_id.clone(),
            timestamp_ms: get_block_timestamp(),
            kind: RequestKind::Context(
                ContextRequest {
                    context_id: context_id.clone(),
                    kind: ContextRequestKind::Add((author_id, application))
                }
            )
        };

        // Serialize the request
        let mut serialized = ArrayTrait::new();
        request.serialize(ref serialized);

        // Sign the serialized request
        // let private_key: felt252 = 0; // Replace with the actual private key
        // let (r, s) = sign_message(&serialized, private_key);
        let (r, s) = (0, 0);

        let signed_request: Signed<Request> = Signed {
            payload: serialized,
            signature: (r, s),
            public_key: 0x660ad6d4b87091520b5505433340abdd181a00856443010fa799f945d2dd5da.into(),
        };

        let context_id_felt252: felt252 = context_id.clone().into();
        let context_address: ContractAddress = context_id_felt252.try_into().unwrap();
        
        // Start cheat to simulate the contract call from the context_id address
        start_cheat_caller_address(contract_address, context_address);

        // Call the mutate function to add the context
        dispatcher.mutate(signed_request);

        // Stop cheat
        stop_cheat_caller_address(contract_address);

        // Verify that the context was added correctly
        let result_application = dispatcher.application(context_id.clone());
        assert(result_application.id == app_id, 'Incorrect application ID');
        assert(result_application.blob == app_blob, 'Incorrect application blob'); // Clone here
        assert(result_application.size == app_size, 'Incorrect application size');
        assert(result_application.source == app_source, 'Incorrect application source'); // Clone here
        assert(result_application.metadata == app_metadata, 'Incorrect application metadata'); // Clone here

        // Verify that the author is a member of the context
        let members = dispatcher.members(context_id, 1, 2);
        assert(members.len() == 1, 'Incorrect number of members');
        assert(*members.at(0) == author_id, 'Incorrect author ID');

        // Verify that the author has the correct privileges
        let privileges = dispatcher.privileges(context_id, array![author_id]);
        let (identity, capabilities) = privileges.at(0);
        assert!(identity == @author_id, "Author ID does not match expected identity");
        assert!(capabilities.len() > 0, "Expected capabilities for the author");

        // Check specific capabilities
        let expected_capabilities = array![Capability::ManageApplication, Capability::ManageMembers];
        for expected_capability in expected_capabilities {
            let mut found = false;
            for k in 0..capabilities.len() {
                if capabilities.at(k) == @expected_capability {
                    found = true;
                    break;
                }
            };
            assert!(found, "Expected capability not found: {:?}", expected_capability);
        }
    }
}
