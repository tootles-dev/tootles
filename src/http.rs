//! API errors describe the failure without printing credentials or response data.
use anyhow::{Result, anyhow, bail};
use reqwest::{Response, header};
use serde::de::DeserializeOwned;
use serde_json::error::Category;
use serde_path_to_error::{Path, Segment};

const MAX_JSON_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

pub(crate) async fn decode_json<T: DeserializeOwned>(response: Response) -> Result<T> {
    decode_json_with_limit(response, MAX_JSON_RESPONSE_BYTES).await
}

async fn decode_json_with_limit<T: DeserializeOwned>(
    mut response: Response,
    response_limit: usize,
) -> Result<T> {
    let status = response.status().as_u16();
    let html = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            matches!(
                value
                    .split(';')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .to_ascii_lowercase()
                    .as_str(),
                "text/html" | "application/xhtml+xml"
            )
        });
    if response
        .content_length()
        .is_some_and(|size| size > response_limit as u64)
    {
        bail!("The server response exceeds the client size limit (HTTP {status})");
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|error| {
        if error.is_timeout() {
            anyhow!("Timed out while reading the server response (HTTP {status}); try again")
        } else {
            anyhow!("Could not read the server response (HTTP {status}); the connection may have been interrupted or its compression is invalid")
        }
    })? {
        // reqwest yields transparently decompressed bytes here, so this limit
        // also contains compressed responses whose decoded body expands greatly.
        if bytes.len().saturating_add(chunk.len()) > response_limit {
            bail!("The server response exceeds the client size limit (HTTP {status})");
        }
        bytes.extend_from_slice(&chunk);
    }
    // Some reverse proxies return a successful HTML challenge or sign-in page.
    // Never print that page: it could contain a token or terminal controls.
    if html || bytes.iter().copied().find(|b| !b.is_ascii_whitespace()) == Some(b'<') {
        bail!(
            "The server returned an HTML page instead of Mastodon API JSON (HTTP {status}); check for a proxy, sign-in page, or server protection challenge"
        );
    }
    let mut deserializer = serde_json::Deserializer::from_slice(&bytes);
    let value = serde_path_to_error::deserialize(&mut deserializer).map_err(|error| {
        let inner = error.inner();
        match inner.classify() {
            Category::Data => anyhow!(
                "The server returned an unexpected JSON value at {} (HTTP {status}); this field is incompatible with the Mastodon API format",
                field_path(error.path())
            ),
            Category::Eof => anyhow!(
                "The server returned empty or incomplete JSON (HTTP {status}, line {}, column {}); try again",
                inner.line(), inner.column()
            ),
            _ => anyhow!(
                "The server returned invalid JSON (HTTP {status}, line {}, column {})",
                inner.line(), inner.column()
            ),
        }
    })?;
    // Streaming deserialization alone would silently accept a second document.
    deserializer.end().map_err(|error| {
        anyhow!(
            "The server returned unexpected data after its JSON response (HTTP {status}, line {}, column {})",
            error.line(), error.column()
        )
    })?;
    Ok(value)
}

fn field_path(path: &Path) -> String {
    let mut result = String::from("response");
    for segment in path.iter().take(16) {
        match segment {
            Segment::Seq { index } => result.push_str(&format!("[{index}]")),
            // Our typed API models use named struct fields, not dynamic maps.
            // Never echo enum variant values or a malformed key from a server.
            Segment::Map { key }
                if key.len() <= 64
                    && key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') =>
            {
                result.push('.');
                result.push_str(key);
            }
            _ => result.push_str(".[field]"),
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::time::Duration;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn compressed_json_is_decoded_by_the_http_client() {
        // Deterministic gzip of {"ok":true}; exercises real response decoding.
        let gzip = [
            31, 139, 8, 0, 0, 0, 0, 0, 2, 3, 171, 86, 202, 207, 86, 178, 42, 41, 42, 77, 173, 5, 0,
            144, 95, 212, 167, 11, 0, 0, 0,
        ];
        let server = MockServer::start().await;
        Mock::given(wiremock::matchers::path("/"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-encoding", "gzip")
                    .set_body_raw(gzip, "application/json"),
            )
            .mount(&server)
            .await;
        let response = reqwest::Client::new()
            .get(server.uri())
            .send()
            .await
            .unwrap();
        let json: Value = decode_json_with_limit(response, br#"{"ok":true}"#.len())
            .await
            .unwrap();
        assert_eq!(json, serde_json::json!({"ok":true}));
    }

    #[tokio::test]
    async fn declared_oversized_json_response_is_rejected() {
        const TEST_LIMIT: usize = 64;
        let server = MockServer::start().await;
        Mock::given(wiremock::matchers::path("/"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_raw(vec![b' '; TEST_LIMIT + 1], "application/json"),
            )
            .mount(&server)
            .await;
        let response = reqwest::get(server.uri()).await.unwrap();
        assert_eq!(response.content_length(), Some((TEST_LIMIT + 1) as u64));

        let error = decode_json_with_limit::<Value>(response, TEST_LIMIT)
            .await
            .unwrap_err()
            .to_string();

        assert!(error.contains("exceeds the client size limit"), "{error}");
        assert!(error.contains("HTTP 200"), "{error}");
    }

    #[tokio::test]
    async fn chunked_compressed_json_is_limited_after_decoding() {
        const TEST_LIMIT: usize = 64;
        // Deterministic gzip of a 107-byte JSON document. The 34-byte encoded
        // body is below the test limit, while the decoded body is above it.
        let gzip = [
            31, 139, 8, 0, 0, 0, 0, 0, 2, 255, 171, 86, 42, 73, 173, 40, 81, 178, 82, 74, 164, 49,
            80, 170, 5, 0, 246, 184, 148, 192, 107, 0, 0, 0,
        ];
        assert!(gzip.len() < TEST_LIMIT);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                request.push(stream.read_u8().await.unwrap());
            }
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Encoding: gzip\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n22\r\n",
                )
                .await
                .unwrap();
            stream.write_all(&gzip).await.unwrap();
            stream.write_all(b"\r\n0\r\n\r\n").await.unwrap();
        });
        let response = reqwest::Client::new()
            .get(format!("http://{address}"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.content_length(), None);

        let error = decode_json_with_limit::<Value>(response, TEST_LIMIT)
            .await
            .unwrap_err()
            .to_string();
        server.await.unwrap();

        assert!(error.contains("exceeds the client size limit"), "{error}");
        assert!(error.contains("HTTP 200"), "{error}");
    }

    #[tokio::test]
    async fn trailing_documents_are_not_silently_accepted() {
        let server = MockServer::start().await;
        Mock::given(wiremock::matchers::path("/"))
            .respond_with(
                ResponseTemplate::new(200).set_body_raw("{} private-data", "application/json"),
            )
            .mount(&server)
            .await;
        let response = reqwest::get(server.uri()).await.unwrap();
        let error = decode_json::<Value>(response)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("after its JSON response"));
        assert!(!error.contains("private-data"));
    }

    #[tokio::test]
    async fn body_timeouts_and_disconnects_are_not_reported_as_schema_errors() {
        for timeout in [false, true] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") {
                    request.push(stream.read_u8().await.unwrap());
                }
                stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 100\r\nConnection: close\r\n\r\n{").await.unwrap();
                if timeout {
                    tokio::time::sleep(Duration::from_secs(10)).await;
                }
            });
            let client = reqwest::Client::builder()
                .timeout(Duration::from_millis(500))
                .build()
                .unwrap();
            let response = client
                .get(format!("http://{address}"))
                .send()
                .await
                .unwrap();
            let error = decode_json::<Value>(response)
                .await
                .unwrap_err()
                .to_string();
            server.abort();
            assert!(
                error.contains(if timeout {
                    "Timed out while reading"
                } else {
                    "Could not read the server response"
                }),
                "{error}"
            );
            assert!(error.contains("HTTP 200"));
        }
    }
}
