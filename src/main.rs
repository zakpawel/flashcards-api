mod translator;

use async_graphql::{
    http::GraphiQLSource, Context, InputObject, MergedObject, Object, Schema, SimpleObject,
};
use async_graphql_axum::{GraphQLRequest, GraphQLResponse};
use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::get,
    Json, Router,
};
use translator::SessionToken;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{postgres::PgPoolOptions, PgPool};
use std::env;
use uuid::Uuid;

// ── Build SHA ──────────────────────────────────────────────────────────────────

const GIT_SHA: &str = match option_env!("GIT_SHA") {
    Some(s) => s,
    None => "unknown",
};

// ── DB Model ──────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, sqlx::FromRow)]
struct Flashcard {
    id: Uuid,
    front: String,
    back: String,
    category: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

// ── GraphQL types ─────────────────────────────────────────────────────────────

/// A single flashcard.
#[derive(SimpleObject)]
struct GqlFlashcard {
    id: Uuid,
    front: String,
    back: String,
    category: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl From<Flashcard> for GqlFlashcard {
    fn from(f: Flashcard) -> Self {
        GqlFlashcard {
            id: f.id,
            front: f.front,
            back: f.back,
            category: f.category,
            created_at: f.created_at,
            updated_at: f.updated_at,
        }
    }
}

#[derive(InputObject)]
struct CreateFlashcardInput {
    front: String,
    back: String,
    category: Option<String>,
}

#[derive(InputObject)]
struct UpdateFlashcardInput {
    front: Option<String>,
    back: Option<String>,
    /// Provide a value to set a new category, or omit to keep the existing one.
    category: Option<String>,
    /// Set to `true` to explicitly clear the category (set it to null).
    #[graphql(default)]
    clear_category: bool,
}

// ── GraphQL schema ────────────────────────────────────────────────────────────

#[derive(Default)]
pub struct QueryRoot;

#[Object]
impl QueryRoot {
    /// Fetch all flashcards, optionally filtered by category.
    async fn flashcards(
        &self,
        ctx: &Context<'_>,
        category: Option<String>,
    ) -> async_graphql::Result<Vec<GqlFlashcard>> {
        let pool = ctx.data::<PgPool>()?;
        let cards = match category.as_deref() {
            Some(cat) if !cat.is_empty() => {
                sqlx::query_as!(
                    Flashcard,
                    "SELECT id, front, back, category, created_at, updated_at
                 FROM flashcards
                 WHERE category = $1
                 ORDER BY created_at DESC",
                    cat
                )
                .fetch_all(pool)
                .await?
            }
            _ => {
                sqlx::query_as!(
                    Flashcard,
                    "SELECT id, front, back, category, created_at, updated_at
                 FROM flashcards
                 ORDER BY created_at DESC"
                )
                .fetch_all(pool)
                .await?
            }
        };
        Ok(cards.into_iter().map(GqlFlashcard::from).collect())
    }

    /// Fetch a single flashcard by its UUID.
    async fn flashcard(
        &self,
        ctx: &Context<'_>,
        id: Uuid,
    ) -> async_graphql::Result<Option<GqlFlashcard>> {
        let pool = ctx.data::<PgPool>()?;
        let card = sqlx::query_as!(
            Flashcard,
            "SELECT id, front, back, category, created_at, updated_at
         FROM flashcards WHERE id = $1",
            id
        )
        .fetch_optional(pool)
        .await?;
        Ok(card.map(GqlFlashcard::from))
    }
}

#[derive(Default)]
pub struct MutationRoot;

#[Object]
impl MutationRoot {
    /// Create a new flashcard.
    async fn create_flashcard(
        &self,
        ctx: &Context<'_>,
        input: CreateFlashcardInput,
    ) -> async_graphql::Result<GqlFlashcard> {
        let pool = ctx.data::<PgPool>()?;
        let card = sqlx::query_as!(
            Flashcard,
            "INSERT INTO flashcards (front, back, category)
         VALUES ($1, $2, $3)
         RETURNING id, front, back, category, created_at, updated_at",
            input.front,
            input.back,
            input.category
        )
        .fetch_one(pool)
        .await?;
        Ok(GqlFlashcard::from(card))
    }

    /// Update an existing flashcard. Returns `null` if the ID is not found.
    async fn update_flashcard(
        &self,
        ctx: &Context<'_>,
        id: Uuid,
        input: UpdateFlashcardInput,
    ) -> async_graphql::Result<Option<GqlFlashcard>> {
        let pool = ctx.data::<PgPool>()?;

        let existing = sqlx::query_as!(
            Flashcard,
            "SELECT id, front, back, category, created_at, updated_at
         FROM flashcards WHERE id = $1",
            id
        )
        .fetch_optional(pool)
        .await?;

        let Some(existing) = existing else {
            return Ok(None);
        };

        let new_front = input.front.unwrap_or(existing.front);
        let new_back = input.back.unwrap_or(existing.back);
        let new_category = if input.clear_category {
            None
        } else {
            input.category.or(existing.category)
        };

        let updated = sqlx::query_as!(
            Flashcard,
            "UPDATE flashcards
         SET front = $1, back = $2, category = $3, updated_at = now()
         WHERE id = $4
         RETURNING id, front, back, category, created_at, updated_at",
            new_front,
            new_back,
            new_category,
            id
        )
        .fetch_one(pool)
        .await?;

        Ok(Some(GqlFlashcard::from(updated)))
    }

    /// Delete a flashcard by ID. Returns `true` if it existed, `false` otherwise.
    async fn delete_flashcard(
        &self,
        ctx: &Context<'_>,
        id: Uuid,
    ) -> async_graphql::Result<bool> {
        let pool = ctx.data::<PgPool>()?;
        let result = sqlx::query!("DELETE FROM flashcards WHERE id = $1", id)
            .execute(pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }
}

#[derive(MergedObject, Default)]
pub struct CombinedQuery(QueryRoot, translator::TranslatorQuery);

#[derive(MergedObject, Default)]
pub struct CombinedMutation(MutationRoot, translator::TranslatorMutation);

pub type AppSchema = Schema<CombinedQuery, CombinedMutation, async_graphql::EmptySubscription>;

// ── Shared app state ──────────────────────────────────────────────────────────

#[derive(Clone)]
struct AppState {
    pool: PgPool,
    schema: AppSchema,
}

// ── GraphQL handlers ──────────────────────────────────────────────────────────

async fn graphql_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    req: GraphQLRequest,
) -> GraphQLResponse {
    let mut request = req.into_inner();
    if let Some(token) = headers
        .get("x-parse-session-token")
        .and_then(|v| v.to_str().ok())
    {
        request = request.data(SessionToken(token.to_string()));
    }
    state.schema.execute(request).await.into()
}

async fn graphiql_handler() -> impl IntoResponse {
    Html(GraphiQLSource::build().endpoint("/graphql").finish())
}

// ── REST request / query types ────────────────────────────────────────────────

#[derive(Deserialize)]
struct CategoryQuery {
    category: Option<String>,
}

#[derive(Deserialize)]
struct CreateFlashcard {
    front: String,
    back: String,
    category: Option<String>,
}

#[derive(Deserialize)]
struct UpdateFlashcard {
    front: Option<String>,
    back: Option<String>,
    /// `null` in JSON clears the category; omitting the key keeps it unchanged.
    #[serde(default, deserialize_with = "deserialize_optional_field")]
    category: MaybeUpdate<Option<String>>,
}

/// Distinguish between a field being absent vs explicitly set (to null or a value).
#[derive(Debug)]
enum MaybeUpdate<T> {
    Absent,
    Present(T),
}

impl<T> Default for MaybeUpdate<T> {
    fn default() -> Self {
        MaybeUpdate::Absent
    }
}

fn deserialize_optional_field<'de, T, D>(d: D) -> std::result::Result<MaybeUpdate<T>, D::Error>
where
    T: Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    T::deserialize(d).map(MaybeUpdate::Present)
}

// ── Error helper ──────────────────────────────────────────────────────────────

struct AppError(anyhow::Error);

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": self.0.to_string() })),
        )
            .into_response()
    }
}

impl<E: Into<anyhow::Error>> From<E> for AppError {
    fn from(e: E) -> Self {
        AppError(e.into())
    }
}

type Result<T> = std::result::Result<T, AppError>;

// ── REST handlers ─────────────────────────────────────────────────────────────

async fn health() -> &'static str {
    "Flashcards API is running"
}

async fn version() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "commit": GIT_SHA }))
}

async fn list_flashcards(
    State(state): State<AppState>,
    Query(params): Query<CategoryQuery>,
) -> Result<Json<Vec<Flashcard>>> {
    let pool = &state.pool;
    let cards = match params.category.as_deref() {
        Some(cat) if !cat.is_empty() => {
            sqlx::query_as!(
                Flashcard,
                "SELECT id, front, back, category, created_at, updated_at
                 FROM flashcards
                 WHERE category = $1
                 ORDER BY created_at DESC",
                cat
            )
            .fetch_all(pool)
            .await?
        }
        _ => {
            sqlx::query_as!(
                Flashcard,
                "SELECT id, front, back, category, created_at, updated_at
                 FROM flashcards
                 ORDER BY created_at DESC"
            )
            .fetch_all(pool)
            .await?
        }
    };
    Ok(Json(cards))
}

async fn get_flashcard(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    let pool = &state.pool;
    let card = sqlx::query_as!(
        Flashcard,
        "SELECT id, front, back, category, created_at, updated_at
         FROM flashcards WHERE id = $1",
        id
    )
    .fetch_optional(pool)
    .await?;

    Ok(match card {
        Some(f) => Json(f).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": format!("Flashcard {} not found", id) })),
        )
            .into_response(),
    })
}

async fn create_flashcard(
    State(state): State<AppState>,
    Json(body): Json<CreateFlashcard>,
) -> Result<Response> {
    let pool = &state.pool;
    let card = sqlx::query_as!(
        Flashcard,
        "INSERT INTO flashcards (front, back, category)
         VALUES ($1, $2, $3)
         RETURNING id, front, back, category, created_at, updated_at",
        body.front,
        body.back,
        body.category
    )
    .fetch_one(pool)
    .await?;

    Ok((StatusCode::CREATED, Json(card)).into_response())
}

async fn update_flashcard(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateFlashcard>,
) -> Result<Response> {
    let pool = &state.pool;
    let existing = sqlx::query_as!(
        Flashcard,
        "SELECT id, front, back, category, created_at, updated_at
         FROM flashcards WHERE id = $1",
        id
    )
    .fetch_optional(pool)
    .await?;

    let Some(existing) = existing else {
        return Ok((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": format!("Flashcard {} not found", id) })),
        )
            .into_response());
    };

    let new_front = body.front.unwrap_or(existing.front);
    let new_back = body.back.unwrap_or(existing.back);
    let new_category = match body.category {
        MaybeUpdate::Present(v) => v,
        MaybeUpdate::Absent => existing.category,
    };

    let updated = sqlx::query_as!(
        Flashcard,
        "UPDATE flashcards
         SET front = $1, back = $2, category = $3, updated_at = now()
         WHERE id = $4
         RETURNING id, front, back, category, created_at, updated_at",
        new_front,
        new_back,
        new_category,
        id
    )
    .fetch_one(pool)
    .await?;

    Ok(Json(updated).into_response())
}

async fn delete_flashcard(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    let pool = &state.pool;
    let result = sqlx::query!("DELETE FROM flashcards WHERE id = $1", id)
        .execute(pool)
        .await?;

    Ok(if result.rows_affected() > 0 {
        StatusCode::NO_CONTENT.into_response()
    } else {
        (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": format!("Flashcard {} not found", id) })),
        )
            .into_response()
    })
}

// ── Main ──────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    let database_url = env::var("DATABASE_URL").unwrap_or_else(|_| {
        let host = env::var("DB_HOST").unwrap_or_else(|_| "localhost".into());
        let port = env::var("DB_PORT").unwrap_or_else(|_| "5432".into());
        let name = env::var("DB_NAME").unwrap_or_else(|_| "flashcards".into());
        let user = env::var("DB_USER").unwrap_or_else(|_| "postgres".into());
        let pass = env::var("DB_PASSWORD").unwrap_or_else(|_| "postgres".into());
        format!("postgres://{user}:{pass}@{host}:{port}/{name}")
    });

    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(&database_url)
        .await?;

    // Run migrations
    sqlx::migrate!("./migrations").run(&pool).await?;

    // Build GraphQL schema — PgPool is injected as context data
    let schema = Schema::build(
        CombinedQuery::default(),
        CombinedMutation::default(),
        async_graphql::EmptySubscription,
    )
    .data(pool.clone())
    .finish();

    let state = AppState { pool, schema };

    let app = Router::new()
        // ── Health / meta ──
        .route("/", get(health))
        .route("/version", get(version))
        // ── REST API ──
        .route(
            "/api/v1/flashcards",
            get(list_flashcards).post(create_flashcard),
        )
        .route(
            "/api/v1/flashcards/{id}",
            get(get_flashcard)
                .patch(update_flashcard)
                .delete(delete_flashcard),
        )
        // ── GraphQL (POST = execute, GET = GraphiQL IDE) ──
        .route(
            "/graphql",
            get(graphiql_handler).post(graphql_handler),
        )
        .with_state(state);

    let port = env::var("PORT").unwrap_or_else(|_| "8080".into());
    let addr = format!("0.0.0.0:{port}");
    tracing::info!("Listening on {addr}");
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
