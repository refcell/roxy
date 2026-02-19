//! Health recording tower layer.
//!
//! Wraps a backend service to record latency and success/failure
//! after each request, updating the shared `EmaHealthTracker`.

use std::{
    sync::Arc,
    task::{Context, Poll},
    time::Instant,
};

use alloy_json_rpc::{RequestPacket, ResponsePacket};
use futures::future::BoxFuture;
use roxy_traits::HealthTracker;
use roxy_types::RoxyError;
use tokio::sync::RwLock;
use tower::{Layer, Service};

use crate::health::EmaHealthTracker;

/// Shared health tracker state.
pub type SharedHealth = Arc<RwLock<EmaHealthTracker>>;

/// Tower layer that records health metrics (latency, success/failure)
/// after each request completes.
#[derive(Debug, Clone)]
pub struct HealthRecordingLayer {
    health: SharedHealth,
}

impl HealthRecordingLayer {
    /// Create a new health recording layer with the given shared health tracker.
    #[must_use]
    pub const fn new(health: SharedHealth) -> Self {
        Self { health }
    }
}

impl<S> Layer<S> for HealthRecordingLayer {
    type Service = HealthRecordingService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        HealthRecordingService { inner, health: self.health.clone() }
    }
}

/// Tower service that wraps another service and records health metrics.
#[derive(Debug, Clone)]
pub struct HealthRecordingService<S> {
    inner: S,
    health: SharedHealth,
}

impl<S> Service<RequestPacket> for HealthRecordingService<S>
where
    S: Service<RequestPacket, Response = ResponsePacket, Error = RoxyError>
        + Clone
        + Send
        + 'static,
    S::Future: Send,
{
    type Response = ResponsePacket;
    type Error = RoxyError;
    type Future = BoxFuture<'static, Result<ResponsePacket, RoxyError>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: RequestPacket) -> Self::Future {
        let mut inner = self.inner.clone();
        let health = self.health.clone();

        Box::pin(async move {
            let start = Instant::now();
            let result = inner.call(request).await;
            let duration = start.elapsed();

            let success = result.is_ok();
            health.write().await.record(duration, success);

            result
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::health::HealthConfig;
    use roxy_traits::HealthTracker;

    /// A simple mock service for testing the health layer.
    #[derive(Clone)]
    struct MockService {
        succeed: bool,
    }

    impl Service<RequestPacket> for MockService {
        type Response = ResponsePacket;
        type Error = RoxyError;
        type Future = BoxFuture<'static, Result<ResponsePacket, RoxyError>>;

        fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }

        fn call(&mut self, _request: RequestPacket) -> Self::Future {
            let succeed = self.succeed;
            Box::pin(async move {
                if succeed {
                    let json = r#"{"jsonrpc":"2.0","id":1,"result":"0x1"}"#;
                    Ok(serde_json::from_str(json).unwrap())
                } else {
                    Err(RoxyError::BackendOffline { backend: "mock".to_string() })
                }
            })
        }
    }

    fn make_request() -> RequestPacket {
        use alloy_json_rpc::{Id, Request};
        let req: Request<()> = Request::new("eth_blockNumber", Id::Number(1), ());
        RequestPacket::Single(req.serialize().unwrap())
    }

    #[tokio::test]
    async fn test_records_success() {
        let health = Arc::new(RwLock::new(EmaHealthTracker::new(HealthConfig::default())));
        let layer = HealthRecordingLayer::new(health.clone());
        let mut svc = layer.layer(MockService { succeed: true });

        let result = svc.call(make_request()).await;
        assert!(result.is_ok());

        let h = health.read().await;
        assert!(h.latency_ema() > std::time::Duration::ZERO);
    }

    #[tokio::test]
    async fn test_records_failure() {
        let config = HealthConfig { min_requests: 1, ..Default::default() };
        let health = Arc::new(RwLock::new(EmaHealthTracker::new(config)));
        let layer = HealthRecordingLayer::new(health.clone());
        let mut svc = layer.layer(MockService { succeed: false });

        let result = svc.call(make_request()).await;
        assert!(result.is_err());

        let h = health.read().await;
        assert!(h.error_rate() > 0.0);
    }
}
