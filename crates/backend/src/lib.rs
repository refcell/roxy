#![doc = include_str!("../README.md")]
#![doc(issue_tracker_base_url = "https://github.com/refcell/roxy/issues/")]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]

use futures as _;
use pin_project_lite as _;
use tokio as _;
use tracing as _;

mod block_rewrite;
pub use block_rewrite::{BlockRewriteLayer, BlockRewriteService};

mod connection;
pub use connection::{ConnectionConfig, ConnectionState, ConnectionStateMachine};

mod consensus;
pub use consensus::{ConsensusPoller, ConsensusState};

mod group;
pub use group::{BackendGroup, BackendResponse, BoxedBackend};

mod health;
pub use health::{EmaHealthTracker, HealthConfig};

mod health_layer;
pub use health_layer::{HealthRecordingLayer, HealthRecordingService, SharedHealth};

mod http;
pub use http::{BackendConfig, HttpBackend};

mod load_balancer;
pub use load_balancer::{EmaLoadBalancer, RoundRobinBalancer};

mod retry;
pub use retry::RoxyRetryPolicy;

mod safe_tip;
pub use safe_tip::SafeTip;
