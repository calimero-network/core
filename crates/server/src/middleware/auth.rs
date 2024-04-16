use std::convert::Infallible;
use std::task::{Context, Poll};

use axum::body::Body;
use axum::extract::Request;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use calimero_identity::auth::verify_client_key;
use calimero_store::Store;
use libp2p::futures::future::BoxFuture;
use tower::{Layer, Service};

use crate::admin::handlers::add_client_key::WalletType;
use crate::admin::storage::client_keys::{exists_client_key, ClientKey};

#[derive(Clone)]
pub struct AuthSignatureLayer {
    store: Store,
}

impl AuthSignatureLayer {
    pub fn new(store: Store) -> Self {
        Self { store }
    }
}

impl<S> Layer<S> for AuthSignatureLayer {
    type Service = AuthSignatureMiddleware<S>;

    fn layer(&self, inner: S) -> Self::Service {
        AuthSignatureMiddleware {
            inner,
            store: self.store.clone(),
        }
    }
}

#[derive(Clone)]
pub struct AuthSignatureMiddleware<S> {
    inner: S,
    store: Store,
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
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: Request<Body>) -> Self::Future {
        let result = auth(request.headers(), &self.store);

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

#[derive(Debug)]
struct AuthHeaders {
    wallet_type: WalletType,
    signing_key: String,
    signature: Vec<u8>,
    challenge: Vec<u8>,
}

pub fn auth<'a>(
    // run the `HeaderMap` extractor
    headers: &'a HeaderMap,
    store: &'a Store,
) -> Result<(), UnauthorizedError<'a>> {
    let auth_headers = get_auth_headers(headers)
        .map_err(|_| UnauthorizedError::new("Failed to extract authentication headers."))?;

    let client_key = ClientKey {
        wallet_type: auth_headers.wallet_type.clone(),
        signing_key: auth_headers.signing_key.clone(),
    };
    let key_exists = exists_client_key(store, &client_key)
        .map_err(|_| UnauthorizedError::new("Issue during extracting client key"))?;

    if !key_exists {
        return Err(UnauthorizedError::new("Client key does not exist."));
    }

    if verify_client_key(
        auth_headers.signing_key.as_str(),
        auth_headers.challenge.as_slice(),
        auth_headers.signature.as_slice(),
    ) {
        Ok(())
    } else {
        Err(UnauthorizedError::new(
            "Invalid signature for provided key.",
        ))
    }
}

fn get_auth_headers(headers: &HeaderMap) -> Result<AuthHeaders, UnauthorizedError> {
    println!("{:?}", headers);

    let signing_key = headers
        .get("signing_key")
        .ok_or_else(|| UnauthorizedError::new("Missing signing_key header"))?;
    let signing_key = String::from_utf8(signing_key.as_bytes().to_vec())
        .map_err(|_| UnauthorizedError::new("Invalid signing_key string"))?;

    let wallet_type = headers
        .get("wallet_type")
        .ok_or_else(|| UnauthorizedError::new("Missing wallet_type header"))?;
    let wallet_type = String::from_utf8(wallet_type.as_bytes().to_vec())
        .map_err(|_| UnauthorizedError::new("Invalid wallet_type string"))?;
    let wallet_type = WalletType::from_str(&wallet_type)
        .map_err(|_| UnauthorizedError::new("Invalid wallet_type string"))?;

    let signature = headers
        .get("signature")
        .ok_or_else(|| UnauthorizedError::new("Missing signature header"))?;
    let signature = bs58::decode(signature)
        .into_vec()
        .map_err(|_| UnauthorizedError::new("Invalid base58 signature"))?;

    let challenge = headers
        .get("challenge")
        .ok_or_else(|| UnauthorizedError::new("Missing challenge header"))?;
    let challenge = bs58::decode(challenge)
        .into_vec()
        .map_err(|_| UnauthorizedError::new("Invalid base58 challenge"))?;

    let auth = AuthHeaders {
        wallet_type,
        signing_key,
        signature,
        challenge,
    };
    Ok(auth)
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
        f.pad(self.reason)
    }
}

impl std::error::Error for UnauthorizedError<'_> {}

impl IntoResponse for UnauthorizedError<'_> {
    fn into_response(self) -> Response<Body> {
        (StatusCode::UNAUTHORIZED, self.reason.to_string()).into_response()
    }
}
