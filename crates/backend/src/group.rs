//! Backend group with failover.

use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use alloy_json_rpc::{RequestPacket, ResponsePacket};
use derive_more::Debug;
use futures::future::BoxFuture;
use roxy_traits::{BackendMeta, LoadBalancer};
use roxy_types::RoxyError;
use tower::Service;

/// Response from a backend group.
#[derive(Debug)]
pub struct BackendResponse {
    /// The response packet.
    pub response: ResponsePacket,
    /// Name of the backend that served the request.
    pub served_by: String,
}

/// A type-erased backend that combines service + metadata.
///
/// This type is `Clone + Send + Sync` so it can be stored in shared state.
/// Each call clones the inner service to get an independent copy.
pub struct BoxedBackend {
    name: Arc<str>,
    rpc_url: Arc<str>,
    service: Arc<Mutex<tower::util::BoxCloneService<RequestPacket, ResponsePacket, RoxyError>>>,
}

impl Clone for BoxedBackend {
    fn clone(&self) -> Self {
        // Clone the inner BoxCloneService to get an independent copy
        let inner = self.service.lock().unwrap().clone();
        Self {
            name: self.name.clone(),
            rpc_url: self.rpc_url.clone(),
            service: Arc::new(Mutex::new(inner)),
        }
    }
}

impl std::fmt::Debug for BoxedBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BoxedBackend").field("name", &self.name).finish()
    }
}

impl BoxedBackend {
    /// Create a new boxed backend from a service that implements `BackendMeta`.
    pub fn new<S>(svc: S) -> Self
    where
        S: Service<RequestPacket, Response = ResponsePacket, Error = RoxyError>
            + BackendMeta
            + Clone
            + Send
            + 'static,
        S::Future: Send + 'static,
    {
        let name = Arc::from(svc.name());
        let rpc_url = Arc::from(svc.rpc_url());
        Self {
            name,
            rpc_url,
            service: Arc::new(Mutex::new(tower::util::BoxCloneService::new(svc))),
        }
    }

    /// Create a new boxed backend from a service with explicit metadata.
    ///
    /// Use this when the service has been wrapped in tower layers and
    /// no longer implements `BackendMeta` directly.
    pub fn from_service<S>(name: &str, rpc_url: &str, svc: S) -> Self
    where
        S: Service<RequestPacket, Response = ResponsePacket, Error = RoxyError>
            + Clone
            + Send
            + 'static,
        S::Future: Send + 'static,
    {
        Self {
            name: Arc::from(name),
            rpc_url: Arc::from(rpc_url),
            service: Arc::new(Mutex::new(tower::util::BoxCloneService::new(svc))),
        }
    }

    /// Call the underlying service.
    ///
    /// Clones the inner service first (outside async), returns a Send future.
    pub(crate) fn clone_and_call(
        &self,
        request: RequestPacket,
    ) -> impl std::future::Future<Output = Result<ResponsePacket, RoxyError>> + Send {
        let mut svc = self.service.lock().unwrap().clone();
        svc.call(request)
    }
}

impl BackendMeta for BoxedBackend {
    fn name(&self) -> &str {
        &self.name
    }

    fn rpc_url(&self) -> &str {
        &self.rpc_url
    }
}

const _: () = {
    const fn _assert_send_sync<T: Send + Sync>() {}
    const fn _check() {
        _assert_send_sync::<BoxedBackend>();
        _assert_send_sync::<BackendGroup>();
    }
};

/// Shared inner state for a backend group.
#[derive(Debug)]
struct BackendGroupInner {
    name: String,
    #[debug("{} backends", backends.len())]
    backends: Vec<BoxedBackend>,
    #[debug(skip)]
    load_balancer: Arc<dyn LoadBalancer>,
}

/// A group of backends with load balancing and failover.
///
/// Implements `tower::Service<RequestPacket>` with failover across backends.
/// `Clone` is cheap (shared `Arc` state).
#[derive(Debug, Clone)]
pub struct BackendGroup {
    inner: Arc<BackendGroupInner>,
}

impl BackendGroup {
    /// Create a new backend group.
    #[must_use]
    pub fn new(
        name: String,
        backends: Vec<BoxedBackend>,
        load_balancer: Arc<dyn LoadBalancer>,
    ) -> Self {
        Self {
            inner: Arc::new(BackendGroupInner { name, backends, load_balancer }),
        }
    }

    /// Get the group name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.inner.name
    }

    /// Forward a request with failover.
    pub async fn forward(&self, request: RequestPacket) -> Result<BackendResponse, RoxyError> {
        let meta_refs: Vec<Arc<dyn BackendMeta>> = self
            .inner
            .backends
            .iter()
            .map(|b| Arc::new(b.clone()) as Arc<dyn BackendMeta>)
            .collect();

        let ordered = self.inner.load_balancer.select_ordered(&meta_refs);

        if ordered.is_empty() {
            return Err(RoxyError::NoHealthyBackends);
        }

        for meta in ordered {
            let backend_name = meta.name().to_string();
            // Find the matching backend by name, clone it, and call
            if let Some(backend) = self.inner.backends.iter().find(|b| b.name() == backend_name) {
                let fut = backend.clone_and_call(request.clone());
                match fut.await {
                    Ok(response) => {
                        return Ok(BackendResponse {
                            response,
                            served_by: backend_name,
                        });
                    }
                    Err(e) if e.should_failover() => {
                        continue;
                    }
                    Err(e) => {
                        return Err(e);
                    }
                }
            }
        }

        Err(RoxyError::NoHealthyBackends)
    }
}

impl Service<RequestPacket> for BackendGroup {
    type Response = BackendResponse;
    type Error = RoxyError;
    type Future = BoxFuture<'static, Result<BackendResponse, RoxyError>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: RequestPacket) -> Self::Future {
        let this = self.clone();
        Box::pin(async move { this.forward(request).await })
    }
}
