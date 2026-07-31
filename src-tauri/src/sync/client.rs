use super::types::*;
use reqwest::Client;
use std::collections::HashSet;

/// Header names the presigned URL actually signed, read from its
/// `X-Amz-SignedHeaders` query parameter (lowercased, `;`-separated).
///
/// `None` when the URL has no such parameter or does not parse — callers then
/// keep their previous behaviour rather than silently dropping headers.
fn signed_headers(presigned_url: &str) -> Option<HashSet<String>> {
  let url = reqwest::Url::parse(presigned_url).ok()?;
  let value = url
    .query_pairs()
    .find(|(k, _)| k.eq_ignore_ascii_case("X-Amz-SignedHeaders"))
    .map(|(_, v)| v.into_owned())?;
  Some(
    value
      .split(';')
      .map(|h| h.trim().to_ascii_lowercase())
      .filter(|h| !h.is_empty())
      .collect(),
  )
}

#[derive(Clone)]
pub struct SyncClient {
  client: Client,
  base_url: String,
  token: String,
}

impl SyncClient {
  pub fn new(base_url: String, token: String) -> Self {
    Self {
      client: Client::new(),
      base_url: base_url.trim_end_matches('/').to_string(),
      token,
    }
  }

  fn url(&self, path: &str) -> String {
    format!("{}/v1/objects/{}", self.base_url, path)
  }

  pub async fn stat(&self, key: &str) -> SyncResult<StatResponse> {
    let response = self
      .client
      .post(self.url("stat"))
      .header("Authorization", format!("Bearer {}", self.token))
      .json(&StatRequest {
        key: key.to_string(),
      })
      .send()
      .await
      .map_err(|e| SyncError::NetworkError(e.to_string()))?;

    if response.status().is_client_error() {
      let status = response.status();
      let body = response.text().await.unwrap_or_default();
      return Err(SyncError::AuthError(format!("({status}) {body}")));
    }

    response
      .json()
      .await
      .map_err(|e| SyncError::SerializationError(e.to_string()))
  }

  pub async fn presign_upload(
    &self,
    key: &str,
    content_type: Option<&str>,
  ) -> SyncResult<PresignUploadResponse> {
    self
      .presign_upload_with_metadata(key, content_type, None)
      .await
  }

  /// Presign an upload, asking the server to sign `metadata` into the object as
  /// `x-amz-meta-*`. The response echoes the metadata the server actually signed
  /// (empty/None on older servers); the caller must send exactly that back on
  /// the PUT via `upload_bytes_with_metadata`.
  pub async fn presign_upload_with_metadata(
    &self,
    key: &str,
    content_type: Option<&str>,
    metadata: Option<std::collections::HashMap<String, String>>,
  ) -> SyncResult<PresignUploadResponse> {
    let response = self
      .client
      .post(self.url("presign-upload"))
      .header("Authorization", format!("Bearer {}", self.token))
      .json(&PresignUploadRequest {
        key: key.to_string(),
        content_type: content_type.map(|s| s.to_string()),
        expires_in: Some(3600),
        metadata,
      })
      .send()
      .await
      .map_err(|e| SyncError::NetworkError(e.to_string()))?;

    if response.status().is_client_error() {
      let status = response.status();
      let body = response.text().await.unwrap_or_default();
      return Err(SyncError::AuthError(format!("({status}) {body}")));
    }

    response
      .json()
      .await
      .map_err(|e| SyncError::SerializationError(e.to_string()))
  }

  pub async fn presign_download(&self, key: &str) -> SyncResult<PresignDownloadResponse> {
    let response = self
      .client
      .post(self.url("presign-download"))
      .header("Authorization", format!("Bearer {}", self.token))
      .json(&PresignDownloadRequest {
        key: key.to_string(),
        expires_in: Some(3600),
      })
      .send()
      .await
      .map_err(|e| SyncError::NetworkError(e.to_string()))?;

    if response.status().is_client_error() {
      let status = response.status();
      let body = response.text().await.unwrap_or_default();
      return Err(SyncError::AuthError(format!("({status}) {body}")));
    }

    response
      .json()
      .await
      .map_err(|e| SyncError::SerializationError(e.to_string()))
  }

  pub async fn delete(&self, key: &str, tombstone_key: Option<&str>) -> SyncResult<DeleteResponse> {
    let response = self
      .client
      .post(self.url("delete"))
      .header("Authorization", format!("Bearer {}", self.token))
      .json(&DeleteRequest {
        key: key.to_string(),
        tombstone_key: tombstone_key.map(|s| s.to_string()),
        deleted_at: Some(chrono::Utc::now().to_rfc3339()),
      })
      .send()
      .await
      .map_err(|e| SyncError::NetworkError(e.to_string()))?;

    if response.status().is_client_error() {
      let status = response.status();
      let body = response.text().await.unwrap_or_default();
      return Err(SyncError::AuthError(format!("({status}) {body}")));
    }

    response
      .json()
      .await
      .map_err(|e| SyncError::SerializationError(e.to_string()))
  }

  pub async fn list(&self, prefix: &str) -> SyncResult<ListResponse> {
    self.list_page(prefix, None).await
  }

  async fn list_page(
    &self,
    prefix: &str,
    continuation_token: Option<String>,
  ) -> SyncResult<ListResponse> {
    let response = self
      .client
      .post(self.url("list"))
      .header("Authorization", format!("Bearer {}", self.token))
      .json(&ListRequest {
        prefix: prefix.to_string(),
        max_keys: Some(1000),
        continuation_token,
      })
      .send()
      .await
      .map_err(|e| SyncError::NetworkError(e.to_string()))?;

    if response.status().is_client_error() {
      let status = response.status();
      let body = response.text().await.unwrap_or_default();
      return Err(SyncError::AuthError(format!("({status}) {body}")));
    }

    response
      .json()
      .await
      .map_err(|e| SyncError::SerializationError(e.to_string()))
  }

  /// List all objects under a prefix, paginating through all results
  pub async fn list_all(&self, prefix: &str) -> SyncResult<Vec<ListObject>> {
    let mut all_objects = Vec::new();
    let mut continuation_token: Option<String> = None;

    loop {
      let response = self.list_page(prefix, continuation_token).await?;
      all_objects.extend(response.objects);

      if !response.is_truncated {
        break;
      }
      continuation_token = response.next_continuation_token;
      if continuation_token.is_none() {
        break;
      }
    }

    Ok(all_objects)
  }

  pub async fn upload_bytes(
    &self,
    presigned_url: &str,
    data: &[u8],
    content_type: Option<&str>,
  ) -> SyncResult<()> {
    self
      .upload_bytes_with_metadata(presigned_url, data, content_type, None)
      .await
  }

  /// PUT to a presigned URL, sending `metadata` as `x-amz-meta-*` headers. These
  /// MUST be exactly the metadata the presign signed (from
  /// `PresignUploadResponse::metadata`) or S3 rejects the request.
  pub async fn upload_bytes_with_metadata(
    &self,
    presigned_url: &str,
    data: &[u8],
    content_type: Option<&str>,
    metadata: Option<&std::collections::HashMap<String, String>>,
  ) -> SyncResult<()> {
    let mut req = self
      .client
      .put(presigned_url)
      .header("Content-Length", data.len().to_string())
      .body(data.to_vec());

    if let Some(ct) = content_type {
      req = req.header("Content-Type", ct);
    }

    if let Some(meta) = metadata {
      // Send only the metadata the presign signed as a *header*. An older
      // server (or any presigner left on its defaults) hoists `x-amz-meta-*`
      // into the query string instead, and MinIO then rejects the PUT outright
      // for carrying an x-amz-* header outside X-Amz-SignedHeaders. The hoisted
      // query parameter still conveys the value, so skipping the header loses
      // nothing here.
      let signed = signed_headers(presigned_url);
      for (k, v) in meta {
        let header = format!("x-amz-meta-{k}");
        if signed.as_ref().is_none_or(|s| s.contains(&header)) {
          req = req.header(header, v);
        }
      }
    }

    let response = req
      .send()
      .await
      .map_err(|e| SyncError::NetworkError(e.to_string()))?;

    if !response.status().is_success() {
      let status = response.status();
      let body = response.text().await.unwrap_or_default();
      return Err(SyncError::NetworkError(format!(
        "Upload failed with status {status}: {body}"
      )));
    }

    Ok(())
  }

  pub async fn download_bytes(&self, presigned_url: &str) -> SyncResult<Vec<u8>> {
    let response = self
      .client
      .get(presigned_url)
      .send()
      .await
      .map_err(|e| SyncError::NetworkError(e.to_string()))?;

    if !response.status().is_success() {
      return Err(SyncError::NetworkError(format!(
        "Download failed with status: {}",
        response.status()
      )));
    }

    response
      .bytes()
      .await
      .map(|b| b.to_vec())
      .map_err(|e| SyncError::NetworkError(e.to_string()))
  }

  pub async fn presign_upload_batch(
    &self,
    items: Vec<(String, Option<String>)>,
  ) -> SyncResult<PresignUploadBatchResponse> {
    let chunk_size = 500;
    let mut all_items = Vec::new();

    for chunk in items.chunks(chunk_size) {
      let request = PresignUploadBatchRequest {
        items: chunk
          .iter()
          .map(|(key, content_type)| PresignUploadBatchItem {
            key: key.clone(),
            content_type: content_type.clone(),
          })
          .collect(),
        expires_in: Some(3600),
      };

      let response = self
        .client
        .post(self.url("presign-upload-batch"))
        .header("Authorization", format!("Bearer {}", self.token))
        .json(&request)
        .send()
        .await
        .map_err(|e| SyncError::NetworkError(e.to_string()))?;

      if response.status().is_client_error() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(SyncError::AuthError(format!("({status}) {body}")));
      }

      let batch_response: PresignUploadBatchResponse = response
        .json()
        .await
        .map_err(|e| SyncError::SerializationError(e.to_string()))?;

      all_items.extend(batch_response.items);
    }

    Ok(PresignUploadBatchResponse { items: all_items })
  }

  pub async fn presign_download_batch(
    &self,
    keys: Vec<String>,
  ) -> SyncResult<PresignDownloadBatchResponse> {
    let chunk_size = 500;
    let mut all_items = Vec::new();

    for chunk in keys.chunks(chunk_size) {
      let request = PresignDownloadBatchRequest {
        keys: chunk.to_vec(),
        expires_in: Some(3600),
      };

      let response = self
        .client
        .post(self.url("presign-download-batch"))
        .header("Authorization", format!("Bearer {}", self.token))
        .json(&request)
        .send()
        .await
        .map_err(|e| SyncError::NetworkError(e.to_string()))?;

      if response.status().is_client_error() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(SyncError::AuthError(format!("({status}) {body}")));
      }

      let batch_response: PresignDownloadBatchResponse = response
        .json()
        .await
        .map_err(|e| SyncError::SerializationError(e.to_string()))?;

      all_items.extend(batch_response.items);
    }

    Ok(PresignDownloadBatchResponse { items: all_items })
  }

  pub async fn delete_prefix(
    &self,
    prefix: &str,
    tombstone_key: Option<&str>,
  ) -> SyncResult<DeletePrefixResponse> {
    let response = self
      .client
      .post(self.url("delete-prefix"))
      .header("Authorization", format!("Bearer {}", self.token))
      .json(&DeletePrefixRequest {
        prefix: prefix.to_string(),
        tombstone_key: tombstone_key.map(|s| s.to_string()),
        deleted_at: Some(chrono::Utc::now().to_rfc3339()),
      })
      .send()
      .await
      .map_err(|e| SyncError::NetworkError(e.to_string()))?;

    if response.status().is_client_error() {
      let status = response.status();
      let body = response.text().await.unwrap_or_default();
      return Err(SyncError::AuthError(format!("({status}) {body}")));
    }

    response
      .json()
      .await
      .map_err(|e| SyncError::SerializationError(e.to_string()))
  }
}

#[cfg(test)]
mod tests {
  use super::signed_headers;

  const BASE: &str = "https://s3.example.com:8888/bucket/proxies/x.json\
?X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Expires=3600";

  #[test]
  fn reads_signed_header_list() {
    let url = format!("{BASE}&X-Amz-SignedHeaders=host%3Bx-amz-meta-updated-at");
    let signed = signed_headers(&url).expect("parses");
    assert!(signed.contains("host"));
    assert!(signed.contains("x-amz-meta-updated-at"));
  }

  #[test]
  fn hoisted_metadata_is_not_reported_as_signed() {
    // What a presigner on its defaults produces: the metadata rides in the
    // query string and `host` is the only signed header. Sending the matching
    // x-amz-meta-* header here is what MinIO rejects with a 400.
    let url = format!("{BASE}&x-amz-meta-updated-at=1785389945&X-Amz-SignedHeaders=host");
    let signed = signed_headers(&url).expect("parses");
    assert!(signed.contains("host"));
    assert!(!signed.contains("x-amz-meta-updated-at"));
  }

  #[test]
  fn missing_or_unparsable_url_yields_none() {
    assert!(signed_headers(BASE).is_none(), "no SignedHeaders param");
    assert!(signed_headers("not a url").is_none());
  }
}
