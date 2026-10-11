//! Outgoing email through Brevo's HTTP API.
//!
//! An HTTP API is used instead of SMTP because Render's free tier blocks outbound SMTP ports.
//! Configure with `BREVO_API_KEY` and `MAIL_FROM` (a sender address verified in Brevo);
//! `MAIL_FROM_NAME` is optional. Without them, mail is not sent and a warning is logged.

use std::env;

const BREVO_URL: &str = "https://api.brevo.com/v3/smtp/email";

#[derive(Clone)]
pub struct Mailer {
    client: reqwest::Client,
    config: Option<MailConfig>,
    /// Local development only: log reset codes instead of (or in addition to) emailing them.
    log_codes: bool,
}

#[derive(Clone)]
struct MailConfig {
    api_key: String,
    from_email: String,
    from_name: String,
}

impl Mailer {
    pub fn from_env() -> Self {
        let config = match (env::var("BREVO_API_KEY"), env::var("MAIL_FROM")) {
            (Ok(api_key), Ok(from_email)) if !api_key.is_empty() && !from_email.is_empty() => {
                Some(MailConfig {
                    api_key,
                    from_email,
                    from_name: env::var("MAIL_FROM_NAME").unwrap_or_else(|_| "WordTranslator".into()),
                })
            }
            _ => None,
        };
        if config.is_none() {
            tracing::warn!("BREVO_API_KEY / MAIL_FROM not set: password reset emails will not be sent");
        }
        let log_codes = env::var("DEV_LOG_RESET_CODES").map(|v| v == "true").unwrap_or(false);
        Self { client: reqwest::Client::new(), config, log_codes }
    }

    /// Sends the reset code. Failures are logged, never returned to the caller, so that the
    /// API response cannot reveal whether an address has an account.
    pub async fn send_reset_code(&self, to_email: &str, username: &str, code: &str) {
        if self.log_codes {
            tracing::warn!("DEV_LOG_RESET_CODES: reset code for {to_email} is {code}");
        }
        let Some(config) = &self.config else { return };

        let body = serde_json::json!({
            "sender": { "name": config.from_name, "email": config.from_email },
            "to": [{ "email": to_email }],
            "subject": "Your WordTranslator password reset code",
            "textContent": format!(
                "Hi {username},\n\nYour password reset code is: {code}\n\n\
                 It is valid for 15 minutes. If you didn't ask for this, you can ignore this email.\n"
            ),
        });

        match self
            .client
            .post(BREVO_URL)
            .header("api-key", &config.api_key)
            .json(&body)
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => {
                tracing::info!("password reset email sent");
            }
            Ok(resp) => {
                let status = resp.status();
                let text = resp.text().await.unwrap_or_default();
                tracing::error!("Brevo rejected the reset email ({status}): {text}");
            }
            Err(e) => tracing::error!("Could not reach Brevo: {e}"),
        }
    }
}
