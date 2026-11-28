// Copyright 2026 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

//! The read-only Mastodon API used by Placehero. No authentication, streaming,
//! or unrelated service models are needed by the current application.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, Result, ensure};
use reqwest::{Client, Response, Url};
use serde::{Deserialize, de::DeserializeOwned};
use time::OffsetDateTime;

const MAX_API_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug)]
pub(crate) struct Mastodon {
    client: Client,
    base: Url,
}

impl Mastodon {
    pub(crate) fn new(base: &str) -> Result<Self> {
        let base = Url::parse(base).context("Invalid Mastodon server URL")?;
        ensure!(
            matches!(base.scheme(), "http" | "https")
                && base.host_str().is_some()
                && base.username().is_empty()
                && base.password().is_none()
                && base.query().is_none()
                && base.fragment().is_none(),
            "Expected a Mastodon HTTP(S) server URL without credentials, query or fragment"
        );
        let client = Client::builder()
            .user_agent("Placehero")
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()?;
        Ok(Self { client, base })
    }

    pub(crate) const fn client(&self) -> &Client {
        &self.client
    }

    fn endpoint(&self, segments: &[&str]) -> Result<Url> {
        ensure!(
            segments
                .iter()
                .all(|segment| !segment.is_empty() && !matches!(*segment, "." | "..")),
            "Invalid empty or relative Mastodon resource identifier"
        );
        let mut url = self.base.clone();
        url.path_segments_mut()
            .map_err(|()| anyhow::anyhow!("Mastodon URL cannot contain path segments"))?
            .pop_if_empty()
            .extend(segments.iter().copied());
        Ok(url)
    }

    async fn get<T: DeserializeOwned>(&self, url: Url) -> Result<T> {
        let response = self.client.get(url).send().await?.error_for_status()?;
        let body = read_body(response, MAX_API_RESPONSE_BYTES).await?;
        serde_json::from_slice(&body).context("Invalid Mastodon API response")
    }

    pub(crate) async fn instance(&self) -> Result<Instance> {
        self.get(self.endpoint(&["api", "v1", "instance"])?).await
    }

    pub(crate) async fn account(&self, acct: &str) -> Result<Account> {
        let mut url = self.endpoint(&["api", "v1", "accounts", "lookup"])?;
        url.query_pairs_mut().append_pair("acct", acct);
        self.get(url).await
    }

    pub(crate) async fn context(&self, id: &str) -> Result<Context> {
        self.get(self.endpoint(&["api", "v1", "statuses", id, "context"])?)
            .await
    }

    pub(crate) async fn statuses(
        &self,
        id: &str,
        max_id: Option<&str>,
    ) -> Result<Vec<Arc<Status>>> {
        let mut url = self.endpoint(&["api", "v1", "accounts", id, "statuses"])?;
        {
            let mut query = url.query_pairs_mut();
            query
                .append_pair("exclude_replies", "true")
                .append_pair("exclude_reblogs", "false");
            if let Some(max_id) = max_id {
                query.append_pair("max_id", max_id);
            }
        }
        self.get(url).await
    }
}

/// Bound the actual streamed bytes even when Content-Length is absent or misleading.
pub(crate) async fn read_body(mut response: Response, max_bytes: usize) -> Result<Vec<u8>> {
    ensure!(
        response
            .content_length()
            .is_none_or(|length| length <= max_bytes as u64),
        "HTTP response exceeds {max_bytes} bytes"
    );
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        ensure!(
            chunk.len() <= max_bytes - body.len(),
            "HTTP response exceeds {max_bytes} bytes"
        );
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

#[derive(Debug, Deserialize)]
pub(crate) struct Instance {
    pub(crate) title: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Account {
    pub(crate) id: String,
    pub(crate) acct: String,
    pub(crate) display_name: String,
    pub(crate) avatar_static: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Status {
    pub(crate) id: String,
    pub(crate) account: Account,
    pub(crate) in_reply_to_id: Option<String>,
    pub(crate) reblog: Option<Arc<Self>>,
    pub(crate) content: String,
    #[serde(with = "time::serde::rfc3339")]
    pub(crate) created_at: OffsetDateTime,
    pub(crate) replies_count: u32,
    pub(crate) reblogs_count: u32,
    pub(crate) favourites_count: u32,
    pub(crate) media_attachments: Vec<Attachment>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Context {
    pub(crate) ancestors: Vec<Arc<Status>>,
    pub(crate) descendants: Vec<Arc<Status>>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Attachment {
    pub(crate) r#type: AttachmentType,
    pub(crate) meta: Option<AttachmentMeta>,
    pub(crate) description: Option<String>,
    pub(crate) blurhash: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum AttachmentType {
    Image,
    Video,
    Gifv,
    Audio,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Deserialize)]
pub(crate) struct AttachmentMeta {
    pub(crate) width: Option<u32>,
    pub(crate) height: Option<u32>,
    pub(crate) size: Option<String>,
    pub(crate) length: Option<String>,
    pub(crate) original: Option<AttachmentDimensions>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct AttachmentDimensions {
    pub(crate) width: Option<u32>,
    pub(crate) height: Option<u32>,
    pub(crate) size: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    use xilem::tokio::{
        self,
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    async fn serve_once(
        response: &'static [u8],
    ) -> Result<(Mastodon, tokio::task::JoinHandle<Result<String>>)> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let mut api = Mastodon::new(&format!("http://{}", listener.local_addr()?))?;
        api.client = Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(5))
            .build()?;
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await?;
            let mut request = Vec::new();
            let mut buffer = [0; 1024];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let size = stream.read(&mut buffer).await?;
                ensure!(
                    size > 0 && request.len() + size <= 8192,
                    "Invalid test HTTP request"
                );
                request.extend_from_slice(&buffer[..size]);
            }
            stream.write_all(response).await?;
            Ok(String::from_utf8(request)?)
        });
        Ok((api, server))
    }

    #[test]
    fn lookup_encodes_query_and_distinguishes_http_status_errors() -> Result<()> {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(async {
                let (api, server) = serve_once(
                    b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .await?;
                let error = api.account("alice+&@example.test").await.unwrap_err();
                assert_eq!(
                    error
                        .downcast_ref::<reqwest::Error>()
                        .and_then(reqwest::Error::status),
                    Some(reqwest::StatusCode::NOT_FOUND)
                );
                let request = tokio::time::timeout(Duration::from_secs(5), server).await???;
                assert!(
                    request.starts_with(
                        "GET /api/v1/accounts/lookup?acct=alice%2B%26%40example.test HTTP/1.1\r\n"
                    ),
                    "{request}"
                );
                Ok(())
            })
    }

    #[test]
    fn streaming_body_limit_checks_actual_bytes_without_content_length() -> Result<()> {
        tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
            let (api, server) = serve_once(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n3\r\nabc\r\n3\r\ndef\r\n0\r\n\r\n").await?;
            let response = api.client.get(api.base.clone()).send().await?;
            assert!(response.content_length().is_none());
            let error = read_body(response, 5).await.unwrap_err();
            assert!(error.to_string().contains("exceeds 5 bytes"));
            tokio::time::timeout(Duration::from_secs(5), server).await???;
            Ok(())
        })
    }

    #[test]
    fn endpoint_preserves_origin_and_encodes_resource_ids() -> Result<()> {
        let api = Mastodon::new("https://example.test/mastodon/")?;
        let endpoint = api.endpoint(&["api", "v1", "statuses", "a/b?c#d%", "context"])?;
        assert_eq!(
            endpoint.as_str(),
            "https://example.test/mastodon/api/v1/statuses/a%2Fb%3Fc%23d%25/context"
        );
        assert!(api.endpoint(&["api", ".."]).is_err());
        assert!(Mastodon::new("https://user:pass@example.test/").is_err());
        Ok(())
    }

    #[test]
    fn status_deserializes_only_used_fields_and_unknown_attachment_types() -> Result<()> {
        let status: Status = serde_json::from_str(
            r#"{
            "id":"42", "account":{"id":"7","acct":"alice@example.test","display_name":"Alice","avatar_static":"https://example.test/a.png"},
            "in_reply_to_id":null,"reblog":null,"content":"<p>hello</p>","created_at":"2026-11-28T00:01:02.345Z",
            "replies_count":1,"reblogs_count":2,"favourites_count":3,"media_attachments":[{"type":"future-format","meta":null,"description":null,"blurhash":null}],"future_field":true
        }"#,
        )?;
        assert_eq!(status.id, "42");
        assert_eq!(status.account.acct, "alice@example.test");
        assert_eq!(status.created_at.millisecond(), 345);
        assert_eq!(status.media_attachments[0].r#type, AttachmentType::Unknown);
        Ok(())
    }
}
