//! Placeholder TUN backend for platforms without a native utun/wintun driver.
//!
//! Apple mobile hosts (iOS/iPadOS) never create a system TUN: the packet path is
//! provided by the Network Extension packet flow and injected through the
//! platform packet I/O abstraction. Every constructor therefore fails loudly
//! instead of pretending a device exists.

use anyhow::{bail, Result};

use fuckipv6_common::VirtualIps;

/// Marker type that keeps `TunDevice` layout stable on unsupported targets.
#[derive(Clone, Debug, Default)]
pub struct UnsupportedTun;

impl UnsupportedTun {
    pub async fn create_dual_stack(
        _name: &str,
        _virtual_ips: &VirtualIps,
        _ipv4_prefix: u8,
        _ipv6_prefix: u8,
        _mtu: u16,
    ) -> Result<Self> {
        bail!("system TUN devices are not available on this platform")
    }

    pub fn name(&self) -> &str {
        "unsupported-tun"
    }

    pub async fn read(&mut self, _buf: &mut [u8]) -> Result<usize> {
        bail!("system TUN devices are not available on this platform")
    }

    pub async fn write(&mut self, _packet: &[u8]) -> Result<()> {
        bail!("system TUN devices are not available on this platform")
    }

    pub async fn delete(&mut self) -> Result<()> {
        Ok(())
    }
}
