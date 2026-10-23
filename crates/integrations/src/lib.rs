//! Clients for external services. Every HTTP request carries a descriptive User-Agent, as the
//! Trackmania Exchange and Nadeo API guidelines ask.

pub mod discord;
pub mod export;
pub mod nadeo;
pub mod tmx;
pub mod trackmania;

/// Errors of external services.
#[derive(Debug, thiserror::Error)]
pub enum IntegrationError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("{service} returned HTTP {status}: {body}")]
    Status {
        service: &'static str,
        status: u16,
        body: String,
    },
    #[error("{0}")]
    NotFound(String),
    #[error("invalid response from {service}: {message}")]
    Invalid {
        service: &'static str,
        message: String,
    },
    #[error("missing credentials for {0}")]
    MissingCredentials(&'static str),
}

/// Default User-Agent.
pub fn user_agent() -> String {
    format!(
        "PitWall/{} (Trackmania server controller; +https://github.com/MaKSiiMe/PitWall)",
        env!("CARGO_PKG_VERSION")
    )
}

/// A shared HTTP client with timeouts and our User-Agent.
pub fn http_client() -> Result<reqwest::Client, IntegrationError> {
    Ok(reqwest::Client::builder()
        .user_agent(user_agent())
        .timeout(std::time::Duration::from_secs(30))
        .connect_timeout(std::time::Duration::from_secs(10))
        .build()?)
}

/// Fails on non-2xx statuses, keeping the beginning of the body for the error message.
pub(crate) async fn check(
    service: &'static str,
    response: reqwest::Response,
) -> Result<reqwest::Response, IntegrationError> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let body = response.text().await.unwrap_or_default();
    Err(IntegrationError::Status {
        service,
        status: status.as_u16(),
        body: body.chars().take(300).collect(),
    })
}
