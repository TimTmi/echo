//! Qdrant client module.
//!
//! REST API client for Qdrant vector database operations.
//! Communicates with Qdrant's HTTP API to list collections,
//! view collection info, and (in future) manage points.

use anyhow::Context;
use serde::Deserialize;
use serde_json::Value;

/// Information about a single Qdrant collection.
#[derive(Debug, Clone)]
pub struct CollectionInfo {
    pub name: String,
    pub vector_size: usize,
    pub distance: String,
    pub points_count: u64,
}

/// Vector configuration used for new collections. Project is locked to BGE-M3
/// (1024-D Cosine); anything else would change the entire embedding math and
/// is not supported in this app.
const DEFAULT_COLLECTION_SIZE: usize = 1024;
const DEFAULT_COLLECTION_DISTANCE: &str = "Cosine";

/// What happened when we tried to ensure the default collection exists.
#[derive(Debug, PartialEq, Eq)]
pub enum EnsureOutcome {
    Created,
    AlreadyExists,
}

/// Outcome of renaming the default-collection config (old -> new) on Qdrant.
#[derive(Debug, PartialEq, Eq)]
pub enum RenameOutcome {
    /// Old and new are identical; nothing to do.
    Unchanged,
    /// No previous default and a non-empty new one was given.
    Created,
    /// Old existed, new did not; old deleted, new created.
    Renamed,
    /// Old existed; new already existed too. Both kept as-is to avoid data loss.
    CollisionKept,
    /// Old existed, new is `None` -- we drop the default but leave Qdrant alone.
    DefaultCleared,
    /// New is `None` and old was already `None` -- nothing to do.
    NoDefault,
}

/// HTTP client for the Qdrant REST API.
#[derive(Debug, Clone)]
pub struct QdrantClient {
    /// Base URL of the Qdrant REST API (e.g. `http://localhost:6333`).
    base_url: String,
    /// Shared HTTP client for connection pooling.
    client: reqwest::Client,
}

impl QdrantClient {
    /// Create a new Qdrant client.
    ///
    /// The `base_url` should be the root of the Qdrant REST API
    /// (e.g. `http://localhost:6333`).
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            client: reqwest::Client::new(),
        }
    }

    /// List all collection names from Qdrant.
    ///
    /// Calls `GET /collections` and extracts the `name` field from each result.
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP request fails, the server returns a non-OK
    /// status, or the response cannot be parsed.
    pub async fn list_collections(&self) -> anyhow::Result<Vec<String>> {
        let url = format!("{}/collections", self.base_url);
        let response = self
            .client
            .get(&url)
            .send()
            .await
            .context("failed to send list collections request")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("list collections failed with status {status}: {body}");
        }

        let list_response: CollectionsListResponse = response
            .json()
            .await
            .context("failed to parse collections list response")?;

        let names = list_response
            .result
            .map(|r| r.collections)
            .unwrap_or_default()
            .into_iter()
            .map(|c| c.name)
            .collect();

        Ok(names)
    }

    /// Get detailed information about a specific collection.
    ///
    /// Calls `GET /collections/{name}` and extracts vector size, distance
    /// function, and points count.
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP request fails, the server returns a non-OK
    /// status, or the response cannot be parsed.
    pub async fn get_collection_info(&self, name: &str) -> anyhow::Result<CollectionInfo> {
        let url = format!("{}/collections/{name}", self.base_url);
        let response = self
            .client
            .get(&url)
            .send()
            .await
            .context("failed to send collection info request")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("get collection info failed with status {status}: {body}");
        }

        let info_response: CollectionInfoResponse = response
            .json()
            .await
            .context("failed to parse collection info response")?;

        let result = info_response
            .result
            .context("Qdrant returned no result for collection info")?;

        // Extract vector config — prefer unnamed (default) vector, else first named one
        let (vector_size, distance) = result
            .config
            .params
            .vectors
            .as_ref()
            .and_then(|vectors| {
                vectors
                    .get("")
                    .or_else(|| vectors.values().next())
                    .and_then(|v| v.as_object())
            })
            .and_then(|obj| {
                let size = obj.get("size")?.as_u64()? as usize;
                let distance = obj.get("distance")?.as_str()?.to_string();
                Some((size, distance))
            })
            .unwrap_or((0, "Unknown".to_string()));

        Ok(CollectionInfo {
            name: name.to_string(),
            vector_size,
            distance,
            points_count: result.points_count,
        })
    }
}

/// Search points in a collection by vector.
///
/// Calls `POST /collections/{name}/points/search`.
///
/// # Errors
///
/// Returns an error if the HTTP request fails or response is unparseable.
impl QdrantClient {
    pub async fn search_points(
        &self,
        collection: &str,
        vector: &[f32],
        limit: usize,
    ) -> anyhow::Result<Vec<SearchResult>> {
        let url = format!("{}/collections/{collection}/points/search", self.base_url);
        let body = serde_json::json!({
            "vector": vector,
            "limit": limit,
            "with_payload": true,
            "with_vector": false,
        });

        let response = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .context("failed to send search request")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("search failed with status {status}: {body}");
        }

        let search_response: SearchResponse = response
            .json()
            .await
            .context("failed to parse search response")?;

        Ok(search_response.result.unwrap_or_default())
    }

    /// Scroll points in a collection with cursor-based pagination.
    ///
    /// Calls `POST /collections/{name}/points/scroll`. Pass `offset=None` for
    /// the first page; pass `next_offset` returned by the previous call to fetch
    /// the next page. When `next_offset` is `None` or absent, there are no more
    /// points to paginate.
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP request fails or the response is unparseable.
    pub async fn scroll_points(
        &self,
        collection: &str,
        limit: usize,
        offset: Option<&serde_json::Value>,
    ) -> anyhow::Result<ScrollPage> {
        let url = format!("{}/collections/{collection}/points/scroll", self.base_url);

        let mut body = serde_json::json!({
            "limit": limit,
            "with_payload": true,
            "with_vector": false,
        });
        if let Some(off) = offset {
            body["offset"] = off.clone();
        }

        let response = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .context("failed to send scroll request")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("scroll failed with status {status}: {body}");
        }

        let scroll_response: ScrollResponse = response
            .json()
            .await
            .context("failed to parse scroll response")?;

        let result = scroll_response.result.unwrap_or(ScrollResult {
            points: Vec::new(),
            next_page_offset: None,
        });
        Ok(ScrollPage {
            points: result.points,
            next_offset: result.next_page_offset,
        })
    }

    /// Check whether a collection with the given name exists on Qdrant.
    /// Returns `false` for both "not found" and any network error -- callers
    /// treat "I don't know" the same as "no" for ensure-default purposes.
    pub async fn collection_exists(&self, name: &str) -> bool {
        let url = format!("{}/collections/{name}", self.base_url);
        match self.client.get(&url).send().await {
            Ok(resp) => resp.status().is_success(),
            Err(_) => false,
        }
    }

    /// Create a collection with the given vector configuration.
    /// `PUT /collections/{name}` with body `{"vectors": {"size": ..., "distance": ...}}`.
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP request fails or Qdrant rejects (e.g.
    /// `409` if the collection already exists).
    pub async fn create_collection(
        &self,
        name: &str,
        size: usize,
        distance: &str,
    ) -> anyhow::Result<()> {
        let url = format!("{}/collections/{name}", self.base_url);
        let body = serde_json::json!({
            "vectors": { "size": size, "distance": distance }
        });
        let response = self
            .client
            .put(&url)
            .json(&body)
            .send()
            .await
            .context("failed to send create collection request")?;
        if !response.status().is_success() {
            let status = response.status();
            let body_text = response.text().await.unwrap_or_default();
            anyhow::bail!("create collection failed with status {status}: {body_text}");
        }
        Ok(())
    }

    /// Delete a collection. `DELETE /collections/{name}`.
    /// A 404 (collection already absent) is treated as success.
    ///
    /// # Errors
    ///
    /// Returns an error for other non-OK statuses or transport failures.
    pub async fn delete_collection(&self, name: &str) -> anyhow::Result<()> {
        let url = format!("{}/collections/{name}", self.base_url);
        let response = self
            .client
            .delete(&url)
            .send()
            .await
            .context("failed to send delete collection request")?;
        let status = response.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Ok(());
        }
        if !status.is_success() {
            let body_text = response.text().await.unwrap_or_default();
            anyhow::bail!("delete collection failed with status {status}: {body_text}");
        }
        Ok(())
    }

    /// Ensure `name` exists on Qdrant. Creates it with the BGE-M3 vector
    /// configuration if missing. Used on App startup to guarantee that
    /// `Config.default_collection` is always usable.
    pub async fn ensure_default_collection(&self, name: &str) -> anyhow::Result<EnsureOutcome> {
        if self.collection_exists(name).await {
            return Ok(EnsureOutcome::AlreadyExists);
        }
        self.create_collection(name, DEFAULT_COLLECTION_SIZE, DEFAULT_COLLECTION_DISTANCE)
            .await?;
        Ok(EnsureOutcome::Created)
    }

    /// Apply a rename of the configured default collection: delete `old` if
    /// it existed, create `new` if it didn't. If `new` already exists we
    /// keep it as-is (return [`RenameOutcome::CollisionKept`]) to avoid
    /// silently dropping an unrelated collection the user may have created
    /// out-of-band.
    pub async fn rename_default_collection(
        &self,
        old: Option<&str>,
        new: Option<&str>,
    ) -> anyhow::Result<RenameOutcome> {
        match (old, new) {
            (None, None) => Ok(RenameOutcome::NoDefault),
            (Some(_), None) => Ok(RenameOutcome::DefaultCleared),
            (None, Some(n)) => {
                if self.collection_exists(n).await {
                    Ok(RenameOutcome::CollisionKept)
                } else {
                    self.create_collection(n, DEFAULT_COLLECTION_SIZE, DEFAULT_COLLECTION_DISTANCE)
                        .await?;
                    Ok(RenameOutcome::Created)
                }
            }
            (Some(o), Some(n)) if o == n => Ok(RenameOutcome::Unchanged),
            (Some(o), Some(n)) => {
                if self.collection_exists(n).await {
                    Ok(RenameOutcome::CollisionKept)
                } else {
                    self.delete_collection(o).await?;
                    self.create_collection(n, DEFAULT_COLLECTION_SIZE, DEFAULT_COLLECTION_DISTANCE)
                        .await?;
                    Ok(RenameOutcome::Renamed)
                }
            }
        }
    }
}

/// A page of points returned by `scroll_points`, plus the cursor for the next page.
#[derive(Debug, Clone, Default)]
pub struct ScrollPage {
    pub points: Vec<PointRecord>,
    /// Pass back as `offset` on the next call. `None` means no more pages.
    pub next_offset: Option<serde_json::Value>,
}

/// A single point in a collection (returned by scroll).
#[derive(Debug, Clone, Deserialize)]
pub struct PointRecord {
    pub id: serde_json::Value,
    pub payload: Option<serde_json::Map<String, serde_json::Value>>,
}

/// A single matched point from a Qdrant search result.
#[derive(Debug, Clone, Deserialize)]
pub struct SearchResult {
    pub id: serde_json::Value,
    pub version: Option<u64>,
    pub score: Option<f64>,
    pub payload: Option<serde_json::Map<String, serde_json::Value>>,
    pub vector: Option<Vec<f64>>,
}

// ---------------------------------------------------------------------------
// Internal response types — mirroring the Qdrant REST API JSON structure
// ---------------------------------------------------------------------------

/// Response for a search request.
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct SearchResponse {
    result: Option<Vec<SearchResult>>,
    status: Option<String>,
    time: Option<f64>,
}

/// Response for `GET /collections`.
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct CollectionsListResponse {
    result: Option<CollectionsListResult>,
    status: Option<String>,
    time: Option<f64>,
}

/// The `result` field of a collections list response.
/// Qdrant nests the array of collections under a `collections` key.
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct CollectionsListResult {
    collections: Vec<CollectionSummary>,
}

/// A single collection entry in the list response.
#[derive(Debug, Deserialize)]
struct CollectionSummary {
    name: String,
}

/// Response for `GET /collections/{name}`.
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct CollectionInfoResponse {
    result: Option<CollectionResult>,
    status: Option<String>,
    time: Option<f64>,
}

/// The `result` field of a collection info response.
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct CollectionResult {
    status: Option<String>,
    optimizer_status: Option<String>,
    points_count: u64,
    segments_count: Option<u64>,
    config: CollectionConfig,
}

/// The `config` section of collection info.
#[derive(Debug, Deserialize)]
struct CollectionConfig {
    params: CollectionParams,
}

/// The `params` section of collection config.
#[derive(Debug, Deserialize)]
struct CollectionParams {
    vectors: Option<serde_json::Map<String, Value>>,
}

/// Response for a scroll request.
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct ScrollResponse {
    result: Option<ScrollResult>,
    status: Option<String>,
    time: Option<f64>,
}

/// The `result` field of a scroll response.
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct ScrollResult {
    points: Vec<PointRecord>,
    next_page_offset: Option<serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Helper: create a mockito server with a matching endpoint.
    async fn setup_mock(
        method: &str,
        path: &str,
        status: usize,
        body: String,
    ) -> (mockito::ServerGuard, mockito::Mock) {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock(method, path)
            .with_status(status)
            .with_header("content-type", "application/json")
            .with_body(body)
            .create();
        (server, mock)
    }

    #[tokio::test]
    async fn test_list_collections_empty() {
        let response_body = json!({
            "result": {"collections": []},
            "status": "ok",
            "time": 0.001
        })
        .to_string();

        let (server, mock) = setup_mock("GET", "/collections", 200, response_body).await;
        let client = QdrantClient::new(server.url());

        let result = client.list_collections().await;
        assert!(result.is_ok(), "expected Ok, got {result:?}");
        assert!(result.unwrap().is_empty());

        mock.assert();
    }

    #[tokio::test]
    async fn test_list_collections_two_items() {
        let response_body = json!({
            "result": {
                "collections": [
                    {"name": "documents"},
                    {"name": "images"}
                ]
            },
            "status": "ok",
            "time": 0.002
        })
        .to_string();

        let (server, mock) = setup_mock("GET", "/collections", 200, response_body).await;
        let client = QdrantClient::new(server.url());

        let result = client.list_collections().await;
        assert!(result.is_ok(), "expected Ok, got {result:?}");
        let names = result.unwrap();
        assert_eq!(names, vec!["documents", "images"]);

        mock.assert();
    }

    #[tokio::test]
    async fn test_list_collections_http_error() {
        let (server, mock) =
            setup_mock("GET", "/collections", 503, "Service Unavailable".into()).await;
        let client = QdrantClient::new(server.url());

        let result = client.list_collections().await;
        assert!(result.is_err(), "expected error for 503");
        let err_msg = format!("{:#}", result.unwrap_err());
        assert!(err_msg.contains("503"), "expected 503 in error: {err_msg}");

        mock.assert();
    }

    #[tokio::test]
    async fn test_get_collection_info_success() {
        let response_body = json!({
            "result": {
                "status": "green",
                "optimizer_status": "ok",
                "points_count": 42,
                "segments_count": 2,
                "config": {
                    "params": {
                        "vectors": {
                            "": {
                                "size": 1024,
                                "distance": "Cosine"
                            }
                        }
                    }
                }
            },
            "status": "ok",
            "time": 0.005
        })
        .to_string();

        let (server, mock) = setup_mock("GET", "/collections/my_coll", 200, response_body).await;
        let client = QdrantClient::new(server.url());

        let result = client.get_collection_info("my_coll").await;
        assert!(result.is_ok(), "expected Ok, got {result:?}");
        let info = result.unwrap();
        assert_eq!(info.name, "my_coll");
        assert_eq!(info.vector_size, 1024);
        assert_eq!(info.distance, "Cosine");
        assert_eq!(info.points_count, 42);

        mock.assert();
    }

    #[tokio::test]
    async fn test_get_collection_info_named_vector() {
        let response_body = json!({
            "result": {
                "status": "green",
                "optimizer_status": "ok",
                "points_count": 10,
                "segments_count": 1,
                "config": {
                    "params": {
                        "vectors": {
                            "text": {
                                "size": 768,
                                "distance": "Dot"
                            }
                        }
                    }
                }
            },
            "status": "ok",
            "time": 0.003
        })
        .to_string();

        let (server, mock) = setup_mock("GET", "/collections/named_coll", 200, response_body).await;
        let client = QdrantClient::new(server.url());

        let result = client.get_collection_info("named_coll").await;
        assert!(result.is_ok(), "expected Ok, got {result:?}");
        let info = result.unwrap();
        assert_eq!(info.name, "named_coll");
        assert_eq!(info.vector_size, 768);
        assert_eq!(info.distance, "Dot");

        mock.assert();
    }

    #[tokio::test]
    async fn test_get_collection_info_not_found() {
        let response_body = json!({
            "result": null,
            "status": "error",
            "time": 0.001
        })
        .to_string();

        let (server, mock) =
            setup_mock("GET", "/collections/nonexistent", 404, response_body).await;
        let client = QdrantClient::new(server.url());

        let result = client.get_collection_info("nonexistent").await;
        assert!(result.is_err(), "expected error for 404");
        let err_msg = format!("{:#}", result.unwrap_err());
        assert!(err_msg.contains("404"), "expected 404 in error: {err_msg}");

        mock.assert();
    }

    #[tokio::test]
    async fn test_get_collection_info_null_result() {
        let response_body = json!({
            "result": null,
            "status": "ok",
            "time": 0.001
        })
        .to_string();

        let (server, mock) = setup_mock("GET", "/collections/ghost", 200, response_body).await;
        let client = QdrantClient::new(server.url());

        let result = client.get_collection_info("ghost").await;
        assert!(result.is_err(), "expected error for null result");
        let err_msg = format!("{:#}", result.unwrap_err());
        assert!(
            err_msg.contains("no result"),
            "expected 'no result' in error: {err_msg}"
        );

        mock.assert();
    }

    #[tokio::test]
    async fn test_scroll_points_basic() {
        let response_body = json!({
            "result": {
                "points": [
                    { "id": 1, "payload": {"text": "hello"}, "version": 0 },
                    { "id": 2, "payload": {"text": "world"}, "version": 1 }
                ],
                "next_page_offset": 2
            },
            "status": "ok",
            "time": 0.001
        })
        .to_string();

        let (server, mock) = setup_mock(
            "POST",
            "/collections/docs/points/scroll",
            200,
            response_body,
        )
        .await;
        let client = QdrantClient::new(server.url());

        let page = client
            .scroll_points("docs", 20, None)
            .await
            .expect("scroll failed");
        assert_eq!(page.points.len(), 2);
        assert_eq!(page.points[0].id, json!(1));
        assert_eq!(
            page.points[0].payload.as_ref().and_then(|p| p.get("text")),
            Some(&json!("hello"))
        );
        assert_eq!(page.next_offset, Some(json!(2)));

        mock.assert();
    }

    #[tokio::test]
    async fn test_scroll_points_with_offset() {
        let response_body = json!({
            "result": {
                "points": [
                    { "id": "uuid-3", "payload": {"k": "v"} }
                ],
                "next_page_offset": null
            },
            "status": "ok",
            "time": 0.001
        })
        .to_string();

        let (server, mock) = setup_mock(
            "POST",
            "/collections/notes/points/scroll",
            200,
            response_body,
        )
        .await;
        let client = QdrantClient::new(server.url());

        let offset = json!("uuid-2");
        let page = client
            .scroll_points("notes", 10, Some(&offset))
            .await
            .expect("scroll failed");
        assert_eq!(page.points.len(), 1);
        assert_eq!(page.next_offset, None, "null offset means end of pages");

        mock.assert();
    }

    #[tokio::test]
    async fn test_scroll_points_empty_collection() {
        let response_body = json!({
            "result": {
                "points": [],
                "next_page_offset": null
            },
            "status": "ok",
            "time": 0.001
        })
        .to_string();

        let (server, mock) = setup_mock(
            "POST",
            "/collections/empty/points/scroll",
            200,
            response_body,
        )
        .await;
        let client = QdrantClient::new(server.url());

        let page = client
            .scroll_points("empty", 20, None)
            .await
            .expect("scroll failed");
        assert!(page.points.is_empty());
        assert_eq!(page.next_offset, None);

        mock.assert();
    }

    #[tokio::test]
    async fn test_scroll_points_http_error() {
        let response_body = json!({
            "status": "error",
            "message": "collection not found",
            "time": 0.001
        })
        .to_string();

        let (server, mock) = setup_mock(
            "POST",
            "/collections/missing/points/scroll",
            404,
            response_body,
        )
        .await;
        let client = QdrantClient::new(server.url());

        let result = client.scroll_points("missing", 20, None).await;
        assert!(result.is_err(), "expected error on 404");
        let err = format!("{:#}", result.unwrap_err());
        assert!(err.contains("404"), "expected status 404 in: {err}");

        mock.assert();
    }

    // -----------------------------------------------------------------
    // collection_exists / create / delete / ensure / rename
    // -----------------------------------------------------------------

    #[tokio::test]
    async fn test_collection_exists_true() {
        let (server, mock) = setup_mock("GET", "/collections/alpha", 200, "{}".into()).await;
        let client = QdrantClient::new(server.url());
        assert!(client.collection_exists("alpha").await);
        mock.assert();
    }

    #[tokio::test]
    async fn test_collection_exists_false_on_404() {
        let (server, mock) = setup_mock("GET", "/collections/missing", 404, "{}".into()).await;
        let client = QdrantClient::new(server.url());
        assert!(!client.collection_exists("missing").await);
        mock.assert();
    }

    #[tokio::test]
    async fn test_create_collection_success() {
        let body = json!({"result": true, "status": "ok", "time": 0.001}).to_string();
        let (server, mock) = setup_mock("PUT", "/collections/general", 200, body).await;
        let client = QdrantClient::new(server.url());
        client
            .create_collection("general", 1024, "Cosine")
            .await
            .expect("create should succeed on 200");
        mock.assert();
    }

    #[tokio::test]
    async fn test_create_collection_409_already_exists() {
        let body = json!({
            "status": "error",
            "message": "Collection `general` already exists",
            "time": 0.001
        })
        .to_string();
        let (server, mock) = setup_mock("PUT", "/collections/general", 409, body).await;
        let client = QdrantClient::new(server.url());
        let err = client
            .create_collection("general", 1024, "Cosine")
            .await
            .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("409"), "expected 409 in: {msg}");
        assert!(msg.contains("already exists"), "expected body in: {msg}");
        mock.assert();
    }

    #[tokio::test]
    async fn test_delete_collection_success() {
        let body = json!({"result": true, "status": "ok", "time": 0.001}).to_string();
        let (server, mock) = setup_mock("DELETE", "/collections/general", 200, body).await;
        let client = QdrantClient::new(server.url());
        client
            .delete_collection("general")
            .await
            .expect("delete should succeed on 200");
        mock.assert();
    }

    #[tokio::test]
    async fn test_delete_collection_404_is_ok() {
        let body = json!({"status": "error", "message": "Not found"}).to_string();
        let (server, mock) = setup_mock("DELETE", "/collections/ghost", 404, body).await;
        let client = QdrantClient::new(server.url());
        client
            .delete_collection("ghost")
            .await
            .expect("delete should treat 404 as success");
        mock.assert();
    }

    #[tokio::test]
    async fn test_ensure_default_collection_creates_when_missing() {
        let mut server = mockito::Server::new_async().await;
        let _exists = server
            .mock("GET", "/collections/general")
            .with_status(404)
            .with_header("content-type", "application/json")
            .with_body("{}")
            .create();
        let create = server
            .mock("PUT", "/collections/general")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(json!({"result": true, "status": "ok"}).to_string())
            .create();

        let client = QdrantClient::new(server.url());
        let outcome = client
            .ensure_default_collection("general")
            .await
            .expect("ensure ok");
        assert_eq!(outcome, EnsureOutcome::Created);
        create.assert();
    }

    #[tokio::test]
    async fn test_ensure_default_collection_already_exists_is_noop() {
        let mut server = mockito::Server::new_async().await;
        let exists = server
            .mock("GET", "/collections/general")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body("{}")
            .expect(1)
            .create();

        let client = QdrantClient::new(server.url());
        let outcome = client
            .ensure_default_collection("general")
            .await
            .expect("ensure ok");
        assert_eq!(outcome, EnsureOutcome::AlreadyExists);
        exists.assert();
    }

    #[tokio::test]
    async fn test_rename_default_collection_unchanged() {
        let server = mockito::Server::new_async().await;
        let client = QdrantClient::new(server.url());
        let outcome = client
            .rename_default_collection(Some("general"), Some("general"))
            .await
            .unwrap();
        assert_eq!(outcome, RenameOutcome::Unchanged);
    }

    #[tokio::test]
    async fn test_rename_default_collection_rename_when_new_absent() {
        let mut server = mockito::Server::new_async().await;
        let exists_new = server
            .mock("GET", "/collections/docs")
            .with_status(404)
            .with_header("content-type", "application/json")
            .with_body("{}")
            .create();
        let delete_old = server
            .mock("DELETE", "/collections/general")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(json!({"result": true, "status": "ok"}).to_string())
            .create();
        let create_new = server
            .mock("PUT", "/collections/docs")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(json!({"result": true, "status": "ok"}).to_string())
            .create();

        let client = QdrantClient::new(server.url());
        let outcome = client
            .rename_default_collection(Some("general"), Some("docs"))
            .await
            .unwrap();
        assert_eq!(outcome, RenameOutcome::Renamed);
        exists_new.assert();
        delete_old.assert();
        create_new.assert();
    }

    #[tokio::test]
    async fn test_rename_default_collection_collision_keeps_existing() {
        let mut server = mockito::Server::new_async().await;
        let exists_new = server
            .mock("GET", "/collections/docs")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body("{}")
            .expect(1)
            .create();
        let _must_not_delete = server
            .mock("DELETE", "/collections/general")
            .with_status(200)
            .expect(0)
            .create();
        let _must_not_create = server
            .mock("PUT", "/collections/docs")
            .with_status(200)
            .expect(0)
            .create();

        let client = QdrantClient::new(server.url());
        let outcome = client
            .rename_default_collection(Some("general"), Some("docs"))
            .await
            .unwrap();
        assert_eq!(outcome, RenameOutcome::CollisionKept);
        exists_new.assert();
    }

    #[tokio::test]
    async fn test_rename_default_collection_create_only_when_old_none() {
        let mut server = mockito::Server::new_async().await;
        let exists_new = server
            .mock("GET", "/collections/general")
            .with_status(404)
            .with_header("content-type", "application/json")
            .with_body("{}")
            .create();
        let create_new = server
            .mock("PUT", "/collections/general")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(json!({"result": true, "status": "ok"}).to_string())
            .create();

        let client = QdrantClient::new(server.url());
        let outcome = client
            .rename_default_collection(None, Some("general"))
            .await
            .unwrap();
        assert_eq!(outcome, RenameOutcome::Created);
        exists_new.assert();
        create_new.assert();
    }

    #[tokio::test]
    async fn test_rename_default_collection_clear_leaves_qdrant_alone() {
        let server = mockito::Server::new_async().await;
        let client = QdrantClient::new(server.url());
        let outcome = client
            .rename_default_collection(Some("general"), None)
            .await
            .unwrap();
        assert_eq!(outcome, RenameOutcome::DefaultCleared);
    }

    #[tokio::test]
    async fn test_rename_default_collection_no_default_is_noop() {
        let server = mockito::Server::new_async().await;
        let client = QdrantClient::new(server.url());
        let outcome = client
            .rename_default_collection(None, None)
            .await
            .unwrap();
        assert_eq!(outcome, RenameOutcome::NoDefault);
    }
}
