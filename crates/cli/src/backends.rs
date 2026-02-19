//! Backend creation utilities.
//!
//! This module provides factory types for creating HTTP backends from configuration.

use std::{collections::HashMap, sync::Arc, time::Duration};

use eyre::Result;
use roxy_backend::{
    BackendConfig as HttpBackendConfig, BoxedBackend, EmaHealthTracker, HealthConfig,
    HealthRecordingLayer, HttpBackend, RoxyRetryPolicy,
};
use roxy_config::RoxyConfig;
use roxy_types::RoxyError;
use tokio::sync::RwLock;
use tower::ServiceBuilder;

/// Factory for creating HTTP backends from configuration.
///
/// # Example
///
/// ```ignore
/// use roxyproxy_cli::BackendFactory;
///
/// let factory = BackendFactory::new().with_batch_size(200);
/// let backends = factory.create(&config)?;
/// ```
#[derive(Debug, Clone)]
pub struct BackendFactory {
    default_batch_size: usize,
}

impl Default for BackendFactory {
    fn default() -> Self {
        Self::new()
    }
}

impl BackendFactory {
    /// Create a new backend factory with default settings.
    #[must_use]
    pub const fn new() -> Self {
        Self { default_batch_size: 100 }
    }

    /// Set the default batch size for backends.
    #[must_use]
    pub const fn with_batch_size(mut self, size: usize) -> Self {
        self.default_batch_size = size;
        self
    }

    /// Create backends from configuration.
    ///
    /// Each backend is composed with tower layers:
    /// - Health recording (outermost) - records latency and success/failure
    /// - Timeout - enforces per-request timeout
    /// - Retry - retries on transient errors with exponential backoff
    /// - Raw HTTP backend (innermost) - performs the actual HTTP POST
    ///
    /// # Errors
    ///
    /// Returns an error if backend creation fails.
    pub fn create(&self, config: &RoxyConfig) -> Result<HashMap<String, BoxedBackend>> {
        let mut backends = HashMap::new();

        for backend_config in &config.backends {
            let http_config = HttpBackendConfig {
                max_batch_size: self.default_batch_size,
            };

            let raw_backend = HttpBackend::new(
                backend_config.name.clone(),
                backend_config.url.clone(),
                http_config,
            )?;

            let timeout = Duration::from_millis(backend_config.timeout_ms);
            let retry_policy = RoxyRetryPolicy::new(backend_config.max_retries);
            let health = Arc::new(RwLock::new(EmaHealthTracker::new(HealthConfig::default())));

            let backend_name_for_err = backend_config.name.clone();

            // Compose layers: health recording -> map_err -> timeout -> retry -> raw backend
            // The timeout layer returns Box<dyn Error>, so we map it back to RoxyError.
            let layered = ServiceBuilder::new()
                .layer(HealthRecordingLayer::new(health))
                .map_err(move |err: Box<dyn std::error::Error + Send + Sync>| {
                    if err.is::<tower::timeout::error::Elapsed>() {
                        RoxyError::BackendTimeout { backend: backend_name_for_err.clone() }
                    } else {
                        RoxyError::Internal(err.to_string())
                    }
                })
                .layer(tower::timeout::TimeoutLayer::new(timeout))
                .layer(tower::retry::RetryLayer::new(retry_policy))
                .service(raw_backend);

            let boxed = BoxedBackend::from_service(
                &backend_config.name,
                &backend_config.url,
                layered,
            );

            backends.insert(backend_config.name.clone(), boxed);
            trace!(name = %backend_config.name, url = %backend_config.url, "Created backend");
        }

        Ok(backends)
    }
}

/// Create HTTP backends from configuration.
///
/// This is a convenience function that creates a [`BackendFactory`] and calls
/// [`BackendFactory::create`].
///
/// # Arguments
///
/// * `config` - The Roxy configuration
///
/// # Returns
///
/// A map of backend names to boxed backends.
///
/// # Errors
///
/// Returns an error if backend creation fails.
pub fn create_backends(config: &RoxyConfig) -> Result<HashMap<String, BoxedBackend>> {
    BackendFactory::new().create(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutils::minimal_config;

    #[test]
    fn test_create_backends() {
        let config = minimal_config();
        let backends = create_backends(&config).unwrap();
        assert_eq!(backends.len(), 1);
        assert!(backends.contains_key("primary"));
    }

    #[test]
    fn test_backend_factory_with_batch_size() {
        let factory = BackendFactory::new().with_batch_size(200);
        assert_eq!(factory.default_batch_size, 200);
    }

    #[test]
    fn test_backend_factory_default() {
        let factory = BackendFactory::default();
        assert_eq!(factory.default_batch_size, 100);
    }
}
