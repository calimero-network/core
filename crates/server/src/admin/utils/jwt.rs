use core::str::from_utf8;

use calimero_primitives::context::ContextId;
use calimero_primitives::hash;
use calimero_server_primitives::admin::JwtTokenRequest;
use calimero_store::Store;
use chrono::{Duration, Utc};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};

use crate::admin::service::ApiError;
use crate::admin::storage::jwt_secret::get_jwt_secret;
use crate::admin::storage::jwt_token::{
    create_refresh_token, delete_refresh_token, get_refresh_token,
};

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    context_id: ContextId,
    executor_public_key: String,
    pub exp: usize,
    token_type: TokenType,
}

#[derive(Debug, Serialize)]
#[non_exhaustive]
pub struct JwtToken {
    pub access_token: String,
    pub refresh_token: String,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
enum TokenType {
    Access,
    Refresh,
}

pub fn generate_jwt_tokens(req: JwtTokenRequest, store: &Store) -> Result<JwtToken, ApiError> {
    let jwt_secret = match get_jwt_secret(store) {
        Ok(Some(secret)) => secret.jwt_secret().to_vec(),
        Ok(None) => {
            return Err(ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                message: "JWT secret not found".into(),
            });
        }
        Err(err) => {
            return Err(ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                message: format!("Failed to get JWT secret: {err}"),
            });
        }
    };

    let context_id = req.context_id;
    let executor_public_key = req.executor_public_key;
    // Generate Access Token
    let access_expiration = Utc::now()
        .checked_add_signed(Duration::hours(1))
        .unwrap_or_else(Utc::now);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "Essentially infallible"
    )]
    let access_claims = Claims {
        context_id,
        executor_public_key: executor_public_key.clone(),
        exp: access_expiration.timestamp() as usize,
        token_type: TokenType::Access,
    };

    let access_token = encode(
        &Header::default(),
        &access_claims,
        &EncodingKey::from_secret(jwt_secret.as_slice()),
    )
    .map_err(|err| ApiError {
        status_code: StatusCode::BAD_REQUEST,
        message: format!("Failed to generate access token: {err}"),
    })?;

    // Generate Refresh Token
    let refresh_expiration = Utc::now()
        .checked_add_signed(Duration::days(30))
        .unwrap_or_else(Utc::now);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "Essentially infallible"
    )]
    let refresh_claims = Claims {
        context_id,
        executor_public_key,
        exp: refresh_expiration.timestamp() as usize,
        token_type: TokenType::Refresh,
    };

    let refresh_token = encode(
        &Header::default(),
        &refresh_claims,
        &EncodingKey::from_secret(jwt_secret.as_slice()),
    )
    .map_err(|err| ApiError {
        status_code: StatusCode::BAD_REQUEST,
        message: format!("Failed to generate refresh token: {err}"),
    })?;

    let db_key = format!("{}{}", refresh_claims.context_id, refresh_claims.exp);
    let db_key_hash = hash::Hash::new(db_key.as_bytes());
    // Store the refresh token in the database
    create_refresh_token(
        store,
        refresh_token.as_bytes().to_vec(),
        db_key_hash.as_bytes(),
    )
    .map_err(|err| ApiError {
        status_code: StatusCode::INTERNAL_SERVER_ERROR,
        message: format!("Failed to store refresh token: {err}"),
    })?;

    Ok(JwtToken {
        access_token,
        refresh_token,
    })
}

// Check if the refresh token is valid and generate new tokens
// TODO: Consider splitting this function into smaller pieces
#[expect(clippy::too_many_lines, reason = "TODO: Will be refactored")]
pub fn refresh_access_token(refresh_token: &str, store: &Store) -> Result<JwtToken, ApiError> {
    // Get the JWT secret from the DB
    let jwt_secret = match get_jwt_secret(store) {
        Ok(Some(secret)) => secret.jwt_secret().to_vec(),
        Ok(None) => {
            return Err(ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                message: "JWT secret not found".into(),
            });
        }
        Err(err) => {
            return Err(ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                message: format!("Failed to get JWT secret: {err}"),
            });
        }
    };

    // Decode the token to check its claims
    let token_data = decode::<Claims>(
        refresh_token,
        &DecodingKey::from_secret(jwt_secret.as_slice()),
        &Validation::default(),
    )
    .map_err(|err| ApiError {
        status_code: StatusCode::FORBIDDEN,
        message: format!("Invalid refresh token: {err}"),
    })?;

    // Check if the token type is "refresh"
    if token_data.claims.token_type != TokenType::Refresh {
        return Err(ApiError {
            status_code: StatusCode::FORBIDDEN,
            message: "Invalid token type".into(),
        });
    }

    // Check if the token is expired
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "Essentially infallible"
    )]
    let now = Utc::now().timestamp() as usize;
    if token_data.claims.exp < now {
        return Err(ApiError {
            status_code: StatusCode::FORBIDDEN,
            message: "Refresh token has expired".into(),
        });
    }

    let context_id = token_data.claims.context_id;
    let executor = token_data.claims.executor_public_key.clone();

    let db_key = format!("{}{}", context_id, token_data.claims.exp);
    let db_key_hash = hash::Hash::new(db_key.as_bytes());

    // Check if the refresh token from the database is present
    let refresh_token_db = match get_refresh_token(store, db_key_hash.as_bytes()) {
        Ok(Some(token)) => {
            let refresh_token = from_utf8(token.refresh_token()).map_err(|err| ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                message: format!("Failed to parse refresh token: {err}"),
            })?;
            refresh_token.to_owned()
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
                message: format!("Failed to get refresh token: {err}"),
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

    delete_refresh_token(store, &db_key_hash).map_err(|err| ApiError {
        status_code: StatusCode::INTERNAL_SERVER_ERROR,
        message: format!("Failed to delete refresh token: {err}"),
    })?;

    // Generate new Access Token
    let access_expiration = Utc::now()
        .checked_add_signed(Duration::hours(1))
        .unwrap_or_else(Utc::now);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "Essentially infallible"
    )]
    let access_claims = Claims {
        context_id,
        executor_public_key: executor.clone(),
        exp: access_expiration.timestamp() as usize,
        token_type: TokenType::Access,
    };

    let access_token = encode(
        &Header::default(),
        &access_claims,
        &EncodingKey::from_secret(jwt_secret.as_slice()),
    )
    .map_err(|err| ApiError {
        status_code: StatusCode::BAD_REQUEST,
        message: format!("Failed to generate access token: {err}"),
    })?;

    let payload = JwtTokenRequest::new(context_id, executor);
    let jwt_tokens = generate_jwt_tokens(payload, store).map_err(|err| ApiError {
        status_code: StatusCode::BAD_REQUEST,
        message: format!("Failed to generate access token: {err}"),
    })?;

    Ok(JwtToken {
        access_token,
        refresh_token: jwt_tokens.refresh_token,
    })
}
