//! Load balancer implementations.

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use roxy_traits::{BackendMeta, LoadBalancer};

/// EMA-based load balancer - lower latency = higher priority.
///
/// Note: With the tower refactoring, health and latency data will be
/// provided by layers. For now, this simply returns all backends in order.
#[derive(Debug, Default)]
pub struct EmaLoadBalancer;

impl LoadBalancer for EmaLoadBalancer {
    fn select(&self, backends: &[Arc<dyn BackendMeta>]) -> Option<Arc<dyn BackendMeta>> {
        self.select_ordered(backends).into_iter().next()
    }

    fn select_ordered(&self, backends: &[Arc<dyn BackendMeta>]) -> Vec<Arc<dyn BackendMeta>> {
        backends.to_vec()
    }
}

/// Round-robin load balancer.
#[derive(Debug)]
pub struct RoundRobinBalancer {
    index: AtomicUsize,
}

impl RoundRobinBalancer {
    /// Create a new round-robin balancer.
    #[must_use]
    pub const fn new() -> Self {
        Self { index: AtomicUsize::new(0) }
    }
}

impl Default for RoundRobinBalancer {
    fn default() -> Self {
        Self::new()
    }
}

impl LoadBalancer for RoundRobinBalancer {
    fn select(&self, backends: &[Arc<dyn BackendMeta>]) -> Option<Arc<dyn BackendMeta>> {
        if backends.is_empty() {
            return None;
        }

        let idx = self.index.fetch_add(1, Ordering::Relaxed);
        Some(backends[idx % backends.len()].clone())
    }

    fn select_ordered(&self, backends: &[Arc<dyn BackendMeta>]) -> Vec<Arc<dyn BackendMeta>> {
        if backends.is_empty() {
            return Vec::new();
        }

        let idx = self.index.fetch_add(1, Ordering::Relaxed);

        let mut result = Vec::with_capacity(backends.len());
        for i in 0..backends.len() {
            result.push(backends[(idx + i) % backends.len()].clone());
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mock backend metadata for testing.
    struct MockMeta {
        name: String,
    }

    impl MockMeta {
        fn create(name: &str) -> Arc<dyn BackendMeta> {
            Arc::new(Self { name: name.to_string() })
        }
    }

    impl std::fmt::Debug for MockMeta {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("MockMeta").field("name", &self.name).finish()
        }
    }

    impl BackendMeta for MockMeta {
        fn name(&self) -> &str {
            &self.name
        }

        fn rpc_url(&self) -> &str {
            "http://mock"
        }
    }

    #[test]
    fn test_ema_load_balancer_empty() {
        let lb = EmaLoadBalancer;
        let backends: Vec<Arc<dyn BackendMeta>> = vec![];

        assert!(lb.select(&backends).is_none());
        assert!(lb.select_ordered(&backends).is_empty());
    }

    #[test]
    fn test_ema_load_balancer_single() {
        let lb = EmaLoadBalancer;
        let backends = vec![MockMeta::create("b1")];

        let selected = lb.select(&backends);
        assert!(selected.is_some());
        assert_eq!(selected.unwrap().name(), "b1");
    }

    #[test]
    fn test_ema_load_balancer_returns_all() {
        let lb = EmaLoadBalancer;
        let backends = vec![
            MockMeta::create("slow"),
            MockMeta::create("fast"),
            MockMeta::create("medium"),
        ];

        let ordered = lb.select_ordered(&backends);
        assert_eq!(ordered.len(), 3);
    }

    #[test]
    fn test_round_robin_empty() {
        let lb = RoundRobinBalancer::new();
        let backends: Vec<Arc<dyn BackendMeta>> = vec![];

        assert!(lb.select(&backends).is_none());
    }

    #[test]
    fn test_round_robin_rotates() {
        let lb = RoundRobinBalancer::new();
        let backends = vec![
            MockMeta::create("b1"),
            MockMeta::create("b2"),
            MockMeta::create("b3"),
        ];

        let s1 = lb.select(&backends).unwrap();
        let s2 = lb.select(&backends).unwrap();
        let s3 = lb.select(&backends).unwrap();
        let s4 = lb.select(&backends).unwrap();

        // Should rotate through backends
        assert_eq!(s1.name(), "b1");
        assert_eq!(s2.name(), "b2");
        assert_eq!(s3.name(), "b3");
        assert_eq!(s4.name(), "b1"); // Wraps around
    }
}
