//! Embedding client module.
//!
//! HTTP client for llama.cpp embedding server (BGE-M3).
//! Generates embeddings via the OpenAI-compatible `/v1/embeddings` endpoint.

use anyhow::Context;
use serde::{Deserialize, Serialize};

/// Response from the llama.cpp `/v1/embeddings` endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingResponse {
    pub embedding: Vec<f32>,
}

/// Request payload sent to the `/v1/embeddings` endpoint.
#[derive(Debug, Serialize)]
struct EmbeddingRequest {
    content: String,
}

/// HTTP client for generating embeddings via llama.cpp.
#[derive(Debug, Clone)]
pub struct EmbeddingClient {
    /// Base URL of the embedding service (e.g., `http://localhost:8080/v1/embeddings`).
    base_url: String,
    /// Shared HTTP client for connection pooling.
    client: reqwest::Client,
}

impl EmbeddingClient {
    /// Create a new embedding client with the given service URL.
    ///
    /// The `base_url` should point directly to the `/v1/embeddings` endpoint.
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            client: reqwest::Client::new(),
        }
    }

    /// Generate an embedding vector for the given text.
    ///
    /// Sends a POST request to the llama.cpp `/v1/embeddings` endpoint.
    /// Returns a 1024-dimensional float vector (BGE-M3 default).
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP request fails, the server returns a non-OK
    /// status, or the response cannot be parsed.
    pub async fn generate_embedding(&self, text: &str) -> anyhow::Result<Vec<f32>> {
        let request = EmbeddingRequest {
            content: text.to_string(),
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
            anyhow::bail!("embedding request failed with status {status}: {body}",);
        }

        let embedding_response: EmbeddingResponse = response
            .json()
            .await
            .context("failed to parse embedding response")?;

        Ok(embedding_response.embedding)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A simple mock HTTP server that returns a fake embedding.
    /// Uses a local TCP port to avoid external dependencies.
    #[tokio::test]
    async fn test_generate_embedding_success() {
        let mock_response = json!({
            "embedding": vec![0.1_f32; 1024]
        });

        let mut mock_server = mockito::Server::new_async().await;
        let mock_endpoint = mock_server
            .mock("POST", "/")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(mock_response.to_string())
            .create();

        let client = EmbeddingClient::new(mock_server.url());
        let result = client.generate_embedding("hello world").await;

        assert!(result.is_ok(), "expected Ok, got {result:?}");
        let embedding = result.unwrap();
        assert_eq!(embedding.len(), 1024);
        assert!(embedding.iter().all(|&v| (v - 0.1).abs() < f32::EPSILON));

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

        let client = EmbeddingClient::new(mock_server.url());
        let result = client.generate_embedding("test").await;

        assert!(result.is_err(), "expected error for 500 status");
        let err = result.unwrap_err();
        let err_msg = format!("{err:#}");
        assert!(err_msg.contains("500"), "expected 500 in error: {err_msg}");
    }

    #[tokio::test]
    async fn test_generate_embedding_empty_text() {
        let mock_response = json!({
            "embedding": vec![0.0_f32; 1024]
        });

        let mut mock_server = mockito::Server::new_async().await;
        let mock_endpoint = mock_server
            .mock("POST", "/")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(mock_response.to_string())
            .create();

        let client = EmbeddingClient::new(mock_server.url());
        let result = client.generate_embedding("").await;

        assert!(result.is_ok(), "expected Ok, got {result:?}");
        let embedding = result.unwrap();
        assert_eq!(embedding.len(), 1024);

        mock_endpoint.assert();
    }
}
