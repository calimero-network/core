use actix_web::{error::ErrorUnauthorized, Error};
use calimero_identity::auth::verify_peer_auth;
use std::future::{ready, Ready};

use actix_web::dev::{forward_ready, Service, ServiceRequest, ServiceResponse, Transform};
use futures_util::future::LocalBoxFuture;

pub struct AuthSignature;
impl<S, B> Transform<S, ServiceRequest> for AuthSignature
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error>,
    S::Future: 'static,
    B: 'static,
{
    type Response = ServiceResponse<B>;
    type Error = Error;
    type InitError = ();
    type Transform = AuthSignatureMiddleware<S>;
    type Future = Ready<Result<Self::Transform, Self::InitError>>;

    fn new_transform(&self, service: S) -> Self::Future {
        ready(Ok(AuthSignatureMiddleware {
            service,
            verification_path: vec!["/api"],
            noverification_path: vec!["/api/auth"],
        }))
    }
}

pub struct AuthSignatureMiddleware<S> {
    service: S,
    verification_path: Vec<&'static str>,
    noverification_path: Vec<&'static str>,
}

impl<S, B> AuthSignatureMiddleware<S>
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error>,
    S::Future: 'static,
    B: 'static,
{
    fn is_need_verification(&self, path: &str) -> bool {
        self.verification_path
            .iter()
            .any(|&vp| path.starts_with(vp))
            && !self
                .noverification_path
                .iter()
                .any(|&vp| path.starts_with(vp))
    }
}

impl<S, B> Service<ServiceRequest> for AuthSignatureMiddleware<S>
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error>,
    S::Future: 'static,
    B: 'static,
{
    type Response = ServiceResponse<B>;
    type Error = Error;
    type Future = LocalBoxFuture<'static, Result<Self::Response, Self::Error>>;

    forward_ready!(service);

    fn call(&self, req: ServiceRequest) -> Self::Future {
        if self.is_need_verification(req.path()) {
            let signature = req.headers().get("signature");
            if signature.is_none() {
                return Box::pin(async { Err(ErrorUnauthorized("Missing signature header")) });
            }
            let signature = signature.unwrap().to_str();
            if signature.is_err() {
                return Box::pin(async { Err(ErrorUnauthorized("Cannot unwrap signature")) });
            }
            let signature = bs58::decode(signature.unwrap()).into_vec();
            if signature.is_err() {
                return Box::pin(async { Err(ErrorUnauthorized("Invalid base58")) });
            }
            let signature = signature.unwrap();

            let content = req.headers().get("content");
            if content.is_none() {
                return Box::pin(async { Err(ErrorUnauthorized("Missing content header")) });
            }
            let content = content.unwrap().to_str();
            if content.is_err() {
                return Box::pin(async { Err(ErrorUnauthorized("Cannot unwrap content")) });
            }
            let content = bs58::decode(content.unwrap()).into_vec();
            if content.is_err() {
                return Box::pin(async { Err(ErrorUnauthorized("Invalid base58")) });
            }
            let content = content.unwrap();

            let verify_result = verify_peer_auth(content.as_slice(), signature.as_slice());
            if verify_result.is_err() {
                return Box::pin(async { Err(ErrorUnauthorized(verify_result.err().unwrap())) });
            }
            if !verify_result.unwrap() {
                return Box::pin(async { Err(ErrorUnauthorized("Unauthorized")) });
            }
        }

        let fut = self.service.call(req);

        Box::pin(async move {
            let res = fut.await?;
            Ok(res)
        })
    }
}
