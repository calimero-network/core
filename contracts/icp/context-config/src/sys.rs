use crate::CONTEXT_CONFIGS;

#[ic_cdk::update]
pub fn set_proxy_code(proxy_code: Vec<u8>) -> Result<(), String> {
    CONTEXT_CONFIGS.with(|configs| {
        let mut configs = configs.borrow_mut();

        // Check if caller is the owner
        if ic_cdk::api::caller() != configs.owner {
            return Err("Unauthorized: only owner can set proxy code".to_string());
        }

        configs.proxy_code = Some(proxy_code);
        Ok(())
    })
}
