use std::str::from_utf8;

use calimero_server_primitives::admin::JwtTokenRequest;
use calimero_store::Store;
use chrono::{Duration, Utc};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};

use crate::admin::service::ApiError;
use crate::admin::storage::jwt_token::{get_refresh_token, insert_or_update_refresh_token};

#[derive(Debug, Serialize, Deserialize)]
struct Claims {
    context_id: String,
    exec_pub_key: String,
    exp: usize,
    token_type: String,
}

#[derive(Debug, Serialize)]
pub struct JwtToken {
    pub access_token: String,
    pub refresh_token: String,
}

pub fn generate_jwt_tokens(req: JwtTokenRequest, store: Store, jwt_secret: Vec<u8>) -> Result<JwtToken, ApiError> {

    let context_id = req.context_id;
    let executor_public_key = req.executor_public_key;
    // Generate Access Token
    let access_expiration = Utc::now() + Duration::hours(1);
    let access_claims = Claims {
        context_id: context_id.to_string(),
        exec_pub_key: executor_public_key.to_string(),
        exp: access_expiration.timestamp() as usize,
        token_type: "access".to_string(),
    };

    let access_token = encode(
        &Header::default(),
        &access_claims,
        &EncodingKey::from_secret(jwt_secret.as_slice()),
    )
    .map_err(|err| ApiError {
        status_code: StatusCode::BAD_REQUEST,
        message: format!("Failed to generate access token: {}", err),
    })?;

    // Generate Refresh Token
    let refresh_expiration = Utc::now() + Duration::days(30);
    let refresh_claims = Claims {
        context_id: context_id.to_string(),
        exec_pub_key: executor_public_key.to_string(),
        exp: refresh_expiration.timestamp() as usize,
        token_type: "refresh".to_string(),
    };

    let refresh_token = encode(
        &Header::default(),
        &refresh_claims,
        &EncodingKey::from_secret(jwt_secret.as_slice()),
    )
    .map_err(|err| ApiError {
        status_code: StatusCode::BAD_REQUEST,
        message: format!("Failed to generate refresh token: {}", err),
    })?;

    // Store the refresh token in the database
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

// Check if the refresh token is valid and generate new tokens
pub fn refresh_access_token(refresh_token: &str, store: Store, jwt_secret: Vec<u8>) -> Result<JwtToken, ApiError> {
    // Check if the refresh token from the database is present
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
        &DecodingKey::from_secret(jwt_secret.as_slice()),
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

    let context_id = token_data.claims.context_id.clone();
    let exec_pub_key = token_data.claims.exec_pub_key.clone();

    // Generate new Access Token
    let access_expiration = Utc::now() + Duration::hours(1);
    let access_claims = Claims {
        context_id: context_id.clone(),
        exec_pub_key: exec_pub_key.clone(),
        exp: access_expiration.timestamp() as usize,
        token_type: "access".to_string(),
    };

    let access_token = encode(
        &Header::default(),
        &access_claims,
        &EncodingKey::from_secret(jwt_secret.as_slice()),
    )
    .map_err(|err| ApiError {
        status_code: StatusCode::BAD_REQUEST,
        message: format!("Failed to generate access token: {}", err),
    })?;

    // Generate new Refresh Token
    let refresh_expiration = Utc::now() + Duration::days(30);
    let refresh_claims = Claims {
        context_id: context_id.clone(),
        exec_pub_key: exec_pub_key.clone(),
        exp: refresh_expiration.timestamp() as usize,
        token_type: "refresh".to_string(),
    };

    let new_refresh_token = encode(
        &Header::default(),
        &refresh_claims,
        &EncodingKey::from_secret(jwt_secret.as_slice()),
    )
    .map_err(|err| ApiError {
        status_code: StatusCode::BAD_REQUEST,
        message: format!("Failed to generate new refresh token: {}", err),
    })?;

    // Store the refresh token in the database
    insert_or_update_refresh_token(store.clone(), refresh_token.as_bytes().to_vec()).map_err(
        |err| ApiError {
            status_code: StatusCode::INTERNAL_SERVER_ERROR,
            message: format!("Failed to store refresh token: {}", err),
        },
    )?;

    Ok(JwtToken {
        access_token,
        refresh_token: new_refresh_token,
    })
}
