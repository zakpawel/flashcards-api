use async_graphql::{Context, InputObject, Object, SimpleObject};
use bcrypt::{hash, verify, DEFAULT_COST};
use rand::distributions::Alphanumeric;
use rand::Rng;
use serde::Deserialize;
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
    #[serde(default)]
    error_description: Option<String>,
}

/// Verify a Google ID token via the tokeninfo endpoint.
/// Returns `(google_uid, email)` on success.
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

    Ok((info.sub, info.email))
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

    // 2. No provider row — create a new user.
    let user_id = new_id();
    let base_username = email
        .map(username_from_email)
        .unwrap_or_else(|| format!("user_{}", new_id()));

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

        let row = sqlx::query!(
            "SELECT id, username, password_hash FROM users WHERE username = $1",
            username
        )
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| async_graphql::Error::new("Invalid username or password"))?;

        let valid = verify(&password, &row.password_hash)
            .map_err(|e| async_graphql::Error::new(e.to_string()))?;
        if !valid {
            return Err(async_graphql::Error::new("Invalid username or password"));
        }

        let token = new_token();
        sqlx::query!(
            "INSERT INTO sessions (token, user_id) VALUES ($1, $2)",
            token,
            row.id
        )
        .execute(pool)
        .await?;

        Ok(LogInResult {
            viewer: GqlViewer {
                session_token: token,
                user: GqlUser { id: row.id, username: row.username },
            },
        })
    }

    /// Create a new collection.
    async fn create_collection(
        &self,
        ctx: &Context<'_>,
        input: CreateCollectionInput,
    ) -> async_graphql::Result<CreateCollectionResult> {
        let pool = ctx.data::<PgPool>()?;
        let f = input.fields;
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
        .map_err(|e| async_graphql::Error::new(e.to_string()))?;

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

        let row = sqlx::query_as!(
            CollectionRow,
            "UPDATE collections SET name = $1, language = $2, updated_at = now()
             WHERE id = $3
             RETURNING id, name, language, user_id",
            input.fields.name,
            input.fields.language,
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
