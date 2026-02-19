//! Load balancer traits for backend selection.

use std::sync::Arc;

use crate::backend::BackendMeta;

/// Load balancer trait for selecting backends.
pub trait LoadBalancer: Send + Sync {
    /// Select a single backend.
    fn select(&self, backends: &[Arc<dyn BackendMeta>]) -> Option<Arc<dyn BackendMeta>>;

    /// Order backends by preference for failover.
    fn select_ordered(&self, backends: &[Arc<dyn BackendMeta>]) -> Vec<Arc<dyn BackendMeta>>;
}
