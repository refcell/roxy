//! Backend traits for RPC forwarding and health tracking.

use std::time::{Duration, Instant};

use alloy_primitives::BlockNumber;

/// Health status of a backend.
#[derive(Debug, Clone, Copy)]
pub enum HealthStatus {
    /// Backend is healthy.
    Healthy,
    /// Backend is degraded with high latency.
    Degraded {
        /// Current latency EMA.
        latency_ema: Duration,
    },
    /// Backend is unhealthy with high error rate.
    Unhealthy {
        /// Current error rate (0.0 to 1.0).
        error_rate: f64,
    },
    /// Backend is temporarily banned.
    Banned {
        /// Time until the ban expires.
        until: Instant,
    },
}

/// Backend identity/metadata trait.
///
/// Backends are `tower::Service<RequestPacket, Response=ResponsePacket, Error=RoxyError>`
/// implementations that also implement this trait for identity information.
pub trait BackendMeta: Send + Sync + 'static {
    /// Backend identifier.
    fn name(&self) -> &str;

    /// RPC endpoint URL.
    fn rpc_url(&self) -> &str;
}

/// Health tracking with EMA.
pub trait HealthTracker: Send + Sync {
    /// Record a request result.
    fn record(&mut self, duration: Duration, success: bool);

    /// Get latency EMA.
    fn latency_ema(&self) -> Duration;

    /// Get error rate (0.0 to 1.0).
    fn error_rate(&self) -> f64;

    /// Get current health status.
    fn status(&self) -> HealthStatus;
}

/// Consensus tracking across backends.
pub trait ConsensusTracker: Send + Sync {
    /// Update a backend's reported block.
    fn update(&mut self, backend: &str, height: BlockNumber);

    /// Get the latest reported block (any backend).
    fn latest(&self) -> BlockNumber;

    /// Get the safe block (majority agree).
    fn safe(&self) -> BlockNumber;

    /// Get the finalized block (Byzantine-safe, f+1 agree).
    fn finalized(&self) -> BlockNumber;
}
