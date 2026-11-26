//! Wire types for the coordinator's network-configuration and blacklist endpoints.
//!
//! These used to live in `webui`, which mixed unmounted browser routes with the
//! DTOs the client surface reuses. Splitting them out lets the routes go away
//! without breaking the client (W0-03).

use fuckipv6_common::types::NetworkMode;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkConfigDto {
    pub mode: NetworkMode,
    pub ipv4_pool: String,
    pub ipv6_pool: String,
    pub ipv4_prefix: u8,
    pub ipv6_prefix: u8,
    pub config_file: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UpdateNetworkConfigRequest {
    pub mode: NetworkMode,
    pub ipv4_pool: String,
    pub ipv6_pool: String,
}

#[derive(Debug, Serialize)]
pub struct NetworkConfigApiResponse {
    pub success: bool,
    pub message: String,
    pub data: Option<NetworkConfigDto>,
}

#[derive(Debug, Deserialize)]
pub struct BlacklistActionRequest {
    pub client_id: String,
}

#[derive(Debug, Serialize)]
pub struct BlacklistActionResponse {
    pub success: bool,
    pub message: String,
    pub blacklist: Vec<String>,
}
