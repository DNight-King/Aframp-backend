use sqlx::PgPool;

use crate::auth::password;
use crate::models::{Merchant, User};

#[derive(Debug, thiserror::Error)]
pub enum UserError {
    #[error("invalid email or password")]
    InvalidCredentials,
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

/// Materializes a brand-new account. The only caller is
/// `services::otp::verify`, once a signup's OTP challenge passes — never
/// called speculatively, so an unverified phone can never end up attached
/// to a real, loggable-in account. `password_hash` must already be hashed.
pub async fn create_verified(
    db: &PgPool,
    email: &str,
    password_hash: &str,
    name: &str,
    phone_number: &str,
) -> Result<(User, Merchant), sqlx::Error> {
    let mut tx = db.begin().await?;
    let user = sqlx::query_as::<_, User>(
        "INSERT INTO users (email, password_hash, name, phone_number, phone_verified)
         VALUES ($1, $2, $3, $4, true)
         RETURNING id, email, password_hash, name, is_admin, phone_number, phone_verified, created_at, updated_at",
    )
    .bind(email)
    .bind(password_hash)
    .bind(name)
    .bind(phone_number)
    .fetch_one(&mut *tx)
    .await?;

    let merchant = sqlx::query_as::<_, Merchant>(
        "INSERT INTO merchants (user_id, name)
         VALUES ($1, $2)
         RETURNING id, user_id, name, created_at",
    )
    .bind(user.id)
    .bind(name)
    .fetch_one(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok((user, merchant))
}

/// Which unique constraint a `23505` violation hit, if any — lets a caller
/// distinguish "email taken" from "phone taken" from an unrelated conflict.
pub(crate) fn unique_violation_field(err: &sqlx::Error) -> Option<&str> {
    match err {
        sqlx::Error::Database(db) if db.code().as_deref() == Some("23505") => db.constraint(),
        _ => None,
    }
}

pub async fn login(db: &PgPool, email: &str, password_raw: &str) -> Result<(User, Option<Merchant>), UserError> {
    let user = sqlx::query_as::<_, User>(
        "SELECT id, email, password_hash, name, is_admin, phone_number, phone_verified, created_at, updated_at
           FROM users WHERE email = $1",
    )
    .bind(email)
    .fetch_optional(db)
    .await?
    .ok_or(UserError::InvalidCredentials)?;

    if !password::verify(password_raw, &user.password_hash) {
        return Err(UserError::InvalidCredentials);
    }

    let merchant = sqlx::query_as::<_, Merchant>(
        "SELECT id, user_id, name, created_at FROM merchants WHERE user_id = $1 LIMIT 1",
    )
    .bind(user.id)
    .fetch_optional(db)
    .await?;

    Ok((user, merchant))
}

pub async fn user_by_id(db: &PgPool, user_id: uuid::Uuid) -> Result<Option<User>, sqlx::Error> {
    sqlx::query_as::<_, User>(
        "SELECT id, email, password_hash, name, is_admin, phone_number, phone_verified, created_at, updated_at
           FROM users WHERE id = $1",
    )
    .bind(user_id)
    .fetch_optional(db)
    .await
}

pub async fn merchant_by_user(db: &PgPool, user_id: uuid::Uuid) -> Result<Option<Merchant>, sqlx::Error> {
    sqlx::query_as::<_, Merchant>(
        "SELECT id, user_id, name, created_at FROM merchants WHERE user_id = $1 LIMIT 1",
    )
    .bind(user_id)
    .fetch_optional(db)
    .await
}

pub async fn merchant_by_id(db: &PgPool, merchant_id: uuid::Uuid) -> Result<Option<Merchant>, sqlx::Error> {
    sqlx::query_as::<_, Merchant>(
        "SELECT id, user_id, name, created_at FROM merchants WHERE id = $1",
    )
    .bind(merchant_id)
    .fetch_optional(db)
    .await
}

/// Erases a user's personal data (GDPR / NDPA right to erasure) while
/// keeping the rows that the financial records point at:
///
/// - email becomes `deleted-<user id>@deleted.invalid`, name and the
///   merchant name become "Deleted user", the phone number is cleared
/// - the password hash is blanked, so the account can never log in again
/// - pending OTP challenges (which can hold PII) are removed
/// - `deleted_at` is set, which makes every outstanding session token for
///   this user fail authentication
///
/// Payments, payment requests, withdrawals, wallets and balances are left
/// intact. Returns `false` if the user doesn't exist or was already deleted.
pub async fn anonymize_and_delete(db: &PgPool, user_id: uuid::Uuid) -> Result<bool, sqlx::Error> {
    let mut tx = db.begin().await?;
    let updated = sqlx::query(
        "UPDATE users
            SET email = 'deleted-' || id::text || '@deleted.invalid',
                name = 'Deleted user',
                phone_number = NULL,
                phone_verified = false,
                password_hash = '',
                deleted_at = now(),
                updated_at = now()
          WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(user_id)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if updated == 0 {
        return Ok(false);
    }
    sqlx::query("UPDATE merchants SET name = 'Deleted user' WHERE user_id = $1")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM otp_challenges WHERE user_id = $1")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(true)
}

/// Whether the account exists and hasn't been deleted.
pub async fn is_active(db: &PgPool, user_id: uuid::Uuid) -> Result<bool, sqlx::Error> {
    Ok(sqlx::query_scalar::<_, bool>("SELECT deleted_at IS NULL FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_optional(db)
        .await?
        .unwrap_or(false))
}
