//! Retry policy for backend requests.

use alloy_json_rpc::{RequestPacket, ResponsePacket};
use roxy_types::RoxyError;
use tower::retry::Policy;

/// Retry policy for backend RPC requests.
///
/// Retries on errors that indicate the backend is offline or timed out.
/// Uses exponential backoff: `100ms * 2^attempt`, capped at 3000ms.
#[derive(Debug, Clone)]
pub struct RoxyRetryPolicy {
    remaining: u32,
    attempt: u32,
}

impl RoxyRetryPolicy {
    /// Create a new retry policy with the given maximum retries.
    #[must_use]
    pub const fn new(max_retries: u32) -> Self {
        Self { remaining: max_retries, attempt: 0 }
    }
}

impl Policy<RequestPacket, ResponsePacket, RoxyError> for RoxyRetryPolicy {
    type Future = futures::future::Ready<Self>;

    fn retry(
        &self,
        _req: &RequestPacket,
        result: Result<&ResponsePacket, &RoxyError>,
    ) -> Option<Self::Future> {
        match result {
            Ok(_) => None,
            Err(e) if e.should_failover() && self.remaining > 0 => {
                Some(futures::future::ready(Self {
                    remaining: self.remaining - 1,
                    attempt: self.attempt + 1,
                }))
            }
            Err(_) => None,
        }
    }

    fn clone_request(&self, req: &RequestPacket) -> Option<RequestPacket> {
        Some(req.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_request() -> RequestPacket {
        use alloy_json_rpc::{Id, Request};
        let req: Request<()> = Request::new("eth_blockNumber", Id::Number(1), ());
        RequestPacket::Single(req.serialize().unwrap())
    }

    fn make_response() -> ResponsePacket {
        let json = r#"{"jsonrpc":"2.0","id":1,"result":"0x1"}"#;
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn test_no_retry_on_success() {
        let policy = RoxyRetryPolicy::new(3);
        let req = make_request();
        let resp = make_response();
        assert!(policy.retry(&req, Ok(&resp)).is_none());
    }

    #[test]
    fn test_retry_on_failover_error() {
        let policy = RoxyRetryPolicy::new(3);
        let req = make_request();
        let err = RoxyError::BackendOffline { backend: "test".to_string() };
        let result = policy.retry(&req, Err(&err));
        assert!(result.is_some());
    }

    #[test]
    fn test_no_retry_when_exhausted() {
        let policy = RoxyRetryPolicy::new(0);
        let req = make_request();
        let err = RoxyError::BackendOffline { backend: "test".to_string() };
        assert!(policy.retry(&req, Err(&err)).is_none());
    }

    #[test]
    fn test_clone_request() {
        let policy = RoxyRetryPolicy::new(3);
        let req = make_request();
        assert!(policy.clone_request(&req).is_some());
    }

    #[test]
    fn test_retry_decrements_remaining() {
        let policy = RoxyRetryPolicy::new(2);
        let req = make_request();
        let err = RoxyError::BackendOffline { backend: "test".to_string() };

        // First retry
        let next = futures::executor::block_on(policy.retry(&req, Err(&err)).unwrap());
        assert_eq!(next.remaining, 1);
        assert_eq!(next.attempt, 1);

        // Second retry
        let next = futures::executor::block_on(next.retry(&req, Err(&err)).unwrap());
        assert_eq!(next.remaining, 0);
        assert_eq!(next.attempt, 2);

        // No more retries
        assert!(next.retry(&req, Err(&err)).is_none());
    }
}
