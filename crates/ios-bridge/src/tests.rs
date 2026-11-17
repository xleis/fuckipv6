//! Bridge-level tests that do not require a live mesh.
//!
//! Packet-flow tests drive the real ExternalPacketDevice through the C ABI;
//! mesh-level interoperability stays in the embedded runtime suite.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::os::raw::c_char;
use std::ptr;

use fuckipv6_client::embedded::EmbeddedNetworkRequest;
use fuckipv6_client::packet_io::{ExternalPacketDevice, PacketDevice};
use fuckipv6_common::config::ClientConfig;
use fuckipv6_common::routing::{RoutingConfig, RoutingMode};
use fuckipv6_common::types::VirtualIps;

use crate::error::Fv6Status;
use crate::ffi::*;
use crate::platform::{EndpointHandle, HostCallbacks};
use crate::settings::TunnelSettings;

fn base_config() -> ClientConfig {
    serde_json::from_str(
        r#"{"coordinator_url":"ws://127.0.0.1:9000","network":{"virtual_network":"fd00::/64"}}"#,
    )
    .expect("minimal client config")
}

fn request(assigned: VirtualIps, transport: &str) -> EmbeddedNetworkRequest {
    let mut config = base_config();
    config.tun.mtu = 1280;
    EmbeddedNetworkRequest {
        network_name: "mesh".to_string(),
        assigned_ips: assigned,
        config,
        transport_addr: transport.parse::<SocketAddr>().unwrap(),
        dns_servers: vec![IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1))],
    }
}

fn dual_stack() -> VirtualIps {
    VirtualIps {
        ipv4: Some(Ipv4Addr::new(10, 8, 0, 2)),
        ipv6: Some("fd00::2".parse::<Ipv6Addr>().unwrap()),
    }
}

#[test]
fn settings_derive_dual_stack_routes() {
    let settings = TunnelSettings::derive(&request(dual_stack(), "127.0.0.1:4000")).unwrap();
    assert_eq!(settings.mtu, 1280);
    assert_eq!(settings.network, "mesh");
    assert_eq!(settings.ipv4.as_ref().unwrap().address, "10.8.0.2");
    assert_eq!(settings.ipv4.as_ref().unwrap().prefix, 24);
    assert_eq!(settings.ipv6.as_ref().unwrap().address, "fd00::2");
    assert_eq!(settings.ipv6.as_ref().unwrap().prefix, 64);
    assert!(settings.routes.iter().any(|route| route == "10.8.0.0/24"));
    assert!(settings.routes.iter().any(|route| route == "fd00::/64"));
    assert_eq!(settings.dns, vec!["1.1.1.1".to_string()]);
}

#[test]
fn settings_reject_non_mesh_routing() {
    let mut request = request(dual_stack(), "127.0.0.1:4000");
    request.config.routing = RoutingConfig {
        mode: RoutingMode::GlobalProxy,
        ..RoutingConfig::default()
    };
    let error = TunnelSettings::derive(&request).unwrap_err();
    assert!(error.message.contains("MeshOnly"), "{}", error.message);
}

#[test]
fn settings_reject_transport_inside_route() {
    let request = request(dual_stack(), "10.8.0.7:4000");
    let error = TunnelSettings::derive(&request).unwrap_err();
    assert!(error.message.contains("transport"), "{}", error.message);
}

#[test]
fn settings_reject_empty_addresses() {
    let request = request(
        VirtualIps {
            ipv4: None,
            ipv6: None,
        },
        "127.0.0.1:4000",
    );
    let error = TunnelSettings::derive(&request).unwrap_err();
    assert!(!error.message.is_empty());
}

#[test]
fn settings_reject_small_mtu() {
    let mut request = request(dual_stack(), "127.0.0.1:4000");
    request.config.tun.mtu = 576;
    assert!(TunnelSettings::derive(&request).is_err());
}

#[test]
fn ffi_null_handles_report_invalid_handle() {
    assert_eq!(
        unsafe { fv6_runtime_start(ptr::null_mut(), 1000) },
        Fv6Status::InvalidHandle.as_i32()
    );
    assert_eq!(
        unsafe { fv6_runtime_stop(ptr::null_mut(), 1000) },
        Fv6Status::InvalidHandle.as_i32()
    );
    assert_eq!(unsafe { fv6_runtime_phase(ptr::null()) }, -1);
    assert_eq!(
        unsafe { fv6_endpoint_send(ptr::null_mut(), ptr::null(), 0) },
        Fv6Status::InvalidHandle.as_i32()
    );
    // Releasing a null token is a no-op rather than a crash.
    unsafe { fv6_endpoint_release(ptr::null_mut()) };
    unsafe { fv6_runtime_destroy(ptr::null_mut()) };
}

#[test]
fn ffi_endpoint_rejects_short_buffer() {
    // A null endpoint is rejected before the buffer check.
    let mut len = 0usize;
    assert_eq!(
        unsafe { fv6_endpoint_recv(ptr::null_mut(), ptr::null_mut(), 0, &mut len, 0) },
        Fv6Status::InvalidHandle.as_i32()
    );
}

#[test]
fn ffi_create_rejects_relative_root() {
    let config = c_string(r#"{"storage_root":"relative/path"}"#);
    let mut runtime: *mut Fv6Runtime = ptr::null_mut();
    let mut error = vec![0u8; 256];
    let mut error_len = 0usize;
    let status = unsafe {
        fv6_runtime_create(
            config.as_ptr(),
            HostCallbacks::default(),
            ptr::null_mut(),
            &mut runtime,
            error.as_mut_ptr(),
            error.len(),
            &mut error_len,
        )
    };
    assert_eq!(status, Fv6Status::InvalidArgument.as_i32());
    assert!(runtime.is_null());
    let message = String::from_utf8_lossy(&error[..error_len]).into_owned();
    assert!(message.contains("absolute"), "{message}");
}

#[test]
fn ffi_create_rejects_invalid_json() {
    let config = c_string("{not json");
    let mut runtime: *mut Fv6Runtime = ptr::null_mut();
    let mut error = vec![0u8; 256];
    let mut error_len = 0usize;
    let status = unsafe {
        fv6_runtime_create(
            config.as_ptr(),
            HostCallbacks::default(),
            ptr::null_mut(),
            &mut runtime,
            error.as_mut_ptr(),
            error.len(),
            &mut error_len,
        )
    };
    assert_eq!(status, Fv6Status::InvalidArgument.as_i32());
    assert!(runtime.is_null());
}

#[test]
fn ffi_create_and_destroy_is_clean() {
    let dir = std::env::temp_dir().join(format!("fv6-bridge-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let config = c_string(&format!(r#"{{"storage_root":"{}"}}"#, dir.display()));
    let mut runtime: *mut Fv6Runtime = ptr::null_mut();
    let mut error = vec![0u8; 256];
    let mut error_len = 0usize;
    let status = unsafe {
        fv6_runtime_create(
            config.as_ptr(),
            HostCallbacks::default(),
            ptr::null_mut(),
            &mut runtime,
            error.as_mut_ptr(),
            error.len(),
            &mut error_len,
        )
    };
    assert_eq!(status, Fv6Status::Ok.as_i32());
    assert!(!runtime.is_null());
    assert_eq!(
        unsafe { fv6_runtime_phase(runtime) },
        FV6_PHASE_CREATED as i32
    );
    // Stopping a runtime that never started is idempotent.
    assert_eq!(
        unsafe { fv6_runtime_stop(runtime, 1000) },
        Fv6Status::Ok.as_i32()
    );
    assert_eq!(
        unsafe { fv6_runtime_stop(runtime, 1000) },
        Fv6Status::Ok.as_i32()
    );
    // Commands require a running node.
    assert_eq!(
        unsafe { fv6_runtime_network_path_changed(runtime, 1000) },
        Fv6Status::State.as_i32()
    );
    unsafe { fv6_runtime_destroy(runtime) };
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn ffi_start_snapshot_and_repeat_stop() {
    let dir = std::env::temp_dir().join(format!("fv6-bridge-run-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let runtime = create_runtime(&dir);

    assert_eq!(
        unsafe { fv6_runtime_start(runtime, 20_000) },
        Fv6Status::Ok.as_i32()
    );
    assert_eq!(
        unsafe { fv6_runtime_phase(runtime) },
        FV6_PHASE_RUNNING as i32
    );
    // Repeated start is a no-op while the node is running.
    assert_eq!(
        unsafe { fv6_runtime_start(runtime, 5_000) },
        Fv6Status::Ok.as_i32()
    );

    let snapshot = unsafe {
        read_buffer(|buffer, len, out_len| {
            fv6_runtime_snapshot(runtime, buffer, len, out_len, 10_000)
        })
    };
    assert!(snapshot.contains("state"), "{snapshot}");

    assert_eq!(
        unsafe { fv6_runtime_stop(runtime, 20_000) },
        Fv6Status::Ok.as_i32()
    );
    assert_eq!(
        unsafe { fv6_runtime_phase(runtime) },
        FV6_PHASE_STOPPED as i32
    );
    assert_eq!(
        unsafe { fv6_runtime_stop(runtime, 5_000) },
        Fv6Status::Ok.as_i32()
    );
    unsafe { fv6_runtime_destroy(runtime) };
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn ffi_second_runtime_on_same_root_reports_a_node_problem() {
    let dir = std::env::temp_dir().join(format!("fv6-bridge-lock-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let first = create_runtime(&dir);
    assert_eq!(
        unsafe { fv6_runtime_start(first, 20_000) },
        Fv6Status::Ok.as_i32()
    );

    let second = create_runtime(&dir);
    assert_eq!(
        unsafe { fv6_runtime_start(second, 5_000) },
        Fv6Status::NodeProblem.as_i32()
    );
    assert_eq!(
        unsafe { fv6_runtime_phase(second) },
        FV6_PHASE_FAILED as i32
    );
    let mut required = 0usize;
    assert_eq!(
        unsafe { fv6_runtime_last_error(second, ptr::null_mut(), 0, &mut required) },
        Fv6Status::BufferTooSmall.as_i32()
    );
    assert!(required > 0);
    let message = unsafe {
        read_buffer(|buffer, len, out_len| fv6_runtime_last_error(second, buffer, len, out_len))
    };
    assert!(!message.is_empty());

    assert_eq!(
        unsafe { fv6_runtime_stop(second, 5_000) },
        Fv6Status::Ok.as_i32()
    );
    unsafe { fv6_runtime_destroy(second) };
    assert_eq!(
        unsafe { fv6_runtime_stop(first, 20_000) },
        Fv6Status::Ok.as_i32()
    );
    unsafe { fv6_runtime_destroy(first) };
    let _ = std::fs::remove_dir_all(&dir);
}

const MTU: u16 = 1280;

#[test]
fn ffi_endpoint_moves_packets_through_the_real_packet_device() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("packet device runtime");
    let (mut device, endpoint) =
        ExternalPacketDevice::new("ios-ffi-test", MTU, 8).expect("packet device");
    let token = Box::into_raw(Box::new(EndpointHandle::new(
        endpoint,
        runtime.handle().clone(),
        MTU,
    )));

    let mut mtu = 0u16;
    assert_eq!(
        unsafe { fv6_endpoint_mtu(token, &mut mtu) },
        Fv6Status::Ok.as_i32()
    );
    assert_eq!(mtu, MTU);

    // Whole IP datagrams only: a bad version or an oversized packet never
    // reaches the device queue.
    let mut unknown = ipv6_packet(8);
    unknown[0] = 0x50;
    assert_eq!(
        unsafe { fv6_endpoint_send(token, unknown.as_ptr(), unknown.len()) },
        Fv6Status::InvalidPacket.as_i32()
    );
    let oversized = ipv6_packet(MTU as usize);
    assert_eq!(
        unsafe { fv6_endpoint_send(token, oversized.as_ptr(), oversized.len()) },
        Fv6Status::InvalidPacket.as_i32()
    );

    // Host -> node: one send becomes one datagram on the node side.
    let inbound = ipv6_packet(1200);
    assert_eq!(
        unsafe { fv6_endpoint_send(token, inbound.as_ptr(), inbound.len()) },
        Fv6Status::Ok.as_i32()
    );
    let mut buffer = vec![0u8; MTU as usize];
    let read = runtime
        .block_on(device.read(&mut buffer))
        .expect("node reads the host packet");
    assert_eq!(&buffer[..read], &inbound[..]);

    // Node -> host: whatever the data plane writes comes back verbatim.
    let outbound = ipv6_packet(64);
    runtime
        .block_on(device.write(&outbound))
        .expect("node writes a packet");
    let mut received = vec![0u8; MTU as usize];
    let mut length = 0usize;
    assert_eq!(
        unsafe {
            fv6_endpoint_recv(
                token,
                received.as_mut_ptr(),
                received.len(),
                &mut length,
                2_000,
            )
        },
        Fv6Status::Ok.as_i32()
    );
    assert_eq!(&received[..length], &outbound[..]);

    // An empty queue with a deadline reports a timeout instead of blocking.
    assert_eq!(
        unsafe { fv6_endpoint_recv(token, received.as_mut_ptr(), received.len(), &mut length, 0) },
        Fv6Status::Timeout.as_i32()
    );

    // A short buffer is rejected before any packet is consumed.
    let mut short = vec![0u8; 64];
    assert_eq!(
        unsafe { fv6_endpoint_recv(token, short.as_mut_ptr(), short.len(), &mut length, 0) },
        Fv6Status::BufferTooSmall.as_i32()
    );

    // Releasing the endpoint closes the device side and leaves no reader
    // waiting for a packet that can never arrive.
    unsafe { fv6_endpoint_release(token) };
    assert!(runtime.block_on(device.read(&mut buffer)).is_err());
    drop(runtime);
}

fn ipv6_packet(payload_len: usize) -> Vec<u8> {
    let mut packet = vec![0u8; 40 + payload_len];
    packet[0] = 0x60;
    packet[4..6].copy_from_slice(&(payload_len as u16).to_be_bytes());
    packet[6] = 59; // no next header
    packet[7] = 64; // hop limit
    packet[8..24].copy_from_slice(&"fd00::1".parse::<Ipv6Addr>().unwrap().octets());
    packet[24..40].copy_from_slice(&"fd00::2".parse::<Ipv6Addr>().unwrap().octets());
    packet
}

fn create_runtime(dir: &std::path::Path) -> *mut Fv6Runtime {
    let config = c_string(&format!(r#"{{"storage_root":"{}"}}"#, dir.display()));
    let mut runtime: *mut Fv6Runtime = ptr::null_mut();
    let mut error = vec![0u8; 512];
    let mut error_len = 0usize;
    let status = unsafe {
        fv6_runtime_create(
            config.as_ptr(),
            HostCallbacks::default(),
            ptr::null_mut(),
            &mut runtime,
            error.as_mut_ptr(),
            error.len(),
            &mut error_len,
        )
    };
    assert_eq!(
        status,
        Fv6Status::Ok.as_i32(),
        "{}",
        String::from_utf8_lossy(&error[..error_len])
    );
    runtime
}

/// Runs a two-pass read: the first call reveals the required length, the second
/// copies the payload.
unsafe fn read_buffer(call: impl Fn(*mut u8, usize, *mut usize) -> i32) -> String {
    let mut required = 0usize;
    let status = call(ptr::null_mut(), 0, &mut required);
    assert_eq!(status, Fv6Status::BufferTooSmall.as_i32());
    let mut buffer = vec![0u8; required];
    let mut written = 0usize;
    let status = call(buffer.as_mut_ptr(), buffer.len(), &mut written);
    assert_eq!(status, Fv6Status::Ok.as_i32());
    buffer.truncate(written);
    String::from_utf8_lossy(&buffer).into_owned()
}

fn c_string(value: &str) -> Vec<c_char> {
    let mut bytes: Vec<c_char> = value.bytes().map(|byte| byte as c_char).collect();
    bytes.push(0);
    bytes
}
