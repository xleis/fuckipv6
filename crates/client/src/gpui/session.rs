/// Native GPUI connects to the long-running control/node processes. Opening or
/// closing the desktop window must never own or tear down the data plane.
pub type NativeNodeSession = crate::remote_node::RemoteNodeSession;
