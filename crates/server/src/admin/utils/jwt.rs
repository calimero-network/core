use std::str::from_utf8;

use axum::response::IntoResponse;
use calimero_store::Store;
use chrono::{Duration, Utc};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};

use crate::admin::service::ApiError;
use crate::admin::storage::jwt::{get_refresh_token, insert_or_update_refresh_token};

const JWT_SECRET: &[u8] = b"b2a1f78d02fca157b31315fe46da060d4fc4f98f10248bf2679406037c017c80";

#[derive(Debug, Serialize, Deserialize)]
struct Claims {
    sub: String,
    context_id: String,
    exp: usize,
    token_type: String,
}

#[derive(Debug, Serialize)]
pub struct JwtToken {
    pub access_token: String,
    pub refresh_token: String,
}

pub fn generate_jwt_tokens(
    client_id: &str,
    context_id: &str,
    store: Store,
) -> Result<JwtToken, ApiError> {
    // Generate Access Token
    let access_expiration = Utc::now() + Duration::hours(1);
    let access_claims = Claims {
        sub: client_id.to_string(),
        context_id: context_id.to_string(),
        exp: access_expiration.timestamp() as usize,
        token_type: "access".to_string(),
    };

    let access_token = encode(
        &Header::default(),
        &access_claims,
        &EncodingKey::from_secret(JWT_SECRET),
    )
    .map_err(|err| ApiError {
        status_code: StatusCode::BAD_REQUEST,
        message: format!("Failed to generate access token: {}", err),
    })?;

    // Generate Refresh Token
    let refresh_expiration = Utc::now() + Duration::days(30);
    let refresh_claims = Claims {
        sub: client_id.to_string(),
        context_id: context_id.to_string(),
        exp: refresh_expiration.timestamp() as usize,
        token_type: "refresh".to_string(),
    };

    let refresh_token = encode(
        &Header::default(),
        &refresh_claims,
        &EncodingKey::from_secret(JWT_SECRET),
    )
    .map_err(|err| ApiError {
        status_code: StatusCode::BAD_REQUEST,
        message: format!("Failed to generate refresh token: {}", err),
    })?;

    insert_or_update_refresh_token(store.clone(), refresh_token.as_bytes().to_vec()).map_err(
        |err| ApiError {
            status_code: StatusCode::INTERNAL_SERVER_ERROR,
            message: format!("Failed to store refresh token: {}", err),
        },
    )?;

    Ok(JwtToken {
        access_token,
        refresh_token,
    })
}

pub fn refresh_access_token(refresh_token: &str, store: Store) -> Result<JwtToken, ApiError> {
    // Check if the refresh token from the store is present and matches the provided one
    let refresh_token_db = match get_refresh_token(store.clone()) {
        Ok(Some(token)) => {
            let refresh_token = from_utf8(token.refresh_token()).map_err(|err| ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                message: format!("Failed to parse refresh token: {}", err),
            })?;
            refresh_token.to_string()
        }
        Ok(None) => {
            return Err(ApiError {
                status_code: StatusCode::FORBIDDEN,
                message: "Refresh token not found".into(),
            });
        }
        Err(err) => {
            return Err(ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                message: format!("Failed to get refresh token: {}", err),
            });
        }
    };

    // Check if the refresh token from the store matches the provided one
    if refresh_token_db != refresh_token {
        return Err(ApiError {
            status_code: StatusCode::FORBIDDEN,
            message: "Refresh token mismatch".into(),
        });
    }

    // Decode the token to check its claims
    let token_data = decode::<Claims>(
        refresh_token,
        &DecodingKey::from_secret(JWT_SECRET),
        &Validation::default(),
    )
    .map_err(|err| ApiError {
        status_code: StatusCode::FORBIDDEN,
        message: format!("Invalid refresh token: {}", err),
    })?;

    // Check if the token type is "refresh"
    if token_data.claims.token_type != "refresh" {
        return Err(ApiError {
            status_code: StatusCode::FORBIDDEN,
            message: "Invalid token type".into(),
        });
    }

    // Check if the token is expired
    let now = Utc::now().timestamp() as usize;
    if token_data.claims.exp < now {
        return Err(ApiError {
            status_code: StatusCode::FORBIDDEN,
            message: "Refresh token has expired".into(),
        });
    }

    let sub = token_data.claims.sub.clone();
    let context_id = token_data.claims.context_id.clone();

    // Generate new Access Token
    let access_expiration = Utc::now() + Duration::hours(1);
    let access_claims = Claims {
        sub: sub.clone(),
        context_id: context_id.clone(),
        exp: access_expiration.timestamp() as usize,
        token_type: "access".to_string(),
    };

    let access_token = encode(
        &Header::default(),
        &access_claims,
        &EncodingKey::from_secret(JWT_SECRET),
    )
    .map_err(|err| ApiError {
        status_code: StatusCode::BAD_REQUEST,
        message: format!("Failed to generate access token: {}", err),
    })?;

    // Generate new Refresh Token
    let refresh_expiration = Utc::now() + Duration::days(30);
    let refresh_claims = Claims {
        sub: sub.clone(),
        context_id: context_id.clone(),
        exp: refresh_expiration.timestamp() as usize,
        token_type: "refresh".to_string(),
    };

    let new_refresh_token = encode(
        &Header::default(),
        &refresh_claims,
        &EncodingKey::from_secret(JWT_SECRET),
    )
    .map_err(|err| ApiError {
        status_code: StatusCode::BAD_REQUEST,
        message: format!("Failed to generate new refresh token: {}", err),
    })?;

    // Store the new tokens in the database
    insert_or_update_refresh_token(store.clone(), new_refresh_token.clone().into_bytes());

    Ok(JwtToken {
        access_token,
        refresh_token: new_refresh_token,
    })
}
