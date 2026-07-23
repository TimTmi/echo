//! Embedding client module.
//!
//! HTTP client for llama.cpp embedding server (BGE-M3).
//! Generates embeddings via the OpenAI-compatible `/v1/embeddings` endpoint.

use anyhow::Context;
use serde::{Deserialize, Serialize};

/// Request payload sent to the `/v1/embeddings` endpoint.
/// Field names follow OpenAI's `/v1/embeddings` schema.
#[derive(Debug, Serialize)]
struct EmbeddingRequest {
    input: String,
    model: String,
}

/// Response from the llama.cpp `/v1/embeddings` endpoint (OpenAI-compat shape).
/// The vector lives at `data[0].embedding`.
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct EmbeddingResponse {
    model: Option<String>,
    object: Option<String>,
    data: Vec<EmbeddingData>,
}

/// Single item in the `data` array of an embedding response.
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct EmbeddingData {
    embedding: Vec<f32>,
}

/// HTTP client for generating embeddings via llama.cpp.
#[derive(Debug, Clone)]
pub struct EmbeddingClient {
    /// Full URL of the `/v1/embeddings` endpoint (e.g. `http://localhost:8080/v1/embeddings`).
    base_url: String,
    /// Model name sent with each request so llama-server can route to the right model.
    model: String,
    /// Shared HTTP client for connection pooling.
    client: reqwest::Client,
}

impl EmbeddingClient {
    /// Create a new embedding client.
    ///
    /// `base_url` must point at the `/v1/embeddings` endpoint (path included).
    /// `model` is sent in the request body and identifies which loaded model
    /// should generate the embedding (e.g. `"BGE-M3"`).
    pub fn new(base_url: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            model: model.into(),
            client: reqwest::Client::new(),
        }
    }

    /// Generate an embedding vector for the given text.
    ///
    /// Sends a POST request to the llama.cpp `/v1/embeddings` endpoint with
    /// `{ "input": text, "model": self.model }`. Returns the 1024-dimensional
    /// float vector found at `data[0].embedding` in the response.
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP request fails, the server returns a non-OK
    /// status, the response is empty, or it cannot be parsed.
    pub async fn generate_embedding(&self, text: &str) -> anyhow::Result<Vec<f32>> {
        let request = EmbeddingRequest {
            input: text.to_string(),
            model: self.model.clone(),
        };

        let response = self
            .client
            .post(&self.base_url)
            .json(&request)
            .send()
            .await
            .context("failed to send embedding request")?;

        // Check HTTP status before attempting to parse
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("embedding request failed with status {status}: {body}");
        }

        let embedding_response: EmbeddingResponse = response
            .json()
            .await
            .context("failed to parse embedding response")?;

        embedding_response
            .data
            .into_iter()
            .next()
            .map(|d| d.embedding)
            .context("embedding response contained no data entries")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Build a mock response matching the live llama-server OpenAI-compat shape.
    fn live_shape_response(embedding: Vec<f32>) -> String {
        json!({
            "model": "BGE-M3",
            "object": "list",
            "usage": { "prompt_tokens": 3, "total_tokens": 3 },
            "data": [{ "embedding": embedding, "index": 0, "object": "embedding" }]
        })
        .to_string()
    }

    #[tokio::test]
    async fn test_generate_embedding_success() {
        let mock_response = live_shape_response(vec![0.1_f32; 1024]);

        let mut mock_server = mockito::Server::new_async().await;
        let mock_endpoint = mock_server
            .mock("POST", "/")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(mock_response)
            .create();

        let client = EmbeddingClient::new(mock_server.url(), "BGE-M3");
        let result = client.generate_embedding("hello world").await;

        assert!(result.is_ok(), "expected Ok, got {result:?}");
        let embedding = result.unwrap();
        assert_eq!(embedding.len(), 1024);
        assert!(embedding.iter().all(|&v| (v - 0.1).abs() < f32::EPSILON));

        mock_endpoint.assert();
    }

    /// Reproduces the user-reported error (404 with JSON error body). Client
    /// must surface a clean status error, not a parse error.
    #[tokio::test]
    async fn test_generate_embedding_html_404() {
        let mut mock_server = mockito::Server::new_async().await;
        let mock_endpoint = mock_server
            .mock("POST", "/")
            .with_status(404)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "error": {
                        "message": "File Not Found",
                        "type": "not_found_error",
                        "code": 404
                    }
                })
                .to_string(),
            )
            .create();

        let client = EmbeddingClient::new(mock_server.url(), "BGE-M3");
        let err = client.generate_embedding("hello").await.unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("404"), "expected 404 in error: {msg}");
        assert!(
            msg.contains("File Not Found"),
            "expected body in error: {msg}"
        );

        mock_endpoint.assert();
    }

    #[tokio::test]
    async fn test_generate_embedding_http_error() {
        let mut mock_server = mockito::Server::new_async().await;
        let _mock_endpoint = mock_server
            .mock("POST", "/")
            .with_status(500)
            .with_body("Internal Server Error")
            .create();

        let client = EmbeddingClient::new(mock_server.url(), "BGE-M3");
        let result = client.generate_embedding("test").await;

        assert!(result.is_err(), "expected error for 500 status");
        let err = result.unwrap_err();
        let err_msg = format!("{err:#}");
        assert!(err_msg.contains("500"), "expected 500 in error: {err_msg}");
    }

    #[tokio::test]
    async fn test_generate_embedding_empty_text() {
        let mock_response = live_shape_response(vec![0.0_f32; 1024]);

        let mut mock_server = mockito::Server::new_async().await;
        let mock_endpoint = mock_server
            .mock("POST", "/")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(mock_response)
            .create();

        let client = EmbeddingClient::new(mock_server.url(), "BGE-M3");
        let result = client.generate_embedding("").await;

        assert!(result.is_ok(), "expected Ok, got {result:?}");
        let embedding = result.unwrap();
        assert_eq!(embedding.len(), 1024);

        mock_endpoint.assert();
    }

    /// Live-shape response with empty `data` must surface a clean error
    /// instead of panicking on `.next()`.
    #[tokio::test]
    async fn test_generate_embedding_empty_data() {
        let body = json!({
            "model": "BGE-M3",
            "object": "list",
            "data": []
        })
        .to_string();

        let mut mock_server = mockito::Server::new_async().await;
        let mock_endpoint = mock_server
            .mock("POST", "/")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(body)
            .create();

        let client = EmbeddingClient::new(mock_server.url(), "BGE-M3");
        let err = client.generate_embedding("hi").await.unwrap_err();
        assert!(
            err.to_string().contains("no data"),
            "expected 'no data' message: {err}"
        );

        mock_endpoint.assert();
    }
}
