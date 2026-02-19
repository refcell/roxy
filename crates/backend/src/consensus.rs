//! Shared consensus state and background poller.
//!
//! Periodically polls `eth_blockNumber` from each backend and maintains
//! Byzantine-safe block height state via [`SafeTip`].

use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use alloy_json_rpc::{Id, Request, RequestPacket};
use alloy_primitives::BlockNumber;
use roxy_traits::BackendMeta;
use tokio::sync::Mutex;

use crate::SafeTip;
use crate::group::BoxedBackend;

/// Shared consensus state with atomic block heights.
///
/// Updated by the background consensus poller and read by the
/// block-rewrite layer.
#[derive(Debug)]
pub struct ConsensusState {
    pub(crate) safe_block: AtomicU64,
    pub(crate) latest_block: AtomicU64,
    pub(crate) finalized_block: AtomicU64,
}

impl Default for ConsensusState {
    fn default() -> Self {
        Self::new()
    }
}

impl ConsensusState {
    /// Create a new consensus state with all blocks at 0.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            safe_block: AtomicU64::new(0),
            latest_block: AtomicU64::new(0),
            finalized_block: AtomicU64::new(0),
        }
    }

    /// Get the safe block number (f+1 honest backends agree).
    #[must_use]
    pub fn safe_block(&self) -> BlockNumber {
        self.safe_block.load(Ordering::Relaxed)
    }

    /// Get the latest reported block number (any backend).
    #[must_use]
    pub fn latest_block(&self) -> BlockNumber {
        self.latest_block.load(Ordering::Relaxed)
    }

    /// Get the finalized block number.
    #[must_use]
    pub fn finalized_block(&self) -> BlockNumber {
        self.finalized_block.load(Ordering::Relaxed)
    }

    /// Update the consensus state from a SafeTip tracker.
    pub fn update_from_safe_tip(&self, safe_tip: &SafeTip) {
        self.safe_block.store(safe_tip.get(), Ordering::Relaxed);
        self.latest_block.store(safe_tip.latest(), Ordering::Relaxed);
        // Finalized tracks safe for now (could be enhanced with
        // eth_getBlockByNumber("finalized") polling).
        self.finalized_block.store(safe_tip.get(), Ordering::Relaxed);
    }
}

/// Background consensus poller that periodically queries backends for
/// `eth_blockNumber` and updates shared [`ConsensusState`].
#[derive(Debug)]
pub struct ConsensusPoller {
    backends: Vec<BoxedBackend>,
    safe_tip: Mutex<SafeTip>,
    state: Arc<ConsensusState>,
    interval: std::time::Duration,
}

impl ConsensusPoller {
    /// Create a new consensus poller.
    ///
    /// # Arguments
    ///
    /// * `backends` - Backends to poll
    /// * `byzantine_f` - Maximum number of Byzantine faulty backends
    /// * `state` - Shared consensus state to update
    /// * `interval` - Polling interval
    #[must_use]
    pub fn new(
        backends: Vec<BoxedBackend>,
        byzantine_f: usize,
        state: Arc<ConsensusState>,
        interval: std::time::Duration,
    ) -> Self {
        Self {
            backends,
            safe_tip: Mutex::new(SafeTip::new(byzantine_f)),
            state,
            interval,
        }
    }

    /// Run the consensus poller as a background task.
    ///
    /// This runs indefinitely, polling all backends every `interval`.
    pub async fn run(self: Arc<Self>) {
        let mut ticker = tokio::time::interval(self.interval);
        loop {
            ticker.tick().await;
            self.poll_once().await;
        }
    }

    /// Perform a single polling round.
    async fn poll_once(&self) {
        for backend in &self.backends {
            let name = backend.name().to_string();
            let request = Self::block_number_request();
            let fut = backend.clone_and_call(request);
            match fut.await {
                Ok(response) => {
                    if let Some(height) = Self::parse_block_number(&response) {
                        let mut tip = self.safe_tip.lock().await;
                        tip.update(&name, height);
                        self.state.update_from_safe_tip(&tip);
                    }
                }
                Err(_) => {
                    // Backend unavailable; skip this round for it.
                }
            }
        }
    }

    /// Build an `eth_blockNumber` request packet.
    fn block_number_request() -> RequestPacket {
        let req: Request<()> = Request::new("eth_blockNumber", Id::Number(1), ());
        RequestPacket::Single(req.serialize().expect("eth_blockNumber serialization"))
    }

    /// Parse a hex block number from a JSON-RPC response.
    fn parse_block_number(response: &alloy_json_rpc::ResponsePacket) -> Option<BlockNumber> {
        use alloy_json_rpc::{ResponsePacket, ResponsePayload};
        match response {
            ResponsePacket::Single(resp) => match &resp.payload {
                ResponsePayload::Success(val) => {
                    let s = val.get().trim().trim_matches('"');
                    u64::from_str_radix(s.strip_prefix("0x").unwrap_or(s), 16).ok()
                }
                _ => None,
            },
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_consensus_state_defaults() {
        let state = ConsensusState::new();
        assert_eq!(state.safe_block(), 0);
        assert_eq!(state.latest_block(), 0);
        assert_eq!(state.finalized_block(), 0);
    }

    #[test]
    fn test_consensus_state_update_from_safe_tip() {
        let state = ConsensusState::new();
        let mut tip = SafeTip::new(0);
        tip.update("b1", 100);
        state.update_from_safe_tip(&tip);

        assert_eq!(state.safe_block(), 100);
        assert_eq!(state.latest_block(), 100);
        assert_eq!(state.finalized_block(), 100);
    }

    #[test]
    fn test_consensus_state_byzantine() {
        let state = ConsensusState::new();
        let mut tip = SafeTip::new(1);
        tip.update("b1", 100);
        tip.update("b2", 100);
        tip.update("b3", 200);
        state.update_from_safe_tip(&tip);

        assert_eq!(state.safe_block(), 100);
        assert_eq!(state.latest_block(), 200);
    }

    #[test]
    fn test_parse_block_number() {
        use alloy_json_rpc::{Response, ResponsePayload};
        use serde_json::value::RawValue;

        let raw = RawValue::from_string("\"0x64\"".to_string()).unwrap();
        let resp = Response { id: Id::Number(1), payload: ResponsePayload::Success(raw) };
        let packet = alloy_json_rpc::ResponsePacket::Single(resp);

        let result = ConsensusPoller::parse_block_number(&packet);
        assert_eq!(result, Some(100));
    }
}
