extern crate context_contract;

use context_contract::types::{
    ICSignerId, ICContextId, ICApplication, ICApplicationId, 
    ICBlobId, Request, RequestKind, ContextRequest, ContextRequestKind, ICPSigned
};
use context_contract::CONTEXT_CONFIGS;
use context_contract::mutate::mutate;

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_application() -> ICApplication {
        ICApplication {
            id: ICApplicationId::new([0; 32]),
            blob: ICBlobId::new([0; 32]),
            size: 100,
            source: "test_source".to_string(),
            metadata: vec![],
        }
    }

    fn create_test_request(
        signer_id: ICSignerId,
        context_id: ICContextId,
        author_id: ICSignerId,
    ) -> ICPSigned<Request> {
        let request = Request::new(
            signer_id,
            RequestKind::Context(ContextRequest {
                context_id,
                kind: ContextRequestKind::Add {
                    author_id,
                    application: create_test_application(),
                },
            }),
        );

        ICPSigned {
            payload: request,
            signature: vec![0; 64],
        }
    }

    #[test]
    fn test_add_context() {
        let context_id = ICContextId::new([1; 32]);
        let author_id = ICSignerId::new([1; 32]);
        let signer_id = ICSignerId::new([1; 32]);

        let request = create_test_request(signer_id, context_id.clone(), author_id);
        assert!(mutate(request).is_ok());

        CONTEXT_CONFIGS.with(|configs| {
            let configs = configs.borrow();
            assert!(configs.contexts.contains_key(&context_id));
        });
    }

    #[test]
    fn test_unauthorized_add() {
        let context_id = ICContextId::new([1; 32]);
        let author_id = ICSignerId::new([1; 32]);
        let signer_id = ICSignerId::new([2; 32]);

        let request = create_test_request(signer_id, context_id, author_id);
        assert!(mutate(request).is_err());
    }

    #[test]
    fn test_duplicate_context() {
        let context_id = ICContextId::new([1; 32]);
        let author_id = ICSignerId::new([1; 32]);
        let signer_id = ICSignerId::new([1; 32]);

        let request = create_test_request(signer_id, context_id.clone(), author_id.clone());
        assert!(mutate(request).is_ok());

        let request = create_test_request(signer_id, context_id, author_id);
        assert!(mutate(request).is_err());
    }
}