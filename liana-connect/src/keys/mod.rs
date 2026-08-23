pub mod api;
pub mod token;

use serde_json::json;

use crate::http::{self, Method, NotSuccessResponseInfo};

const KEYS_API_URL: &str = "https://keys.wizardsardine.com";

#[derive(Debug, Clone)]
pub enum Error {
    Http(Option<u16>, String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::Http(kind, e) => write!(f, "Http error: [{kind:?}] {e}"),
        }
    }
}

impl From<http::Error> for Error {
    fn from(error: http::Error) -> Self {
        Self::Http(None, error.to_string())
    }
}

impl From<NotSuccessResponseInfo> for Error {
    fn from(value: NotSuccessResponseInfo) -> Self {
        Self::Http(Some(value.status_code), value.text)
    }
}

#[derive(Debug, Clone)]
pub struct Client {
    http: http::Client,
    url: String,
}

impl Client {
    pub fn new(user_agent: &str) -> Self {
        Self::new_with_url(KEYS_API_URL, user_agent)
    }

    pub fn new_with_url(url: &str, user_agent: &str) -> Self {
        Client {
            http: http::Client::new()
                .header("Content-Type", "application/json")
                .header("API-Version", "0.1")
                .header("User-Agent", user_agent),
            url: url.to_string(),
        }
    }

    pub fn new_with_optional_url(url: Option<&str>, user_agent: &str) -> Self {
        match url {
            Some(url) => Self::new_with_url(url, user_agent),
            None => Self::new(user_agent),
        }
    }

    pub async fn get_key_by_token(&self, token: String) -> Result<api::Key, Error> {
        let response = self
            .http
            .request(Method::Get, format!("{}/v1/keys", self.url))
            .query(&[("token", token)])
            .send()
            .await?
            .check_success()?;
        let key = response.json()?;
        Ok(key)
    }

    pub async fn redeem_key(&self, uuid: String, token: String) -> Result<api::Key, Error> {
        let response = self
            .http
            .request(
                Method::Post,
                format!("{}/v1/keys/{}/redeem", self.url, uuid),
            )
            .json(&json!({
                "token": token,
            }))
            .send()
            .await?
            .check_success()?;
        let key = response.json()?;
        Ok(key)
    }
}
