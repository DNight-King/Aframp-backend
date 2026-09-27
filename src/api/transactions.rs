use axum::extract::{Query, State};
use axum::Json;

use crate::auth::extractor::AuthUser;
use crate::error::{bad_request, internal, ApiResult, ErrorCode};
use crate::models::{ListParams, Payment};
use crate::pagination::{Cursor, Page};
use crate::services::payments;
use crate::AppState;

pub async fn list(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(params): Query<ListParams>,
) -> ApiResult<Json<Page<Payment>>> {
    let merchant_id = auth
        .merchant_id
        .ok_or_else(|| bad_request(ErrorCode::MerchantNotFound, "no merchant associated with this account"))?;
    let limit = params.merchant_limit();
    let cursor = match params.cursor.as_deref() {
        Some(raw) => Some(Cursor::decode(raw).ok_or_else(|| bad_request(ErrorCode::InvalidParameters, "invalid cursor"))?),
        None => None,
    };
    let payments = payments::payments_by_merchant_cursor(&state.db, merchant_id, limit, cursor)
        .await
        .map_err(internal)?;
    Ok(Json(Page::new(payments, limit, |p| Cursor {
        created_at: p.created_at,
        id: p.id,
    })))
}