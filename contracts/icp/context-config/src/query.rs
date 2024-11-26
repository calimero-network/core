use candid::Principal;
use ic_cdk_macros::query;
use crate::types::*;
use crate::{Context, ContextConfigs, CONTEXT_CONFIGS};

type QueryResult<T> = Result<T, &'static str>;

#[query]
fn application(context_id: CandidRepr<CandidContextId>) -> QueryResult<&'static CandidApplication<'static>> {
    CONTEXT_CONFIGS.with(|configs| {
        let configs = configs.borrow();
        let context = configs
            .contexts
            .get(&context_id.0)
            .ok_or("context does not exist")?;

        Ok(&context.application.inner)
    })
}

#[query]
fn application_revision(context_id: CandidRepr<CandidContextId>) -> QueryResult<u64> {
    CONTEXT_CONFIGS.with(|configs| {
        let configs = configs.borrow();
        let context = configs
            .contexts
            .get(&context_id.0)
            .ok_or("context does not exist")?;

        Ok(context.application.revision)
    })
}

#[query]
fn proxy_contract(context_id: CandidRepr<CandidContextId>) -> QueryResult<Principal> {
    CONTEXT_CONFIGS.with(|configs| {
        let configs = configs.borrow();
        let context = configs
            .contexts
            .get(&context_id.0)
            .ok_or("context does not exist")?;

        Ok(*context.proxy.inner)
    })
}

#[query]
fn members(
    context_id: CandidRepr<CandidContextId>,
    offset: usize,
    length: usize,
) -> QueryResult<Vec<CandidContextIdentity>> {
    CONTEXT_CONFIGS.with(|configs| {
        let configs = configs.borrow();
        let context = configs
            .contexts
            .get(&context_id.0)
            .ok_or("context does not exist")?;

        let members = &context.members.inner;
        let start = offset.min(members.len());
        let end = (offset + length).min(members.len());

        Ok(members[start..end].to_vec())
    })
}

#[query]
fn has_member(context_id: CandidRepr<CandidContextId>, identity: CandidRepr<CandidContextIdentity>) -> QueryResult<bool> {
    CONTEXT_CONFIGS.with(|configs| {
        let configs = configs.borrow();
        let context = configs
            .contexts
            .get(&context_id.0)
            .ok_or("context does not exist")?;

        Ok(context.members.inner.contains(&identity.0))
    })
}

#[query]
fn members_revision(context_id: CandidRepr<CandidContextId>) -> QueryResult<u64> {
    CONTEXT_CONFIGS.with(|configs| {
        let configs = configs.borrow();
        let context = configs
            .contexts
            .get(&context_id.0)
            .ok_or("context does not exist")?;

        Ok(context.members.revision)
    })
}

#[query]
fn privileges(
    context_id: CandidRepr<CandidContextId>,
    identities: Vec<CandidRepr<CandidContextIdentity>>,
) -> QueryResult<BTreeMap<CandidSignerId, Vec<CandidCapability>>> {
    CONTEXT_CONFIGS.with(|configs| {
        let configs = configs.borrow();
        let context = configs
            .contexts
            .get(&context_id.0)
            .ok_or("context does not exist")?;

        let mut privileges = BTreeMap::<_, Vec<_>>::new();

        let application_privileges = context.application.privileged();
        let member_privileges = context.members.privileged();

        if identities.is_empty() {
            for signer_id in application_privileges {
                privileges
                    .entry(CandidSignerId::new(signer_id))
                    .or_default()
                    .push(CandidCapability::ManageApplication);
            }

            for signer_id in member_privileges {
                privileges
                    .entry(CandidSignerId::new(signer_id))
                    .or_default()
                    .push(CandidCapability::ManageMembers);
            }
        } else {
            for identity in identities {
                let signer_id = identity.rt().expect("infallible conversion");

                let entry = privileges.entry(signer_id).or_default();

                if application_privileges.contains(&signer_id) {
                    entry.push(CandidCapability::ManageApplication);
                }

                if member_privileges.contains(&signer_id) {
                    entry.push(CandidCapability::ManageMembers);
                }
            }
        }

        Ok(privileges)
    })
}