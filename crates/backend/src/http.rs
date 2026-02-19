//! HTTP backend implementation.

use std::task::{Context, Poll};

use alloy_json_rpc::{RequestPacket, ResponsePacket};
use futures::future::BoxFuture;
use roxy_traits::BackendMeta;
use roxy_types::RoxyError;
use std::sync::Arc;
use tower::Service;

/// Configuration for HTTP backend.
#[derive(Debug, Clone)]
pub struct BackendConfig {
    /// Maximum batch size.
    pub max_batch_size: usize,
}

impl Default for BackendConfig {
    fn default() -> Self {
        Self { max_batch_size: 100 }
    }
}

/// HTTP backend for RPC forwarding.
///
/// This is a bare HTTP backend that performs the HTTP POST + JSON deserialize.
/// Retry, timeout, and health recording are handled by tower layers.
#[derive(Debug, Clone)]
pub struct HttpBackend {
    name: Arc<str>,
    rpc_url: Arc<str>,
    client: reqwest::Client,
    _config: BackendConfig,
}

impl HttpBackend {
    /// Create a new HTTP backend.
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP client fails to build.
    pub fn new(name: String, rpc_url: String, config: BackendConfig) -> Result<Self, RoxyError> {
        let client = reqwest::Client::builder()
            .build()
            .map_err(|e| RoxyError::Internal(format!("failed to build HTTP client: {e}")))?;

        Ok(Self {
            name: Arc::from(name.as_str()),
            rpc_url: Arc::from(rpc_url.as_str()),
            client,
            _config: config,
        })
    }
}

impl BackendMeta for HttpBackend {
    fn name(&self) -> &str {
        &self.name
    }

    fn rpc_url(&self) -> &str {
        &self.rpc_url
    }
}

impl Service<RequestPacket> for HttpBackend {
    type Response = ResponsePacket;
    type Error = RoxyError;
    type Future = BoxFuture<'static, Result<ResponsePacket, RoxyError>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: RequestPacket) -> Self::Future {
        let client = self.client.clone();
        let rpc_url = self.rpc_url.clone();
        let name = self.name.clone();

        Box::pin(async move {
            let response = client
                .post(rpc_url.as_ref())
                .json(&request)
                .send()
                .await
                .map_err(|_| RoxyError::BackendOffline { backend: name.to_string() })?;

            if !response.status().is_success() {
                return Err(RoxyError::BackendOffline { backend: name.to_string() });
            }

            response
                .json()
                .await
                .map_err(|e| RoxyError::Internal(format!("failed to parse response: {e}")))
        })
    }
}
