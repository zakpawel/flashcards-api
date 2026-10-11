# flashcards-api

GraphQL backend (Rust) for the WordTranslator / flashcards apps.

Deployed to Render from the Docker image `ghcr.io/zakpawel/flashcards-api`
by the GitHub Actions workflow in `.github/workflows/ci.yml`.

Built with Rust and GraphQL.

## Password reset

`requestPasswordReset(email)` emails a 6-digit code (valid 15 minutes, one active code per
account, at most one email per minute). `resetPassword(email, code, newPassword)` sets the new
password (minimum 8 characters), ends all of the account's sessions, and uses the code up.
After 5 wrong guesses the code is locked. Both calls give the same answer whether or not an
account exists, so they cannot be used to look up who is registered.

Environment variables:

| Variable | Purpose |
|---|---|
| `BREVO_API_KEY` | Brevo API key (Brevo → SMTP & API → API keys) |
| `MAIL_FROM` | Sender address, verified in Brevo |
| `MAIL_FROM_NAME` | Optional display name (default `WordTranslator`) |
| `DEV_LOG_RESET_CODES` | `true` logs codes instead of needing email — local development only |

## Tests

The tests that touch the database need a Postgres server:

```sh
docker run -d --name flashcards-test-pg -e POSTGRES_PASSWORD=postgres -p 55432:5432 postgres:16-alpine
SQLX_OFFLINE=true DATABASE_URL=postgres://postgres:postgres@localhost:55432/postgres cargo test
```
