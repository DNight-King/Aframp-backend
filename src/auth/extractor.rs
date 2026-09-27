use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::Json;

use crate::auth::{cookie, jwt};
use crate::auth::jwt::Claims;
use crate::error::{forbidden, internal, ApiError, ErrorCode};
use crate::services::users;
use crate::AppState;

#[derive(Debug, Clone)]
pub struct AuthUser {
    pub user_id: uuid::Uuid,
    pub merchant_id: Option<uuid::Uuid>,
}

/// The verified claims of the presented session token (bearer or cookie),
/// for handlers that need more than the user id — e.g. token refresh.
#[derive(Debug)]
pub struct Session(pub Claims);

impl FromRequestParts<AppState> for Session {
    type Rejection = (StatusCode, Json<ApiError>);

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        authenticate_active(parts, state).await.map(Session)
    }
}

/// Same session proof as [`AuthUser`], but additionally requires the `is_admin`
/// JWT claim. The claim is baked in at login and not re-checked against the
/// database, so revoking admin access takes up to [`jwt::TOKEN_TTL_HOURS`] to
/// take effect on outstanding tokens.
#[derive(Debug, Clone)]
pub struct AdminUser;

fn authenticate(parts: &Parts, state: &AppState) -> Result<Claims, (StatusCode, Json<ApiError>)> {
    // API clients send a bearer token; browsers send the HttpOnly session
    // cookie, which JS on the page cannot read. Either proves the session.
    let token = parts
        .headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .or_else(|| cookie::from_headers(&parts.headers))
        .ok_or_else(|| (StatusCode::UNAUTHORIZED, Json(ApiError { code: ErrorCode::InvalidCredentials, error: "missing session cookie or bearer token".into(), field: None })))?;
    jwt::verify(&state.jwt_secret, token)
        .map_err(|_| (StatusCode::UNAUTHORIZED, Json(ApiError { code: ErrorCode::InvalidCredentials, error: "invalid or expired token".into(), field: None })))
}

/// Verifies the token and that its account still exists and hasn't been
/// deleted, so deleting an account revokes every token issued for it.
async fn authenticate_active(
    parts: &Parts,
    state: &AppState,
) -> Result<Claims, (StatusCode, Json<ApiError>)> {
    let claims = authenticate(parts, state)?;
    if !users::is_active(&state.db, claims.sub).await.map_err(internal)? {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(ApiError { code: ErrorCode::InvalidCredentials, error: "invalid or expired token".into(), field: None }),
        ));
    }
    Ok(claims)
}

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = (StatusCode, Json<ApiError>);

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let claims = authenticate_active(parts, state).await?;
        Ok(AuthUser {
            user_id: claims.sub,
            merchant_id: claims.merchant_id,
        })
    }
}

impl FromRequestParts<AppState> for AdminUser {
    type Rejection = (StatusCode, Json<ApiError>);

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let claims = authenticate_active(parts, state).await?;
        if !claims.is_admin {
            return Err(forbidden(ErrorCode::Forbidden, "admin access required"));
        }
        tracing::info!(admin_user_id = %claims.sub, path = %parts.uri.path(), "admin access");
        Ok(AdminUser)
    }
}
