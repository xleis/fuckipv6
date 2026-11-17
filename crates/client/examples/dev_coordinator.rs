//! Development-only coordinator launcher for host-to-device interop tests.
//!
//! This binary is not part of the desktop product: the desktop app owns its
//! coordinator lifecycle through `symmetric::EmbeddedCoordinator`. The launcher
//! exists so an engineer can bring up a real signaling + relay coordinator on a
//! workstation and point a physical iOS device at it over the LAN.
//!
//! It reuses the product code path end to end: `ConfigManager` creates the
//! owner network record, and `EmbeddedCoordinator` starts the signaling and
//! Relay listeners with the same durable state the app would use.
//!
//! Usage:
//!   cargo run -p fuckipv6-client --example dev_coordinator -- \
//!     --external-host 192.168.1.20 --network mesh
//!
//! The network is created with `NetworkJoinMode::Open` so a device holding only
//! the coordinator URL can join without an invitation. Anything the launcher
//! writes stays inside `--config-dir` (default `.tmp/dev-coordinator`).

use std::net::IpAddr;
use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};
use fuckipv6_client::config_manager::{ConfigManager, NetworkCreationError};
use fuckipv6_client::symmetric::EmbeddedCoordinator;
use fuckipv6_common::domain::{NetworkCreateProfile, NetworkJoinMode, NetworkVisibility};

const DEFAULT_CONFIG_DIR: &str = ".tmp/dev-coordinator";
const DEFAULT_NETWORK: &str = "mesh";
const DEFAULT_SIGNALING_PORT: u16 = 6666;
const DEFAULT_RELAY_PORT: u16 = 6667;

struct Options {
    config_dir: PathBuf,
    network: String,
    signaling_port: u16,
    relay_port: u16,
    external_host: String,
}

impl Options {
    fn parse() -> Result<Self> {
        let mut options = Self {
            config_dir: PathBuf::from(DEFAULT_CONFIG_DIR),
            network: DEFAULT_NETWORK.to_string(),
            signaling_port: DEFAULT_SIGNALING_PORT,
            relay_port: DEFAULT_RELAY_PORT,
            external_host: String::new(),
        };
        let mut arguments = std::env::args().skip(1);
        while let Some(flag) = arguments.next() {
            let value = arguments
                .next()
                .ok_or_else(|| anyhow!("{flag} requires a value"))?;
            match flag.as_str() {
                "--config-dir" => options.config_dir = PathBuf::from(value),
                "--network" => options.network = value,
                "--signaling-port" => options.signaling_port = value.parse()?,
                "--relay-port" => options.relay_port = value.parse()?,
                "--external-host" => options.external_host = value,
                "--help" | "-h" => {
                    println!(
                        "usage: dev_coordinator --external-host <lan-ip> [--network {DEFAULT_NETWORK}] \
[--signaling-port {DEFAULT_SIGNALING_PORT}] [--relay-port {DEFAULT_RELAY_PORT}] \
[--config-dir {DEFAULT_CONFIG_DIR}]"
                    );
                    std::process::exit(0);
                }
                other => return Err(anyhow!("unknown argument: {other}")),
            }
        }
        if options.external_host.is_empty() {
            return Err(anyhow!(
                "--external-host is required; pass the LAN address devices reach"
            ));
        }
        options
            .external_host
            .parse::<IpAddr>()
            .context("--external-host must be an IP address")?;
        Ok(options)
    }

    fn coordinator_url(&self) -> String {
        format!("ws://{}:{}", self.external_host, self.signaling_port)
    }
}

fn profile(name: &str) -> NetworkCreateProfile {
    NetworkCreateProfile {
        name: name.to_string(),
        purpose: "Development coordinator for host-to-device interop testing".to_string(),
        icon: "home".to_string(),
        language: "zh-CN".to_string(),
        region: "CN".to_string(),
        tags: Vec::new(),
        capability_declarations: Vec::new(),
        visibility: NetworkVisibility::Unlisted,
        join_mode: NetworkJoinMode::Open,
        exit_contribution: Default::default(),
        exit_contribution_scope: Default::default(),
        exit_contribution_mode: Default::default(),
        exit_selection_policy: Default::default(),
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let options = Options::parse()?;
    let _ = fuckipv6_common::init_logging_with_level("info");
    std::fs::create_dir_all(&options.config_dir)
        .with_context(|| format!("cannot create {}", options.config_dir.display()))?;
    let manager = ConfigManager::new_for_dir(options.config_dir.clone())
        .context("cannot open the development config directory")?;
    let coordinator_url = options.coordinator_url();
    let network_id =
        match manager.create_owned_network(profile(&options.network), &coordinator_url, false) {
            Ok(created) => {
                println!(
                    "created network {} ({})",
                    options.network, created.record.network_id
                );
                Some(created.record.network_id)
            }
            Err(NetworkCreationError::DuplicateName(_)) => {
                println!(
                    "reusing network {} from {}",
                    options.network,
                    options.config_dir.display()
                );
                None
            }
            Err(error) => return Err(anyhow!("cannot create the development network: {error}")),
        };

    let coordinator = EmbeddedCoordinator::start_with_config_dir(
        &options.network,
        options.signaling_port,
        options.relay_port,
        None,
        Some(options.external_host.clone()),
        None,
        options.config_dir.clone(),
    )
    .await
    .context("cannot start the development coordinator")?;

    println!("signaling: {coordinator_url}");
    println!(
        "relay: {}",
        coordinator
            .relay_addr
            .map(|address| address.to_string())
            .unwrap_or_else(|| "disabled".to_string())
    );
    println!(
        "tunnel configuration for the device: {{\"networkName\":\"{}\",\"coordinatorURL\":\"{}\",\"dnsServers\":[]}}",
        options.network, coordinator_url
    );
    if network_id.is_none() {
        println!("note: join mode comes from the existing network record");
    }

    std::future::pending::<()>().await;
    Ok(())
}
