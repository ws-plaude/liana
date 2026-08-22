//! Async wrapper around the blocking `minreq` http client.
//!
//! Requests are sent from a dedicated thread so callers stay async. That thread cannot be
//! cancelled, so every request carries a timeout to make sure it eventually releases it.

use std::fmt;

use futures::channel::oneshot;
use serde::{de::DeserializeOwned, Serialize};

pub use minreq::Method;

/// Timeout of every request sent through this module, in seconds.
pub const TIMEOUT_SECS: u64 = 30;

const CONTENT_TYPE: &str = "Content-Type";

/// Information about an unsuccessful response.
#[derive(Debug, Clone)]
pub struct NotSuccessResponseInfo {
    pub status_code: u16,
    pub text: String,
}

#[derive(Debug, Clone)]
pub enum Error {
    /// The request could not be sent, or its response could not be read.
    Request(String),
    /// A body could not be serialized or deserialized.
    Body(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Request(e) => write!(f, "Request failed: {e}"),
            Self::Body(e) => write!(f, "Cannot handle body: {e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<minreq::Error> for Error {
    fn from(value: minreq::Error) -> Self {
        Self::Request(value.to_string())
    }
}

/// Http client holding the headers shared by all the requests it builds.
#[derive(Debug, Clone, Default)]
pub struct Client {
    headers: Vec<(String, String)>,
}

impl Client {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a header sent along every request built by this client.
    pub fn header<K: Into<String>, V: Into<String>>(mut self, key: K, value: V) -> Self {
        self.headers.push((key.into(), value.into()));
        self
    }

    pub fn request<U: Into<String>>(&self, method: Method, url: U) -> Request {
        Request {
            method,
            url: url.into(),
            headers: self.headers.clone(),
            params: Vec::new(),
            body: None,
        }
    }
}

/// A request being built. A body that fails to serialize is reported by `send`.
pub struct Request {
    method: Method,
    url: String,
    headers: Vec<(String, String)>,
    params: Vec<(String, String)>,
    body: Option<Result<Vec<u8>, Error>>,
}

impl Request {
    pub fn header<K: Into<String>, V: Into<String>>(mut self, key: K, value: V) -> Self {
        self.headers.push((key.into(), value.into()));
        self
    }

    /// Appends the given key/value pairs to the url query.
    pub fn query<K: AsRef<str>, V: AsRef<str>>(mut self, params: &[(K, V)]) -> Self {
        self.params.extend(
            params
                .iter()
                .map(|(k, v)| (k.as_ref().to_string(), v.as_ref().to_string())),
        );
        self
    }

    /// Sets `body` serialized as JSON as the request body, and the JSON content type unless the
    /// caller already set one.
    pub fn json<T: Serialize>(mut self, body: &T) -> Self {
        if !self
            .headers
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case(CONTENT_TYPE))
        {
            self.headers
                .push((CONTENT_TYPE.to_string(), "application/json".to_string()));
        }
        self.body = Some(serde_json::to_vec(body).map_err(|e| Error::Body(e.to_string())));
        self
    }

    pub async fn send(self) -> Result<Response, Error> {
        let mut request = minreq::Request::new(self.method, self.url)
            .with_timeout(TIMEOUT_SECS)
            .with_headers(self.headers);
        for (key, value) in &self.params {
            request = request.with_param(key, value);
        }
        if let Some(body) = self.body {
            request = request.with_body(body?);
        }
        log::debug!("Sending http request: {request:?}");
        let (tx, rx) = oneshot::channel();
        std::thread::spawn(move || {
            let _ = tx.send(request.send());
        });
        let response = rx.await.map_err(|e| Error::Request(e.to_string()))??;
        Ok(Response { inner: response })
    }
}

#[derive(Debug)]
pub struct Response {
    inner: minreq::Response,
}

impl Response {
    pub fn status_code(&self) -> u16 {
        self.inner.status_code
    }

    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.inner.status_code)
    }

    pub fn text(&self) -> Result<String, Error> {
        self.inner
            .as_str()
            .map(str::to_string)
            .map_err(|e| Error::Body(e.to_string()))
    }

    pub fn json<T: DeserializeOwned>(&self) -> Result<T, Error> {
        self.inner.json().map_err(|e| Error::Body(e.to_string()))
    }

    /// Fails unless the server answered with a 2xx status.
    pub fn check_success(self) -> Result<Self, NotSuccessResponseInfo> {
        if self.is_success() {
            return Ok(self);
        }
        Err(NotSuccessResponseInfo {
            status_code: self.inner.status_code,
            text: self
                .text()
                .unwrap_or_else(|_| "Failed to read response text".to_string()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use futures::executor::block_on;
    use std::{
        io::{BufRead, BufReader, Read, Write},
        net::TcpListener,
        sync::mpsc,
    };

    use serde::Deserialize;

    const PAYLOAD: &str = r#"{"name":"liana","count":3}"#;

    #[derive(Debug, Deserialize, Serialize)]
    struct Payload {
        name: String,
        count: u8,
    }

    /// Serves one canned response and hands the request it received back over the channel.
    fn serve_once(status_line: &str, body: &str) -> (String, mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let url = format!("http://{}", listener.local_addr().expect("addr"));
        let response = format!(
            "{status_line}\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        let (tx, rx) = mpsc::channel();

        std::thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept");
            let mut reader = BufReader::new(stream);
            let mut request = String::new();
            let mut body_len = 0;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).expect("read line") == 0 {
                    break;
                }
                if let Some(value) = line.strip_prefix("Content-Length: ") {
                    body_len = value.trim().parse().expect("content length");
                }
                request.push_str(&line);
                if line == "\r\n" {
                    break;
                }
            }
            let mut body = vec![0u8; body_len];
            reader.read_exact(&mut body).expect("read body");
            request.push_str(&String::from_utf8(body).expect("utf8 body"));
            tx.send(request).expect("report request");
            reader
                .into_inner()
                .write_all(response.as_bytes())
                .expect("write response");
        });

        (url, rx)
    }

    #[test]
    fn get_sends_headers_and_query_then_deserializes_the_body() {
        block_on(async {
            let (url, requests) = serve_once("HTTP/1.1 200 OK", PAYLOAD);

            let response = Client::new()
                .header("User-Agent", "liana-test")
                .request(Method::Get, format!("{url}/v1/keys"))
                .query(&[("token", "42-one-two-three")])
                .send()
                .await
                .expect("send")
                .check_success()
                .expect("success");

            assert_eq!(response.status_code(), 200);
            assert_eq!(response.text().expect("text"), PAYLOAD);
            let payload: Payload = response.json().expect("json");
            assert_eq!(payload.name, "liana");
            assert_eq!(payload.count, 3);

            let request = requests.recv().expect("request");
            assert!(request.starts_with("GET /v1/keys?token=42-one-two-three HTTP/1.1\r\n"));
            assert!(request.contains("User-Agent: liana-test\r\n"));
        });
    }

    #[test]
    fn post_sends_the_json_body_under_a_single_content_type() {
        block_on(async {
            let (url, requests) = serve_once("HTTP/1.1 200 OK", "{}");

            Client::new()
                .header("Content-Type", "application/json")
                .request(Method::Post, format!("{url}/v1/keys/redeem"))
                .json(&Payload {
                    name: "liana".to_string(),
                    count: 3,
                })
                .send()
                .await
                .expect("send");

            let request = requests.recv().expect("request");
            assert!(request.starts_with("POST /v1/keys/redeem HTTP/1.1\r\n"));
            assert_eq!(
                request
                    .matches("Content-Type: application/json\r\n")
                    .count(),
                1
            );
            assert!(request.contains("Content-Length: 26\r\n"));
            assert!(request.ends_with(PAYLOAD));
        });
    }

    #[test]
    fn not_success_response_carries_the_status_and_the_text() {
        block_on(async {
            let (url, _requests) = serve_once("HTTP/1.1 404 Not Found", "wallet not found");

            let response = Client::new()
                .request(Method::Get, format!("{url}/v1/wallets"))
                .send()
                .await
                .expect("send");

            assert_eq!(response.status_code(), 404);
            let info = response.check_success().expect_err("not a success");
            assert_eq!(info.status_code, 404);
            assert_eq!(info.text, "wallet not found");
        });
    }
}
