use async_graphql::{Context, InputObject, Object, SimpleObject};
use bcrypt::{hash, verify, DEFAULT_COST};
use rand::distributions::Alphanumeric;
use rand::Rng;
use serde::Deserialize;
use crate::mail::Mailer;
use sqlx::PgPool;

// ── Helpers ───────────────────────────────────────────────────────────────────

fn new_id() -> String {
    rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(20)
        .map(char::from)
        .collect()
}

fn new_token() -> String {
    rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(40)
        .map(char::from)
        .collect()
}

// ── Password reset helpers ────────────────────────────────────────────────────

const RESET_CODE_TTL_MINUTES: i32 = 15;
const RESET_MAX_ATTEMPTS: i32 = 5;
const RESET_RESEND_COOLDOWN_SECONDS: i32 = 60;
const MIN_PASSWORD_LEN: usize = 8;

/// A random 6-digit code, zero-padded ("004217").
fn new_reset_code() -> String {
    format!("{:06}", rand::thread_rng().gen_range(0..1_000_000))
}

fn normalize_email(email: &str) -> String {
    email.trim().to_lowercase()
}

// ── DB row types ──────────────────────────────────────────────────────────────

#[derive(sqlx::FromRow)]
struct UserRow {
    id: String,
    username: String,
}

#[derive(sqlx::FromRow)]
struct CollectionRow {
    id: String,
    name: String,
    language: String,
    user_id: String,
}

#[derive(sqlx::FromRow)]
struct TranslationRow {
    id: String,
    from: String,
    to: String,
    language: Option<String>,
    collection_id: String,
}

// ── Social-login helpers ──────────────────────────────────────────────────────

/// Deserialized fields from Google's tokeninfo endpoint.
#[derive(Deserialize)]
struct GoogleTokenInfo {
    sub: String,            // stable Google user ID
    email: Option<String>,
    /// Google sends this as the string "true" / "false" (older responses used a bool).
    #[serde(default)]
    email_verified: serde_json::Value,
    #[serde(default)]
    error_description: Option<String>,
}

/// Verify a Google ID token via the tokeninfo endpoint.
/// Returns `(google_uid, email)` on success. The email is only returned when Google
/// has verified that the user owns it, so it is safe to use for account linking.
async fn verify_google_token(id_token: &str) -> async_graphql::Result<(String, Option<String>)> {
    let url = format!(
        "https://oauth2.googleapis.com/tokeninfo?id_token={}",
        id_token
    );
    let resp = reqwest::get(&url)
        .await
        .map_err(|e| async_graphql::Error::new(format!("Google tokeninfo request failed: {e}")))?;

    let info: GoogleTokenInfo = resp
        .json()
        .await
        .map_err(|e| async_graphql::Error::new(format!("Failed to parse Google tokeninfo: {e}")))?;

    if let Some(desc) = info.error_description {
        return Err(async_graphql::Error::new(format!("Invalid Google token: {desc}")));
    }

    let verified = match &info.email_verified {
        serde_json::Value::Bool(b) => *b,
        serde_json::Value::String(v) => v == "true",
        _ => false,
    };
    Ok((info.sub, info.email.filter(|_| verified)))
}

/// Derive a username from an email address.
/// e.g. "john.doe@gmail.com" → "john.doe", with a random 4-digit suffix if taken.
fn username_from_email(email: &str) -> String {
    let base = email.split('@').next().unwrap_or("user");
    // Sanitise: keep only alphanumeric + underscore + dot
    let clean: String = base
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '_' || c == '.' { c } else { '_' })
        .collect();
    if clean.is_empty() { "user".to_string() } else { clean }
}

/// Find-or-create a user for a social provider login.
/// Returns `(user_id, username)`.
///
/// `email` must only be passed when the provider has verified it. A verified email that
/// matches an existing account links the provider to that account instead of creating a
/// duplicate. Sign-up doesn't verify emails, so someone could have registered an address
/// they don't own; when linking we therefore remove the account's password and sessions.
/// The verified owner can set a new password through the reset flow.
async fn upsert_social_user(
    pool: &PgPool,
    provider: &str,
    provider_uid: &str,
    email: Option<&str>,
) -> async_graphql::Result<(String, String)> {
    // 1. Look up existing provider row.
    let existing = sqlx::query!(
        "SELECT u.id, u.username
         FROM auth_providers ap
         JOIN users u ON u.id = ap.user_id
         WHERE ap.provider = $1 AND ap.provider_uid = $2",
        provider,
        provider_uid
    )
    .fetch_optional(pool)
    .await?;

    if let Some(row) = existing {
        return Ok((row.id, row.username));
    }

    // 2. A verified email that belongs to an existing account links to it.
    if let Some(email) = email {
        let matches: Vec<(String, String)> = sqlx::query_as(
            "SELECT id, username FROM users WHERE lower(email) = lower($1) LIMIT 2",
        )
        .bind(email)
        .fetch_all(pool)
        .await?;

        match matches.len() {
            0 => {}
            1 => {
                let (user_id, username) = matches.into_iter().next().unwrap();
                let mut tx = pool.begin().await?;
                sqlx::query(
                    "INSERT INTO auth_providers (user_id, provider, provider_uid, email)
                     VALUES ($1, $2, $3, $4)",
                )
                .bind(&user_id)
                .bind(provider)
                .bind(provider_uid)
                .bind(email)
                .execute(&mut *tx)
                .await?;
                sqlx::query("UPDATE users SET password_hash = NULL WHERE id = $1")
                    .bind(&user_id)
                    .execute(&mut *tx)
                    .await?;
                sqlx::query("DELETE FROM sessions WHERE user_id = $1")
                    .bind(&user_id)
                    .execute(&mut *tx)
                    .await?;
                tx.commit().await?;
                return Ok((user_id, username));
            }
            _ => {
                return Err(async_graphql::Error::new(
                    "Several accounts use this email address, so it can't be linked automatically",
                ));
            }
        }
    }

    // 3. No provider row and no matching account — create a new user.
    // `users.email` is required, so without a verified email we can't create one.
    let email = email.ok_or_else(|| {
        async_graphql::Error::new("Your Google account has no verified email address")
    })?;
    let user_id = new_id();
    let base_username = username_from_email(email);

    // Append random suffix if the username is already taken.
    let username = {
        let taken: Option<String> = sqlx::query_scalar!(
            "SELECT id FROM users WHERE username = $1",
            base_username
        )
        .fetch_optional(pool)
        .await?;

        if taken.is_some() {
            format!(
                "{}_{}",
                base_username,
                rand::thread_rng()
                    .sample_iter(&Alphanumeric)
                    .take(4)
                    .map(char::from)
                    .collect::<String>()
            )
        } else {
            base_username
        }
    };

    sqlx::query!(
        "INSERT INTO users (id, username, email, password_hash)
         VALUES ($1, $2, $3, NULL)",
        user_id,
        username,
        email
    )
    .execute(pool)
    .await
    .map_err(|e| async_graphql::Error::new(e.to_string()))?;

    sqlx::query!(
        "INSERT INTO auth_providers (user_id, provider, provider_uid, email)
         VALUES ($1, $2, $3, $4)",
        user_id,
        provider,
        provider_uid,
        email
    )
    .execute(pool)
    .await
    .map_err(|e| async_graphql::Error::new(e.to_string()))?;

    Ok((user_id, username))
}

// ── GraphQL output types ──────────────────────────────────────────────────────

#[derive(SimpleObject, Clone)]
pub struct GqlUser {
    pub id: String,
    pub username: String,
}

#[derive(SimpleObject, Clone)]
pub struct GqlCollection {
    pub id: String,
    pub name: String,
    pub language: String,
    pub user: GqlUser,
}

#[derive(SimpleObject, Clone)]
pub struct GqlTranslation {
    pub id: String,
    pub from: String,
    pub to: String,
    pub language: Option<String>,
    pub collection: GqlCollectionBasic,
}

/// Collection without the user field — used inside Translation responses.
#[derive(SimpleObject, Clone)]
pub struct GqlCollectionBasic {
    pub id: String,
    pub name: String,
    pub language: String,
}

/// Wrapper returned by logIn / signUp — mirrors Parse's viewer shape.
#[derive(SimpleObject)]
pub struct GqlViewer {
    pub session_token: String,
    pub user: GqlUser,
}

/// Wrapper for the `viewer` query (token verification).
#[derive(SimpleObject)]
pub struct GqlViewerQuery {
    pub user: GqlUser,
}

// Connection/edge wrappers matching the app's `edges { node { ... } }` shape.

#[derive(SimpleObject)]
pub struct GqlCollectionEdge {
    pub node: GqlCollectionBasic,
}

#[derive(SimpleObject)]
pub struct GqlCollectionConnection {
    pub edges: Vec<GqlCollectionEdge>,
}

#[derive(SimpleObject)]
pub struct GqlTranslationNode {
    pub id: String,
    pub from: String,
    pub to: String,
    pub language: Option<String>,
}

#[derive(SimpleObject)]
pub struct GqlTranslationEdge {
    pub node: GqlTranslationNode,
}

#[derive(SimpleObject)]
pub struct GqlTranslationConnection {
    pub edges: Vec<GqlTranslationEdge>,
}

/// Result type for `GET_TRANSLATIONS` — list + parent collection.
#[derive(SimpleObject)]
pub struct GqlTranslationsResult {
    pub translations: GqlTranslationConnection,
    pub collection: GqlCollectionBasic,
}

// ── Input types ───────────────────────────────────────────────────────────────

#[derive(InputObject)]
pub struct SignUpInput {
    pub username: String,
    pub password: String,
    pub email: String,
}

#[derive(InputObject)]
pub struct SignUpFields {
    pub fields: SignUpInput,
}

#[derive(InputObject)]
pub struct CreateCollectionFields {
    pub name: String,
    pub language: String,
    /// The user to link — must be an existing user id.
    pub user: LinkInput,
}

#[derive(InputObject)]
pub struct CreateCollectionInput {
    pub fields: CreateCollectionFields,
}

#[derive(InputObject)]
pub struct UpdateCollectionFields {
    pub name: String,
    pub language: String,
}

#[derive(InputObject)]
pub struct UpdateCollectionInput {
    pub id: String,
    pub fields: UpdateCollectionFields,
}

#[derive(InputObject)]
pub struct DeleteCollectionInput {
    pub id: String,
}

#[derive(InputObject)]
pub struct CreateTranslationFields {
    pub from: String,
    pub to: String,
    pub language: Option<String>,
    /// The collection to link.
    pub collection: LinkInput,
}

#[derive(InputObject)]
pub struct CreateTranslationInput {
    pub fields: CreateTranslationFields,
}

#[derive(InputObject)]
pub struct UpdateTranslationFields {
    pub from: String,
    pub to: String,
    /// Optionally move the translation to a different collection.
    pub collection: Option<LinkInput>,
}

#[derive(InputObject)]
pub struct UpdateTranslationInput {
    pub id: String,
    pub fields: UpdateTranslationFields,
}

#[derive(InputObject)]
pub struct DeleteTranslationInput {
    pub id: String,
}

/// Generic `{ link: "<id>" }` pointer — mirrors Parse's relation syntax.
#[derive(InputObject)]
pub struct LinkInput {
    pub link: String,
}

// ── Mutation result wrappers ──────────────────────────────────────────────────

#[derive(SimpleObject)]
pub struct SignUpResult {
    pub viewer: GqlViewer,
}

#[derive(SimpleObject)]
pub struct LogInResult {
    pub viewer: GqlViewer,
}

#[derive(SimpleObject)]
pub struct CreateCollectionResult {
    pub collection: GqlCollection,
}

#[derive(SimpleObject)]
pub struct UpdateCollectionResult {
    pub collection: GqlCollection,
}

#[derive(SimpleObject)]
pub struct DeleteCollectionResult {
    pub collection: GqlCollection,
}

#[derive(SimpleObject)]
pub struct CreateTranslationResult {
    pub translation: GqlTranslation,
}

#[derive(SimpleObject)]
pub struct UpdateTranslationResult {
    pub translation: GqlTranslation,
}

#[derive(SimpleObject)]
pub struct DeleteTranslationResult {
    pub translation: GqlTranslation,
}

// ── Context token extraction ──────────────────────────────────────────────────

/// Extracts the `X-Parse-Session-Token` header value from the request context.
fn session_token_from_ctx(ctx: &Context<'_>) -> Option<String> {
    ctx.data_opt::<SessionToken>().map(|t| t.0.clone())
}

async fn user_from_token(
    pool: &PgPool,
    token: &str,
) -> async_graphql::Result<GqlUser> {
    let row = sqlx::query_as!(
        UserRow,
        "SELECT u.id, u.username
         FROM users u
         JOIN sessions s ON s.user_id = u.id
         WHERE s.token = $1 AND s.expires_at > now()",
        token
    )
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| async_graphql::Error::new("Invalid or expired session token"))?;
    Ok(GqlUser { id: row.id, username: row.username })
}

/// Newtype wrapper so we can store the session token in the GraphQL context.
pub struct SessionToken(pub String);

// ── Query root ────────────────────────────────────────────────────────────────

#[derive(Default)]
pub struct TranslatorQuery;

#[Object]
impl TranslatorQuery {
    /// Verify the current session token and return the logged-in user.
    async fn viewer(&self, ctx: &Context<'_>) -> async_graphql::Result<GqlViewerQuery> {
        let pool = ctx.data::<PgPool>()?;
        let token = session_token_from_ctx(ctx)
            .ok_or_else(|| async_graphql::Error::new("Missing session token"))?;
        let user = user_from_token(pool, &token).await?;
        Ok(GqlViewerQuery { user })
    }

    /// Get a single collection by ID.
    async fn collection(
        &self,
        ctx: &Context<'_>,
        id: String,
    ) -> async_graphql::Result<GqlCollection> {
        let pool = ctx.data::<PgPool>()?;
        let row = sqlx::query_as!(
            CollectionRow,
            "SELECT id, name, language, user_id
             FROM collections WHERE id = $1",
            id
        )
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| async_graphql::Error::new("Collection not found"))?;

        let user = sqlx::query_as!(
            UserRow,
            "SELECT id, username FROM users WHERE id = $1",
            row.user_id
        )
        .fetch_one(pool)
        .await?;

        Ok(GqlCollection {
            id: row.id,
            name: row.name,
            language: row.language,
            user: GqlUser { id: user.id, username: user.username },
        })
    }

    /// List all collections for a given user.
    /// Mirrors `collections(where: { user: { have: { id: { equalTo: $userId } } } })`.
    async fn collections(
        &self,
        ctx: &Context<'_>,
        user_id: String,
    ) -> async_graphql::Result<GqlCollectionConnection> {
        let pool = ctx.data::<PgPool>()?;
        let rows = sqlx::query_as!(
            CollectionRow,
            "SELECT id, name, language, user_id
             FROM collections WHERE user_id = $1
             ORDER BY created_at ASC",
            user_id
        )
        .fetch_all(pool)
        .await?;

        let edges = rows
            .into_iter()
            .map(|r| GqlCollectionEdge {
                node: GqlCollectionBasic { id: r.id, name: r.name, language: r.language },
            })
            .collect();

        Ok(GqlCollectionConnection { edges })
    }

    /// Get all translations for a given collection, plus the collection itself.
    /// Mirrors `translations(where: { collection: { have: { id: { equalTo: $collectionId } } } })`.
    async fn translations(
        &self,
        ctx: &Context<'_>,
        collection_id: String,
    ) -> async_graphql::Result<GqlTranslationsResult> {
        let pool = ctx.data::<PgPool>()?;

        let col = sqlx::query_as!(
            CollectionRow,
            "SELECT id, name, language, user_id
             FROM collections WHERE id = $1",
            collection_id
        )
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| async_graphql::Error::new("Collection not found"))?;

        let rows = sqlx::query_as!(
            TranslationRow,
            "SELECT id, \"from\", \"to\", language, collection_id
             FROM translations WHERE collection_id = $1
             ORDER BY created_at ASC",
            collection_id
        )
        .fetch_all(pool)
        .await?;

        let edges = rows
            .into_iter()
            .map(|r| GqlTranslationEdge {
                node: GqlTranslationNode {
                    id: r.id,
                    from: r.from,
                    to: r.to,
                    language: r.language,
                },
            })
            .collect();

        Ok(GqlTranslationsResult {
            translations: GqlTranslationConnection { edges },
            collection: GqlCollectionBasic {
                id: col.id,
                name: col.name,
                language: col.language,
            },
        })
    }
}

// ── Mutation root ─────────────────────────────────────────────────────────────

#[derive(Default)]
pub struct TranslatorMutation;

#[Object]
impl TranslatorMutation {
    /// Register a new user.
    async fn sign_up(
        &self,
        ctx: &Context<'_>,
        input: SignUpFields,
    ) -> async_graphql::Result<SignUpResult> {
        let pool = ctx.data::<PgPool>()?;
        let f = input.fields;
        let id = new_id();
        let password_hash = hash(&f.password, DEFAULT_COST)
            .map_err(|e| async_graphql::Error::new(e.to_string()))?;

        sqlx::query!(
            "INSERT INTO users (id, username, email, password_hash)
             VALUES ($1, $2, $3, $4)",
            id,
            f.username,
            f.email,
            password_hash
        )
        .execute(pool)
        .await
        .map_err(|e| async_graphql::Error::new(e.to_string()))?;

        let token = new_token();
        sqlx::query!(
            "INSERT INTO sessions (token, user_id) VALUES ($1, $2)",
            token,
            id
        )
        .execute(pool)
        .await?;

        Ok(SignUpResult {
            viewer: GqlViewer {
                session_token: token,
                user: GqlUser { id, username: f.username },
            },
        })
    }

    /// Log in with username and password.
    async fn log_in(
        &self,
        ctx: &Context<'_>,
        username: String,
        password: String,
    ) -> async_graphql::Result<LogInResult> {
        let pool = ctx.data::<PgPool>()?;

        let row: Option<(String, String, Option<String>)> = sqlx::query_as(
            "SELECT id, username, password_hash FROM users WHERE username = $1",
        )
        .bind(&username)
        .fetch_optional(pool)
        .await?;
        let (id, db_username, password_hash) =
            row.ok_or_else(|| async_graphql::Error::new("Invalid username or password"))?;

        // Accounts created through Google have no password.
        let valid = match password_hash {
            Some(hash) => verify(&password, &hash)
                .map_err(|e| async_graphql::Error::new(e.to_string()))?,
            None => false,
        };
        if !valid {
            return Err(async_graphql::Error::new("Invalid username or password"));
        }

        let token = new_token();
        sqlx::query!(
            "INSERT INTO sessions (token, user_id) VALUES ($1, $2)",
            token,
            id
        )
        .execute(pool)
        .await?;

        Ok(LogInResult {
            viewer: GqlViewer {
                session_token: token,
                user: GqlUser { id, username: db_username },
            },
        })
    }

    /// Ask for a password reset code to be emailed.
    ///
    /// Always returns `true`, whether or not the address has an account, so the response
    /// cannot be used to find out who is registered. Requests for the same account within
    /// a minute of each other are ignored.
    async fn request_password_reset(
        &self,
        ctx: &Context<'_>,
        email: String,
    ) -> async_graphql::Result<bool> {
        let pool = ctx.data::<PgPool>()?;
        let mailer = ctx.data::<Mailer>()?.clone();
        let email = normalize_email(&email);

        // Hash before looking the user up so known and unknown addresses take similar time.
        let code = new_reset_code();
        let code_hash = hash(&code, DEFAULT_COST)
            .map_err(|e| async_graphql::Error::new(e.to_string()))?;

        let user: Option<(String, String, String)> =
            sqlx::query_as("SELECT id, username, email FROM users WHERE lower(email) = $1")
                .bind(&email)
                .fetch_optional(pool)
                .await?;
        let Some((user_id, username, address)) = user else {
            return Ok(true);
        };

        let stored = sqlx::query(
            "INSERT INTO password_resets (user_id, code_hash, expires_at)
             VALUES ($1, $2, now() + ($3 * interval '1 minute'))
             ON CONFLICT (user_id) DO UPDATE
                SET code_hash = EXCLUDED.code_hash,
                    attempts = 0,
                    created_at = now(),
                    expires_at = EXCLUDED.expires_at
                WHERE password_resets.created_at < now() - ($4 * interval '1 second')",
        )
        .bind(&user_id)
        .bind(&code_hash)
        .bind(RESET_CODE_TTL_MINUTES)
        .bind(RESET_RESEND_COOLDOWN_SECONDS)
        .execute(pool)
        .await?
        .rows_affected();

        if stored > 0 {
            // Send in the background so the response time does not reveal the account exists.
            tokio::spawn(async move {
                mailer.send_reset_code(&address, &username, &code).await;
            });
        }
        Ok(true)
    }

    /// Set a new password using the code from the reset email.
    /// All existing sessions for the account are ended.
    async fn reset_password(
        &self,
        ctx: &Context<'_>,
        email: String,
        code: String,
        new_password: String,
    ) -> async_graphql::Result<bool> {
        let pool = ctx.data::<PgPool>()?;
        let invalid = || async_graphql::Error::new("Invalid or expired code");

        if new_password.chars().count() < MIN_PASSWORD_LEN {
            return Err(async_graphql::Error::new(format!(
                "Password must be at least {MIN_PASSWORD_LEN} characters"
            )));
        }

        let email = normalize_email(&email);
        let user_id: Option<String> =
            sqlx::query_scalar("SELECT id FROM users WHERE lower(email) = $1")
                .bind(&email)
                .fetch_optional(pool)
                .await?;
        let Some(user_id) = user_id else {
            // Spend similar time as a real attempt.
            let _ = hash(&code, DEFAULT_COST);
            return Err(invalid());
        };

        // Count the attempt before checking the code, in one statement, so parallel guesses
        // cannot get past the limit.
        let code_hash: Option<String> = sqlx::query_scalar(
            "UPDATE password_resets SET attempts = attempts + 1
             WHERE user_id = $1 AND expires_at > now() AND attempts < $2
             RETURNING code_hash",
        )
        .bind(&user_id)
        .bind(RESET_MAX_ATTEMPTS)
        .fetch_optional(pool)
        .await?;
        let Some(code_hash) = code_hash else {
            return Err(invalid());
        };

        let matches = verify(code.trim(), &code_hash)
            .map_err(|e| async_graphql::Error::new(e.to_string()))?;
        if !matches {
            return Err(invalid());
        }

        let new_hash = hash(&new_password, DEFAULT_COST)
            .map_err(|e| async_graphql::Error::new(e.to_string()))?;
        let mut tx = pool.begin().await?;
        sqlx::query("UPDATE users SET password_hash = $2 WHERE id = $1")
            .bind(&user_id)
            .bind(&new_hash)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM password_resets WHERE user_id = $1")
            .bind(&user_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM sessions WHERE user_id = $1")
            .bind(&user_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(true)
    }

    /// Create a new collection.
    async fn create_collection(
        &self,
        ctx: &Context<'_>,
        input: CreateCollectionInput,
    ) -> async_graphql::Result<CreateCollectionResult> {
        let pool = ctx.data::<PgPool>()?;
        let mut f = input.fields;
        f.name = clean_collection_name(&f.name)?;
        let id = new_id();

        sqlx::query!(
            "INSERT INTO collections (id, name, language, user_id)
             VALUES ($1, $2, $3, $4)",
            id,
            f.name,
            f.language,
            f.user.link
        )
        .execute(pool)
        .await
        .map_err(|e| collection_write_error(e, &f.name))?;

        let user = sqlx::query_as!(
            UserRow,
            "SELECT id, username FROM users WHERE id = $1",
            f.user.link
        )
        .fetch_one(pool)
        .await?;

        Ok(CreateCollectionResult {
            collection: GqlCollection {
                id,
                name: f.name,
                language: f.language,
                user: GqlUser { id: user.id, username: user.username },
            },
        })
    }

    /// Update name and/or language of an existing collection.
    async fn update_collection(
        &self,
        ctx: &Context<'_>,
        input: UpdateCollectionInput,
    ) -> async_graphql::Result<UpdateCollectionResult> {
        let pool = ctx.data::<PgPool>()?;
        let name = clean_collection_name(&input.fields.name)?;

        let row = sqlx::query_as!(
            CollectionRow,
            "UPDATE collections SET name = $1, language = $2, updated_at = now()
             WHERE id = $3
             RETURNING id, name, language, user_id",
            name,
            input.fields.language,
            input.id
        )
        .fetch_optional(pool)
        .await
        .map_err(|e| collection_write_error(e, &name))?
        .ok_or_else(|| async_graphql::Error::new("Collection not found"))?;

        let user = sqlx::query_as!(
            UserRow,
            "SELECT id, username FROM users WHERE id = $1",
            row.user_id
        )
        .fetch_one(pool)
        .await?;

        Ok(UpdateCollectionResult {
            collection: GqlCollection {
                id: row.id,
                name: row.name,
                language: row.language,
                user: GqlUser { id: user.id, username: user.username },
            },
        })
    }

    /// Delete a collection by ID and return the deleted record.
    async fn delete_collection(
        &self,
        ctx: &Context<'_>,
        input: DeleteCollectionInput,
    ) -> async_graphql::Result<DeleteCollectionResult> {
        let pool = ctx.data::<PgPool>()?;

        let row = sqlx::query_as!(
            CollectionRow,
            "DELETE FROM collections WHERE id = $1
             RETURNING id, name, language, user_id",
            input.id
        )
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| async_graphql::Error::new("Collection not found"))?;

        let user = sqlx::query_as!(
            UserRow,
            "SELECT id, username FROM users WHERE id = $1",
            row.user_id
        )
        .fetch_one(pool)
        .await?;

        Ok(DeleteCollectionResult {
            collection: GqlCollection {
                id: row.id,
                name: row.name,
                language: row.language,
                user: GqlUser { id: user.id, username: user.username },
            },
        })
    }

    /// Create a translation and link it to a collection.
    async fn create_translation(
        &self,
        ctx: &Context<'_>,
        input: CreateTranslationInput,
    ) -> async_graphql::Result<CreateTranslationResult> {
        let pool = ctx.data::<PgPool>()?;
        let f = input.fields;
        let id = new_id();

        sqlx::query!(
            "INSERT INTO translations (id, \"from\", \"to\", language, collection_id)
             VALUES ($1, $2, $3, $4, $5)",
            id,
            f.from,
            f.to,
            f.language,
            f.collection.link
        )
        .execute(pool)
        .await
        .map_err(|e| async_graphql::Error::new(e.to_string()))?;

        let col = sqlx::query_as!(
            CollectionRow,
            "SELECT id, name, language, user_id FROM collections WHERE id = $1",
            f.collection.link
        )
        .fetch_one(pool)
        .await?;

        Ok(CreateTranslationResult {
            translation: GqlTranslation {
                id,
                from: f.from,
                to: f.to,
                language: f.language,
                collection: GqlCollectionBasic {
                    id: col.id,
                    name: col.name,
                    language: col.language,
                },
            },
        })
    }

    /// Update an existing translation.
    async fn update_translation(
        &self,
        ctx: &Context<'_>,
        input: UpdateTranslationInput,
    ) -> async_graphql::Result<UpdateTranslationResult> {
        let pool = ctx.data::<PgPool>()?;

        // Resolve the new collection_id — keep existing if not provided.
        let new_col_id = match input.fields.collection {
            Some(ref link) => link.link.clone(),
            None => {
                sqlx::query_scalar!(
                    "SELECT collection_id FROM translations WHERE id = $1",
                    input.id
                )
                .fetch_optional(pool)
                .await?
                .ok_or_else(|| async_graphql::Error::new("Translation not found"))?
            }
        };

        let row = sqlx::query_as!(
            TranslationRow,
            "UPDATE translations
             SET \"from\" = $1, \"to\" = $2, collection_id = $3, updated_at = now()
             WHERE id = $4
             RETURNING id, \"from\", \"to\", language, collection_id",
            input.fields.from,
            input.fields.to,
            new_col_id,
            input.id
        )
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| async_graphql::Error::new("Translation not found"))?;

        let col = sqlx::query_as!(
            CollectionRow,
            "SELECT id, name, language, user_id FROM collections WHERE id = $1",
            row.collection_id
        )
        .fetch_one(pool)
        .await?;

        Ok(UpdateTranslationResult {
            translation: GqlTranslation {
                id: row.id,
                from: row.from,
                to: row.to,
                language: row.language,
                collection: GqlCollectionBasic {
                    id: col.id,
                    name: col.name,
                    language: col.language,
                },
            },
        })
    }

    /// Delete a translation by ID and return the deleted record.
    async fn delete_translation(
        &self,
        ctx: &Context<'_>,
        input: DeleteTranslationInput,
    ) -> async_graphql::Result<DeleteTranslationResult> {
        let pool = ctx.data::<PgPool>()?;

        let row = sqlx::query_as!(
            TranslationRow,
            "DELETE FROM translations WHERE id = $1
             RETURNING id, \"from\", \"to\", language, collection_id",
            input.id
        )
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| async_graphql::Error::new("Translation not found"))?;

        let col = sqlx::query_as!(
            CollectionRow,
            "SELECT id, name, language, user_id FROM collections WHERE id = $1",
            row.collection_id
        )
        .fetch_one(pool)
        .await?;

        Ok(DeleteTranslationResult {
            translation: GqlTranslation {
                id: row.id,
                from: row.from,
                to: row.to,
                language: row.language,
                collection: GqlCollectionBasic {
                    id: col.id,
                    name: col.name,
                    language: col.language,
                },
            },
        })
    }

    /// Sign in with a Google ID token.
    /// If no account exists for this Google identity, one is created automatically.
    async fn log_in_with_google(
        &self,
        ctx: &Context<'_>,
        id_token: String,
    ) -> async_graphql::Result<LogInResult> {
        let pool = ctx.data::<PgPool>()?;
        let (google_uid, email) = verify_google_token(&id_token).await?;
        let (user_id, username) =
            upsert_social_user(pool, "google", &google_uid, email.as_deref()).await?;

        let token = new_token();
        sqlx::query!(
            "INSERT INTO sessions (token, user_id) VALUES ($1, $2)",
            token,
            user_id
        )
        .execute(pool)
        .await?;

        Ok(LogInResult {
            viewer: GqlViewer {
                session_token: token,
                user: GqlUser { id: user_id, username },
            },
        })
    }

    /// Sign in with a Facebook access token.
    /// **Not yet implemented** — returns an error so the schema is already in place
    /// for when Facebook OAuth is added.
    async fn log_in_with_facebook(
        &self,
        _ctx: &Context<'_>,
        _access_token: String,
    ) -> async_graphql::Result<LogInResult> {
        Err(async_graphql::Error::new(
            "Facebook login is not yet implemented",
        ))
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// Name of the unique index in migration 0006.
const COLLECTION_NAME_INDEX: &str = "collections_user_name_unique";

/// Trims a collection name and refuses an empty one.
fn clean_collection_name(name: &str) -> async_graphql::Result<String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(async_graphql::Error::new("Collection name can't be empty."));
    }
    Ok(name.to_string())
}

/// Turns the "name already used" database error into a message the app can show as is.
fn collection_write_error(e: sqlx::Error, name: &str) -> async_graphql::Error {
    if let Some(db) = e.as_database_error() {
        if db.is_unique_violation() && db.constraint() == Some(COLLECTION_NAME_INDEX) {
            return async_graphql::Error::new(format!(
                "You already have a collection named \"{name}\"."
            ));
        }
    }
    async_graphql::Error::new(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_graphql::{EmptySubscription, Request, Schema, Variables};
    use serde_json::json;

    type TestSchema = Schema<TranslatorQuery, TranslatorMutation, EmptySubscription>;

    const OLD_PASSWORD: &str = "oldpassword";
    const NEW_PASSWORD: &str = "brand-new-pass";
    const CODE: &str = "123456";

    fn schema(pool: &PgPool) -> TestSchema {
        Schema::build(TranslatorQuery::default(), TranslatorMutation::default(), EmptySubscription)
            .data(pool.clone())
            .data(Mailer::from_env())
            .finish()
    }

    async fn run(
        schema: &TestSchema,
        query: &str,
        variables: serde_json::Value,
    ) -> async_graphql::Response {
        schema
            .execute(Request::new(query).variables(Variables::from_json(variables)))
            .await
    }

    async fn add_user(pool: &PgPool, id: &str, username: &str, email: &str) {
        let password_hash = hash(OLD_PASSWORD, 4).unwrap();
        sqlx::query("INSERT INTO users (id, username, email, password_hash) VALUES ($1, $2, $3, $4)")
            .bind(id)
            .bind(username)
            .bind(email)
            .bind(password_hash)
            .execute(pool)
            .await
            .unwrap();
    }

    /// Stores a reset code we know, as if `requestPasswordReset` had been called.
    async fn add_reset(pool: &PgPool, user_id: &str, minutes_until_expiry: i32) {
        sqlx::query(
            "INSERT INTO password_resets (user_id, code_hash, expires_at)
             VALUES ($1, $2, now() + ($3 * interval '1 minute'))",
        )
        .bind(user_id)
        .bind(hash(CODE, 4).unwrap())
        .bind(minutes_until_expiry)
        .execute(pool)
        .await
        .unwrap();
    }

    const RESET: &str = "mutation($e: String!, $c: String!, $p: String!) {
        resetPassword(email: $e, code: $c, newPassword: $p) }";
    const REQUEST: &str = "mutation($e: String!) { requestPasswordReset(email: $e) }";
    const LOGIN: &str = "mutation($u: String!, $p: String!) {
        logIn(username: $u, password: $p) { viewer { sessionToken } } }";

    async fn reset(schema: &TestSchema, email: &str, code: &str, password: &str) -> async_graphql::Response {
        run(schema, RESET, json!({ "e": email, "c": code, "p": password })).await
    }

    async fn can_log_in(schema: &TestSchema, username: &str, password: &str) -> bool {
        run(schema, LOGIN, json!({ "u": username, "p": password })).await.errors.is_empty()
    }

    async fn reset_rows(pool: &PgPool) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM password_resets")
            .fetch_one(pool)
            .await
            .unwrap()
    }

    fn error_message(resp: &async_graphql::Response) -> String {
        resp.errors.first().map(|e| e.message.clone()).unwrap_or_default()
    }

    #[test]
    fn reset_codes_are_six_digits() {
        for _ in 0..200 {
            let code = new_reset_code();
            assert_eq!(code.len(), 6, "{code}");
            assert!(code.chars().all(|c| c.is_ascii_digit()), "{code}");
        }
    }

    #[test]
    fn emails_are_trimmed_and_lowercased() {
        assert_eq!(normalize_email("  Pawel@Example.COM "), "pawel@example.com");
    }

    #[sqlx::test]
    async fn request_for_unknown_email_succeeds_but_stores_nothing(pool: PgPool) {
        let schema = schema(&pool);

        let resp = run(&schema, REQUEST, json!({ "e": "nobody@example.com" })).await;

        assert!(resp.errors.is_empty(), "{:?}", resp.errors);
        assert_eq!(resp.data.into_json().unwrap()["requestPasswordReset"], true);
        assert_eq!(reset_rows(&pool).await, 0);
    }

    #[sqlx::test]
    async fn request_for_known_email_stores_a_code_regardless_of_case(pool: PgPool) {
        add_user(&pool, "u1", "pawel", "Pawel@Example.com").await;
        let schema = schema(&pool);

        let resp = run(&schema, REQUEST, json!({ "e": " PAWEL@example.com " })).await;

        assert!(resp.errors.is_empty(), "{:?}", resp.errors);
        assert_eq!(reset_rows(&pool).await, 1);
    }

    #[sqlx::test]
    async fn repeated_requests_within_a_minute_keep_the_first_code(pool: PgPool) {
        add_user(&pool, "u1", "pawel", "pawel@example.com").await;
        let schema = schema(&pool);
        run(&schema, REQUEST, json!({ "e": "pawel@example.com" })).await;
        let first: String = sqlx::query_scalar("SELECT code_hash FROM password_resets")
            .fetch_one(&pool)
            .await
            .unwrap();

        run(&schema, REQUEST, json!({ "e": "pawel@example.com" })).await;

        let second: String = sqlx::query_scalar("SELECT code_hash FROM password_resets")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(first, second);
    }

    #[sqlx::test]
    async fn an_old_request_is_replaced_by_a_new_one(pool: PgPool) {
        add_user(&pool, "u1", "pawel", "pawel@example.com").await;
        add_reset(&pool, "u1", 10).await;
        sqlx::query("UPDATE password_resets SET created_at = now() - interval '5 minutes', attempts = 3")
            .execute(&pool)
            .await
            .unwrap();
        let old: String = sqlx::query_scalar("SELECT code_hash FROM password_resets")
            .fetch_one(&pool)
            .await
            .unwrap();

        run(&schema(&pool), REQUEST, json!({ "e": "pawel@example.com" })).await;

        let (new, attempts): (String, i32) =
            sqlx::query_as("SELECT code_hash, attempts FROM password_resets")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_ne!(old, new);
        assert_eq!(attempts, 0);
    }

    #[sqlx::test]
    async fn correct_code_changes_the_password(pool: PgPool) {
        add_user(&pool, "u1", "pawel", "pawel@example.com").await;
        add_reset(&pool, "u1", 10).await;
        let schema = schema(&pool);

        let resp = reset(&schema, "pawel@example.com", CODE, NEW_PASSWORD).await;

        assert!(resp.errors.is_empty(), "{:?}", resp.errors);
        assert!(can_log_in(&schema, "pawel", NEW_PASSWORD).await);
        assert!(!can_log_in(&schema, "pawel", OLD_PASSWORD).await);
        assert_eq!(reset_rows(&pool).await, 0, "the code must be single-use");
    }

    #[sqlx::test]
    async fn the_code_works_only_once(pool: PgPool) {
        add_user(&pool, "u1", "pawel", "pawel@example.com").await;
        add_reset(&pool, "u1", 10).await;
        let schema = schema(&pool);
        reset(&schema, "pawel@example.com", CODE, NEW_PASSWORD).await;

        let again = reset(&schema, "pawel@example.com", CODE, "another-password").await;

        assert_eq!(error_message(&again), "Invalid or expired code");
        assert!(can_log_in(&schema, "pawel", NEW_PASSWORD).await);
    }

    #[sqlx::test]
    async fn resetting_logs_out_existing_sessions(pool: PgPool) {
        add_user(&pool, "u1", "pawel", "pawel@example.com").await;
        sqlx::query("INSERT INTO sessions (token, user_id) VALUES ('old-session', 'u1')")
            .execute(&pool)
            .await
            .unwrap();
        add_reset(&pool, "u1", 10).await;

        reset(&schema(&pool), "pawel@example.com", CODE, NEW_PASSWORD).await;

        let sessions: i64 = sqlx::query_scalar("SELECT count(*) FROM sessions WHERE user_id = 'u1'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(sessions, 0);
    }

    #[sqlx::test]
    async fn wrong_code_is_rejected_and_the_password_is_unchanged(pool: PgPool) {
        add_user(&pool, "u1", "pawel", "pawel@example.com").await;
        add_reset(&pool, "u1", 10).await;
        let schema = schema(&pool);

        let resp = reset(&schema, "pawel@example.com", "000000", NEW_PASSWORD).await;

        assert_eq!(error_message(&resp), "Invalid or expired code");
        assert!(can_log_in(&schema, "pawel", OLD_PASSWORD).await);
        assert!(!can_log_in(&schema, "pawel", NEW_PASSWORD).await);
    }

    #[sqlx::test]
    async fn five_wrong_guesses_lock_the_code_even_for_the_right_one(pool: PgPool) {
        add_user(&pool, "u1", "pawel", "pawel@example.com").await;
        add_reset(&pool, "u1", 10).await;
        let schema = schema(&pool);

        for _ in 0..RESET_MAX_ATTEMPTS {
            let resp = reset(&schema, "pawel@example.com", "000000", NEW_PASSWORD).await;
            assert_eq!(error_message(&resp), "Invalid or expired code");
        }
        let resp = reset(&schema, "pawel@example.com", CODE, NEW_PASSWORD).await;

        assert_eq!(error_message(&resp), "Invalid or expired code");
        assert!(can_log_in(&schema, "pawel", OLD_PASSWORD).await);
    }

    #[sqlx::test]
    async fn expired_code_is_rejected(pool: PgPool) {
        add_user(&pool, "u1", "pawel", "pawel@example.com").await;
        add_reset(&pool, "u1", -1).await;

        let resp = reset(&schema(&pool), "pawel@example.com", CODE, NEW_PASSWORD).await;

        assert_eq!(error_message(&resp), "Invalid or expired code");
    }

    #[sqlx::test]
    async fn unknown_email_gets_the_same_error_as_a_wrong_code(pool: PgPool) {
        let resp = reset(&schema(&pool), "nobody@example.com", CODE, NEW_PASSWORD).await;

        assert_eq!(error_message(&resp), "Invalid or expired code");
    }

    #[sqlx::test]
    async fn short_password_is_rejected_without_using_up_the_code(pool: PgPool) {
        add_user(&pool, "u1", "pawel", "pawel@example.com").await;
        add_reset(&pool, "u1", 10).await;
        let schema = schema(&pool);

        let short = reset(&schema, "pawel@example.com", CODE, "short").await;
        let ok = reset(&schema, "pawel@example.com", CODE, NEW_PASSWORD).await;

        assert!(error_message(&short).contains("at least 8"));
        assert!(ok.errors.is_empty(), "{:?}", ok.errors);
    }

    #[sqlx::test]
    async fn accounts_without_a_password_can_set_one(pool: PgPool) {
        // Google-only accounts have no password hash.
        sqlx::query("INSERT INTO users (id, username, email, password_hash) VALUES ('g1', 'gus', 'gus@example.com', NULL)")
            .execute(&pool)
            .await
            .unwrap();
        add_reset(&pool, "g1", 10).await;
        let schema = schema(&pool);

        let resp = reset(&schema, "gus@example.com", CODE, NEW_PASSWORD).await;

        assert!(resp.errors.is_empty(), "{:?}", resp.errors);
        assert!(can_log_in(&schema, "gus", NEW_PASSWORD).await);
    }

    // ── Google account linking ────────────────────────────────────────────────

    async fn insert_user(pool: &PgPool, id: &str, username: &str, email: &str, hash: Option<&str>) {
        sqlx::query("INSERT INTO users (id, username, email, password_hash) VALUES ($1, $2, $3, $4)")
            .bind(id)
            .bind(username)
            .bind(email)
            .bind(hash)
            .execute(pool)
            .await
            .unwrap();
    }

    async fn count(pool: &PgPool, sql: &str) -> i64 {
        sqlx::query_scalar(sql).fetch_one(pool).await.unwrap()
    }

    #[sqlx::test]
    async fn google_login_links_to_the_existing_account_with_that_email(pool: PgPool) {
        insert_user(&pool, "u1", "pawel", "Pawel@Example.com", Some("x")).await;

        let (id, username) = upsert_social_user(&pool, "google", "g-1", Some("pawel@example.com"))
            .await
            .unwrap();

        assert_eq!((id.as_str(), username.as_str()), ("u1", "pawel"));
        assert_eq!(count(&pool, "SELECT count(*) FROM users").await, 1);
        assert_eq!(count(&pool, "SELECT count(*) FROM auth_providers WHERE user_id = 'u1'").await, 1);
    }

    #[sqlx::test]
    async fn linking_removes_the_old_password_and_sessions(pool: PgPool) {
        let hash = hash(OLD_PASSWORD, 4).unwrap();
        insert_user(&pool, "u1", "pawel", "pawel@example.com", Some(&hash)).await;
        sqlx::query("INSERT INTO sessions (token, user_id) VALUES ('old', 'u1')")
            .execute(&pool)
            .await
            .unwrap();

        upsert_social_user(&pool, "google", "g-1", Some("pawel@example.com")).await.unwrap();

        assert_eq!(count(&pool, "SELECT count(*) FROM sessions").await, 0);
        let schema = schema(&pool);
        assert!(!can_log_in(&schema, "pawel", OLD_PASSWORD).await);
    }

    #[sqlx::test]
    async fn second_google_login_reuses_the_link_and_keeps_sessions(pool: PgPool) {
        insert_user(&pool, "u1", "pawel", "pawel@example.com", None).await;
        upsert_social_user(&pool, "google", "g-1", Some("pawel@example.com")).await.unwrap();
        sqlx::query("INSERT INTO sessions (token, user_id) VALUES ('s', 'u1')")
            .execute(&pool)
            .await
            .unwrap();

        let (id, _) = upsert_social_user(&pool, "google", "g-1", Some("pawel@example.com"))
            .await
            .unwrap();

        assert_eq!(id, "u1");
        assert_eq!(count(&pool, "SELECT count(*) FROM sessions").await, 1);
        assert_eq!(count(&pool, "SELECT count(*) FROM auth_providers").await, 1);
    }

    #[sqlx::test]
    async fn unknown_google_email_creates_a_new_account(pool: PgPool) {
        let (id, username) = upsert_social_user(&pool, "google", "g-1", Some("new.person@example.com"))
            .await
            .unwrap();

        assert_eq!(username, "new.person");
        assert_eq!(count(&pool, &format!("SELECT count(*) FROM users WHERE id = '{id}'")).await, 1);
    }

    #[sqlx::test]
    async fn unverified_google_email_never_touches_an_existing_account(pool: PgPool) {
        // verify_google_token drops unverified emails, so the email arrives as None.
        insert_user(&pool, "u1", "pawel", "pawel@example.com", Some("x")).await;

        let result = upsert_social_user(&pool, "google", "g-1", None).await;

        assert!(result.is_err());
        assert_eq!(count(&pool, "SELECT count(*) FROM users WHERE id = 'u1' AND password_hash = 'x'").await, 1);
    }

    #[sqlx::test]
    async fn ambiguous_email_is_refused_instead_of_guessing(pool: PgPool) {
        insert_user(&pool, "u1", "a", "Same@example.com", Some("x")).await;
        insert_user(&pool, "u2", "b", "same@example.com", Some("x")).await;

        let result = upsert_social_user(&pool, "google", "g-1", Some("same@example.com")).await;

        assert!(result.is_err());
        assert_eq!(count(&pool, "SELECT count(*) FROM auth_providers").await, 0);
    }

    #[sqlx::test]
    async fn password_login_on_a_google_only_account_fails_cleanly(pool: PgPool) {
        insert_user(&pool, "u1", "gus", "gus@example.com", None).await;
        let schema = schema(&pool);

        assert!(!can_log_in(&schema, "gus", "anything").await);
    }

    const CREATE_COLLECTION: &str = "mutation($n: String!, $u: String!) {
        createCollection(input: { fields: { name: $n, language: \"pl|en\", user: { link: $u } } }) {
            collection { id name } } }";
    const UPDATE_COLLECTION: &str = "mutation($id: String!, $n: String!) {
        updateCollection(input: { id: $id, fields: { name: $n, language: \"pl|en\" } }) {
            collection { id name } } }";

    async fn create_collection(schema: &TestSchema, user: &str, name: &str) -> async_graphql::Response {
        run(schema, CREATE_COLLECTION, json!({ "n": name, "u": user })).await
    }

    fn collection_id(resp: &async_graphql::Response) -> String {
        resp.data.clone().into_json().unwrap()["createCollection"]["collection"]["id"]
            .as_str()
            .unwrap()
            .to_string()
    }

    #[sqlx::test]
    async fn a_second_collection_with_the_same_name_is_refused(pool: PgPool) {
        add_user(&pool, "u1", "ann", "ann@example.com").await;
        let schema = schema(&pool);

        assert!(create_collection(&schema, "u1", "Food").await.errors.is_empty());
        let again = create_collection(&schema, "u1", "Food").await;

        assert_eq!(error_message(&again), "You already have a collection named \"Food\".");
        assert_eq!(count(&pool, "SELECT count(*) FROM collections").await, 1);
    }

    #[sqlx::test]
    async fn names_differing_only_by_case_or_spaces_count_as_the_same(pool: PgPool) {
        add_user(&pool, "u1", "ann", "ann@example.com").await;
        let schema = schema(&pool);

        assert!(create_collection(&schema, "u1", "Food").await.errors.is_empty());

        assert!(!create_collection(&schema, "u1", "food").await.errors.is_empty());
        assert!(!create_collection(&schema, "u1", "  FOOD ").await.errors.is_empty());
        assert_eq!(count(&pool, "SELECT count(*) FROM collections").await, 1);
    }

    #[sqlx::test]
    async fn different_users_may_use_the_same_name(pool: PgPool) {
        add_user(&pool, "u1", "ann", "ann@example.com").await;
        add_user(&pool, "u2", "bob", "bob@example.com").await;
        let schema = schema(&pool);

        assert!(create_collection(&schema, "u1", "Food").await.errors.is_empty());
        assert!(create_collection(&schema, "u2", "Food").await.errors.is_empty());
    }

    #[sqlx::test]
    async fn the_name_is_stored_trimmed_and_blank_is_refused(pool: PgPool) {
        add_user(&pool, "u1", "ann", "ann@example.com").await;
        let schema = schema(&pool);

        assert!(create_collection(&schema, "u1", "  Food  ").await.errors.is_empty());
        let stored: String = sqlx::query_scalar("SELECT name FROM collections")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(stored, "Food");

        let blank = create_collection(&schema, "u1", "   ").await;
        assert_eq!(error_message(&blank), "Collection name can't be empty.");
    }

    #[sqlx::test]
    async fn renaming_onto_another_collections_name_is_refused(pool: PgPool) {
        add_user(&pool, "u1", "ann", "ann@example.com").await;
        let schema = schema(&pool);
        create_collection(&schema, "u1", "Food").await;
        let travel = collection_id(&create_collection(&schema, "u1", "Travel").await);

        let clash = run(&schema, UPDATE_COLLECTION, json!({ "id": travel, "n": "food" })).await;

        assert_eq!(error_message(&clash), "You already have a collection named \"food\".");
    }

    #[sqlx::test]
    async fn a_collection_can_keep_its_own_name_when_edited(pool: PgPool) {
        add_user(&pool, "u1", "ann", "ann@example.com").await;
        let schema = schema(&pool);
        let food = collection_id(&create_collection(&schema, "u1", "Food").await);

        let same = run(&schema, UPDATE_COLLECTION, json!({ "id": food, "n": "Food" })).await;
        let recase = run(&schema, UPDATE_COLLECTION, json!({ "id": food, "n": "FOOD" })).await;

        assert!(same.errors.is_empty(), "{:?}", same.errors);
        assert!(recase.errors.is_empty(), "{:?}", recase.errors);
    }

    #[sqlx::test]
    async fn the_name_can_be_reused_after_deleting(pool: PgPool) {
        add_user(&pool, "u1", "ann", "ann@example.com").await;
        let schema = schema(&pool);
        let food = collection_id(&create_collection(&schema, "u1", "Food").await);
        sqlx::query("DELETE FROM collections WHERE id = $1").bind(&food).execute(&pool).await.unwrap();

        assert!(create_collection(&schema, "u1", "Food").await.errors.is_empty());
    }

    #[sqlx::test]
    async fn the_migration_renames_existing_duplicates_instead_of_failing(pool: PgPool) {
        add_user(&pool, "u1", "ann", "ann@example.com").await;
        add_user(&pool, "u2", "bob", "bob@example.com").await;
        sqlx::query("DROP INDEX collections_user_name_unique").execute(&pool).await.unwrap();
        for (id, user, name, age_days) in [
            ("a", "u1", "Food", 3),
            ("b", "u1", "food ", 2),
            ("c", "u1", "FOOD", 1),
            ("d", "u1", "Food (2)", 0),
            ("e", "u2", "Food", 0),
        ] {
            sqlx::query(
                "INSERT INTO collections (id, name, language, user_id, created_at)
                 VALUES ($1, $2, 'pl|en', $3, now() - ($4 * interval '1 day'))",
            )
            .bind(id).bind(name).bind(user).bind(age_days)
            .execute(&pool).await.unwrap();
        }

        sqlx::raw_sql(include_str!("../migrations/0006_unique_collection_names.sql"))
            .execute(&pool)
            .await
            .unwrap();

        let names: Vec<(String, String)> =
            sqlx::query_as("SELECT id, name FROM collections ORDER BY id").fetch_all(&pool).await.unwrap();
        let name = |id: &str| names.iter().find(|(i, _)| i == id).unwrap().1.clone();
        assert_eq!(name("a"), "Food", "the oldest keeps its name");
        assert_eq!(name("b"), "food (2)");
        assert_eq!(name("c"), "FOOD (3)");
        assert!(name("d").starts_with("Food (2)"), "{}", name("d"));
        assert_eq!(name("e"), "Food", "other users are untouched");
    }
}
