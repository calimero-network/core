//! The policy parameter of `PermissionedStorage<T, A>` must be an `Authorizer`.

use calimero_sdk::app;
use calimero_storage::collections::{LwwRegister, PermissionedStorage};

struct NotAnAuthorizer;

#[app::state]
struct S {
    data: PermissionedStorage<LwwRegister<String>, NotAnAuthorizer>,
}

#[app::logic]
impl S {
    #[app::init]
    pub fn init() -> S {
        unimplemented!()
    }
}

fn main() {}
