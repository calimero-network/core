use std::{
    convert::Infallible,
    task::{Context, Poll},
};

use axum::{
    body::Body,
    extract::Request,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};

use calimero_identity::auth::verify_peer_auth;
use libp2p::{futures::future::BoxFuture, identity::Keypair};
use tower::{Layer, Service};

#[derive(Clone)]
pub struct AuthSignatureLayer {
    keypair: Keypair,
}

impl AuthSignatureLayer {
    pub fn new(keypair: Keypair) -> Self {
        Self { keypair }
    }
}

impl<S> Layer<S> for AuthSignatureLayer {
    type Service = AuthSignatureMiddleware<S>;

    fn layer(&self, inner: S) -> Self::Service {
        AuthSignatureMiddleware {
            inner,
            keypair: self.keypair.clone(),
        }
    }
}

#[derive(Clone)]
pub struct AuthSignatureMiddleware<S> {
    inner: S,
    keypair: Keypair,
}

impl<S> Service<Request<Body>> for AuthSignatureMiddleware<S>
where
    S: Service<Request<Body>, Response = Response<Body>, Error = Infallible> + Send + 'static,
    S::Future: Send + 'static,
{
    type Response = S::Response;
    type Error = Infallible;
    // `BoxFuture` is a type alias for `Pin<Box<dyn Future + Send + 'a>>`
    type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        // self.inner.poll_ready(cx)
        self.inner.poll_ready(cx).map_err(|e| e.into())
    }

    fn call(&mut self, request: Request<Body>) -> Self::Future {
        let result = auth(request.headers(), &self.keypair);

        if let Err(err) = result {
            let error_response = err.into_response();
            return Box::pin(async move { Ok(error_response) });
        }

        let future = self.inner.call(request);

        Box::pin(async move {
            let response: Response = future.await?;
            Ok(response)
        })
    }
}

struct AuthHeaders {
    signature: Vec<u8>,
    challenge: Vec<u8>,
}

pub fn auth<'a>(
    // run the `HeaderMap` extractor
    headers: &'a HeaderMap,
    keypair: &'a Keypair,
) -> Result<(), UnauthorizedError<'a>> {
    match get_auth_headers(&headers) {
        Ok(auth_headers) if token_is_valid(&keypair, &auth_headers) => Ok(()),
        Ok(_) => Err(UnauthorizedError::new("Keypair not matching signature.")),
        Err(error) => Err(error),
    }
}

fn get_auth_headers(headers: &HeaderMap) -> Result<AuthHeaders, UnauthorizedError> {
    let signature = headers.get("signature");
    if signature.is_none() {
        return Err(UnauthorizedError::new("Missing signature header"));
    }
    let signature = signature.unwrap().to_str();
    if signature.is_err() {
        return Err(UnauthorizedError::new("Cannot unwrap signature"));
    }
    let signature = bs58::decode(signature.unwrap()).into_vec();
    if signature.is_err() {
        return Err(UnauthorizedError::new("Invalid base58"));
    }
    let signature = signature.unwrap();

    let challenge = headers.get("challenge");
    if challenge.is_none() {
        return Err(UnauthorizedError::new("Missing challenge header"));
    }
    let challenge = challenge.unwrap().to_str();
    if challenge.is_err() {
        return Err(UnauthorizedError::new("Cannot unwrap challenge"));
    }
    let challenge = bs58::decode(challenge.unwrap()).into_vec();
    if challenge.is_err() {
        return Err(UnauthorizedError::new("Invalid base58"));
    }
    let challenge = challenge.unwrap();

    let auth = AuthHeaders { signature, challenge };
    Ok(auth)
}

fn token_is_valid(keypair: &Keypair, auth_headers: &AuthHeaders) -> bool {
    let verify_result = verify_peer_auth(
        keypair,
        auth_headers.challenge.as_slice(),
        auth_headers.signature.as_slice(),
    );
    if verify_result.is_err() {
        return false;
    }
    verify_result.unwrap()
}

#[derive(Debug)]
pub struct UnauthorizedError<'a> {
    reason: &'a str,
}

impl<'a> UnauthorizedError<'a> {
    pub fn new(reason: &'a str) -> Self {
        Self { reason }
    }
}

impl std::fmt::Display for UnauthorizedError<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.reason)
    }
}

impl std::error::Error for UnauthorizedError<'_> {}

impl IntoResponse for UnauthorizedError<'_> {
    fn into_response(self) -> Response<Body> {
        let response = Response::builder()
            .status(StatusCode::UNAUTHORIZED)
            .body(Body::from(self.reason.to_string()))
            .unwrap();
        response
    }
}
