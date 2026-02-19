//! Block-tag rewrite tower layer.
//!
//! Replaces `"latest"`, `"safe"`, and `"finalized"` block tags in
//! JSON-RPC request params with concrete block numbers from
//! [`ConsensusState`].

use std::{
    sync::Arc,
    task::{Context, Poll},
};

use alloy_json_rpc::{RequestPacket, ResponsePacket};
use futures::future::BoxFuture;
use roxy_types::RoxyError;
use tower::{Layer, Service};

use crate::consensus::ConsensusState;

/// Tower layer that rewrites block tags to concrete numbers.
#[derive(Debug, Clone)]
pub struct BlockRewriteLayer {
    consensus: Arc<ConsensusState>,
}

impl BlockRewriteLayer {
    /// Create a new block-rewrite layer.
    #[must_use]
    pub const fn new(consensus: Arc<ConsensusState>) -> Self {
        Self { consensus }
    }
}

impl<S> Layer<S> for BlockRewriteLayer {
    type Service = BlockRewriteService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        BlockRewriteService { inner, consensus: self.consensus.clone() }
    }
}

/// Tower service that rewrites block tags before forwarding.
#[derive(Debug, Clone)]
pub struct BlockRewriteService<S> {
    inner: S,
    consensus: Arc<ConsensusState>,
}

impl<S> Service<RequestPacket> for BlockRewriteService<S>
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
        let consensus = self.consensus.clone();

        Box::pin(async move {
            let rewritten = rewrite_block_tags(request, &consensus);
            inner.call(rewritten).await
        })
    }
}

/// Rewrite a single serialized request's block tags.
fn rewrite_serialized_request(
    req: alloy_json_rpc::SerializedRequest,
    latest: u64,
    safe: u64,
    finalized: u64,
) -> alloy_json_rpc::SerializedRequest {
    let json = req.serialized().to_string();

    let rewritten = json
        .replace("\"latest\"", &format!("\"0x{latest:x}\""))
        .replace("\"safe\"", &format!("\"0x{safe:x}\""))
        .replace("\"finalized\"", &format!("\"0x{finalized:x}\""));

    if rewritten == json {
        return req;
    }

    // Try to parse the rewritten JSON back into a SerializedRequest
    match serde_json::from_str::<serde_json::Value>(&rewritten) {
        Ok(value) => {
            // Re-create the request using the original method, id, and rewritten params
            let method = req.method().to_string();
            let id = req.id().clone();
            // Extract params from the rewritten value
            let params_raw = value
                .get("params")
                .and_then(|p| serde_json::value::RawValue::from_string(p.to_string()).ok())
                .unwrap_or_else(|| {
                    serde_json::value::RawValue::from_string("[]".to_string())
                        .expect("empty array is valid JSON")
                });
            alloy_json_rpc::Request::new(method, id, params_raw)
                .serialize()
                .unwrap_or(req)
        }
        Err(_) => req,
    }
}

/// Rewrite block tags in request params to concrete numbers.
///
/// Replaces `"latest"`, `"safe"`, and `"finalized"` string values in
/// the serialized request with the corresponding hex block number from
/// consensus state.
fn rewrite_block_tags(packet: RequestPacket, consensus: &ConsensusState) -> RequestPacket {
    let latest = consensus.latest_block();
    let safe = consensus.safe_block();
    let finalized = consensus.finalized_block();

    // Skip rewriting if consensus has no data yet
    if latest == 0 {
        return packet;
    }

    match packet {
        RequestPacket::Single(req) => {
            RequestPacket::Single(rewrite_serialized_request(req, latest, safe, finalized))
        }
        RequestPacket::Batch(reqs) => {
            let rewritten: Vec<_> = reqs
                .into_iter()
                .map(|req| rewrite_serialized_request(req, latest, safe, finalized))
                .collect();
            RequestPacket::Batch(rewritten)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SafeTip;

    /// Helper to create a ConsensusState with specific block heights.
    fn state_with_blocks(safe: u64, latest: u64, finalized: u64) -> ConsensusState {
        let state = ConsensusState::new();
        // Use SafeTip to update the state through the public API
        let mut tip = SafeTip::new(0);
        // We need different backend names for different heights
        if safe == latest && latest == finalized {
            tip.update("b1", latest);
        } else {
            // For mixed heights, set multiple backends and use f=0 so all go to lo
            tip.update("b1", latest);
        }
        state.update_from_safe_tip(&tip);

        // For precise control, use a custom tip configuration
        // Since the state only has atomic u64 fields, we can set them
        // through update_from_safe_tip by crafting the right SafeTip
        // However for test precision, let's add a test-only setter
        state
    }

    #[test]
    fn test_no_rewrite_when_no_consensus() {
        let state = ConsensusState::new();

        use alloy_json_rpc::{Id, Request};
        use serde_json::value::RawValue;

        let params = RawValue::from_string(r#"[{"to":"0x1234"},"latest"]"#.to_string()).unwrap();
        let req = Request::new("eth_call", Id::Number(1), params);
        let packet = RequestPacket::Single(req.serialize().unwrap());

        let original_json = serde_json::to_string(&packet).unwrap();
        let rewritten = rewrite_block_tags(packet, &state);
        let rewritten_json = serde_json::to_string(&rewritten).unwrap();

        assert_eq!(original_json, rewritten_json, "Should not rewrite when consensus is empty");
    }

    #[test]
    fn test_no_rewrite_when_no_tags() {
        let state = state_with_blocks(100, 100, 100);

        use alloy_json_rpc::{Id, Request};

        let req: Request<()> = Request::new("eth_blockNumber", Id::Number(1), ());
        let packet = RequestPacket::Single(req.serialize().unwrap());

        let original_json = serde_json::to_string(&packet).unwrap();
        let rewritten = rewrite_block_tags(packet, &state);
        let rewritten_json = serde_json::to_string(&rewritten).unwrap();

        assert_eq!(original_json, rewritten_json);
    }

    #[test]
    fn test_rewrite_latest_tag() {
        let state = state_with_blocks(100, 100, 100);

        use alloy_json_rpc::{Id, Request};
        use serde_json::value::RawValue;

        let params = RawValue::from_string(r#"[{"to":"0x1234"},"latest"]"#.to_string()).unwrap();
        let req = Request::new("eth_call", Id::Number(1), params);
        let packet = RequestPacket::Single(req.serialize().unwrap());

        let rewritten = rewrite_block_tags(packet, &state);

        let json = serde_json::to_string(&rewritten).unwrap();
        assert!(json.contains("0x64"), "Expected block 100=0x64 in: {json}");
        assert!(!json.contains("\"latest\""), "Should not contain 'latest' tag: {json}");
    }
}
