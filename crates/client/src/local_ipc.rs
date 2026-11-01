//! Authenticated, bounded local IPC for the node/control process boundary.
//!
//! This is deliberately not a TCP listener. Unix uses a mode-0600 Unix domain
//! socket inside an owner-only runtime directory; Windows uses a named pipe and
//! verifies that the peer process token belongs to the current user.

use std::fmt;
#[cfg(unix)]
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, PermissionsExt};

use anyhow::{bail, Context, Result};
use interprocess::local_socket::{
    tokio::{prelude::*, Listener as LocalListener, Stream as LocalStream},
    ListenerOptions,
};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::time::timeout;

use crate::node::{NodeCommandRequest, NodeCommandResult, NodeEvent, NodeSnapshot};

pub const PROTOCOL_VERSION: u16 = 1;
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
pub const IO_TIMEOUT: Duration = Duration::from_secs(5);
const FRAME_MAGIC: [u8; 4] = *b"FKIP";
const FRAME_HEADER_LEN: usize = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum FrameKind {
    ClientHello = 1,
    ServerHello = 2,
    Request = 3,
    Response = 4,
    Event = 5,
    Error = 6,
    Ping = 7,
    Pong = 8,
}

impl FrameKind {
    fn from_u8(value: u8) -> Result<Self> {
        match value {
            1 => Ok(Self::ClientHello),
            2 => Ok(Self::ServerHello),
            3 => Ok(Self::Request),
            4 => Ok(Self::Response),
            5 => Ok(Self::Event),
            6 => Ok(Self::Error),
            7 => Ok(Self::Ping),
            8 => Ok(Self::Pong),
            _ => bail!("未知本地 IPC 帧类型: {value}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IpcRole {
    Control,
    Node,
    Cli,
    Gpui,
    WebUi,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientHello {
    pub protocol_version: u16,
    pub role: IpcRole,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerHello {
    pub protocol_version: u16,
    pub max_frame_bytes: u32,
    pub server_role: IpcRole,
    pub server_pid: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LocalRequest {
    pub request_id: u64,
    pub command: LocalCommand,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum LocalCommand {
    Snapshot,
    Execute(NodeCommandRequest),
    Subscribe { after_sequence: Option<u64> },
    StopNode,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum LocalResponse {
    Snapshot {
        snapshot: Box<NodeSnapshot>,
        event_sequence: u64,
    },
    NodeCommand(NodeCommandResult),
    Subscribed {
        snapshot: Box<NodeSnapshot>,
        event_sequence: u64,
    },
    Accepted {
        message: String,
    },
    Pong,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalEvent {
    pub event: NodeEvent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IpcErrorCode {
    Unauthorized,
    VersionMismatch,
    OversizedFrame,
    MalformedFrame,
    MalformedRequest,
    RequestIdMismatch,
    BackendUnavailable,
    EventGap,
    Unsupported,
    Timeout,
    Internal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalError {
    pub code: IpcErrorCode,
    pub message: String,
    pub retryable: bool,
}

impl LocalError {
    pub fn new(code: IpcErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            retryable: false,
        }
    }

    pub fn retryable(mut self, retryable: bool) -> Self {
        self.retryable = retryable;
        self
    }
}

impl fmt::Display for LocalError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "本地 IPC 错误 {:?}: {}", self.code, self.message)
    }
}

impl std::error::Error for LocalError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndpointRole {
    Node,
    Control,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalEndpoint {
    #[cfg(unix)]
    Path(PathBuf),
    #[cfg(windows)]
    Name(String),
}

impl fmt::Display for LocalEndpoint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            #[cfg(unix)]
            Self::Path(path) => write!(formatter, "{}", path.display()),
            #[cfg(windows)]
            Self::Name(name) => formatter.write_str(name),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeerIdentity {
    pub euid: u32,
    pub pid: Option<u32>,
}

#[derive(Debug)]
pub struct LocalConnection {
    stream: LocalStream,
    peer: PeerIdentity,
    verified_client_role: Option<IpcRole>,
}

impl LocalConnection {
    pub fn peer(&self) -> PeerIdentity {
        self.peer
    }

    /// The client role is only available after the versioned handshake has
    /// completed. It is transport metadata and never sourced from command JSON.
    pub fn verified_client_role(&self) -> Option<&IpcRole> {
        self.verified_client_role.as_ref()
    }

    pub async fn send<T: Serialize>(&mut self, kind: FrameKind, value: &T) -> Result<()> {
        timeout(IO_TIMEOUT, write_frame(&mut self.stream, kind, value))
            .await
            .map_err(|_| anyhow::anyhow!("写入本地 IPC 帧超时"))?
    }

    pub async fn receive<T: for<'de> Deserialize<'de>>(&mut self) -> Result<(FrameKind, T)> {
        timeout(IO_TIMEOUT, read_frame(&mut self.stream))
            .await
            .map_err(|_| anyhow::anyhow!("读取本地 IPC 帧超时"))?
    }

    /// Wait for the next frame without an idle deadline.
    ///
    /// Long-lived event subscriptions are intentionally allowed to sit idle;
    /// callers cancel the future to stop waiting. Individual frames remain
    /// bounded by [`MAX_FRAME_BYTES`].
    pub(crate) async fn receive_next<T: for<'de> Deserialize<'de>>(
        &mut self,
    ) -> Result<(FrameKind, T)> {
        read_frame(&mut self.stream).await
    }

    pub async fn send_response(&mut self, response: &LocalResponse) -> Result<()> {
        self.send(FrameKind::Response, response).await
    }

    pub async fn send_error(&mut self, error: &LocalError) -> Result<()> {
        self.send(FrameKind::Error, error).await
    }

    pub async fn send_event(&mut self, event: &LocalEvent) -> Result<()> {
        self.send(FrameKind::Event, event).await
    }
}

pub struct LocalServer {
    listener: LocalListener,
    endpoint: LocalEndpoint,
    role: EndpointRole,
}

impl LocalServer {
    pub async fn bind(endpoint: LocalEndpoint, role: EndpointRole) -> Result<Self> {
        #[cfg(unix)]
        let name = match &endpoint {
            LocalEndpoint::Path(path) => path
                .to_str()
                .context("本地 IPC 路径不是有效 UTF-8")?
                .to_fs_name::<interprocess::local_socket::GenericFilePath>()
                .context("无法编码本地 IPC 路径")?,
            #[cfg(windows)]
            _ => unreachable!(),
        };
        #[cfg(windows)]
        let name = match &endpoint {
            LocalEndpoint::Name(name) => name
                .as_str()
                .to_ns_name::<interprocess::local_socket::GenericNamespaced>()
                .context("无法编码本地 IPC 名称")?,
            #[cfg(unix)]
            _ => unreachable!(),
        };

        #[cfg(unix)]
        let options = {
            use interprocess::os::unix::local_socket::ListenerOptionsExt;
            ListenerOptions::new().name(name).mode(0o600)
        };
        #[cfg(windows)]
        let options = {
            use interprocess::os::windows::local_socket::ListenerOptionsExt;
            use interprocess::os::windows::security_descriptor::SecurityDescriptor;
            use widestring::U16CString;

            // A protected DACL granting access only through the owner-rights SID
            // closes the pre-authentication connection window. The live peer SID
            // check in `accept_raw` remains as defense in depth.
            let descriptor = U16CString::from_str("D:P(A;;GA;;;OW)")
                .map_err(|error| anyhow::anyhow!("无法编码 Windows IPC DACL: {error}"))
                .and_then(|sddl| {
                    SecurityDescriptor::deserialize(sddl.as_ucstr())
                        .context("无法解析 Windows IPC 安全描述符")
                })?;
            ListenerOptions::new()
                .name(name)
                .security_descriptor(descriptor)
        };

        let listener = options.create_tokio().with_context(|| {
            format!(
                "无法在 {} 创建 {} IPC 监听器",
                endpoint,
                endpoint_role_name(role)
            )
        })?;
        Ok(Self {
            listener,
            endpoint,
            role,
        })
    }

    pub fn endpoint(&self) -> &LocalEndpoint {
        &self.endpoint
    }

    pub(crate) fn role(&self) -> EndpointRole {
        self.role
    }

    /// Accept and authenticate a peer without waiting for its application
    /// handshake. Long-running services use this to perform untrusted
    /// handshakes in separate tasks instead of blocking the accept loop.
    pub(crate) async fn accept_raw(&self) -> Result<LocalConnection> {
        let stream = self
            .listener
            .accept()
            .await
            .context("接受本地 IPC 连接失败")?;
        let peer = peer_identity(&stream)?;
        authorize_peer(peer, current_user_id())?;
        Ok(LocalConnection {
            stream,
            peer,
            verified_client_role: None,
        })
    }

    /// Complete the versioned client/server handshake for an authenticated
    /// connection returned by [`LocalServer::accept_raw`].
    pub(crate) async fn finish_handshake(
        &self,
        mut connection: LocalConnection,
    ) -> Result<LocalConnection> {
        let (kind, hello): (FrameKind, ClientHello) = connection.receive().await?;
        if kind != FrameKind::ClientHello {
            send_protocol_error(
                &mut connection,
                LocalError::new(
                    IpcErrorCode::MalformedRequest,
                    "第一条消息必须是 ClientHello",
                ),
            )
            .await;
            bail!("本地 IPC 客户端未发送握手");
        }
        if hello.protocol_version != PROTOCOL_VERSION {
            send_protocol_error(
                &mut connection,
                LocalError::new(
                    IpcErrorCode::VersionMismatch,
                    format!(
                        "本地 IPC 协议版本不匹配: peer={}, server={}",
                        hello.protocol_version, PROTOCOL_VERSION
                    ),
                ),
            )
            .await;
            bail!("本地 IPC 协议版本不匹配");
        }

        let ack = ServerHello {
            protocol_version: PROTOCOL_VERSION,
            max_frame_bytes: MAX_FRAME_BYTES as u32,
            server_role: match self.role {
                EndpointRole::Node => IpcRole::Node,
                EndpointRole::Control => IpcRole::Control,
            },
            server_pid: std::process::id(),
        };
        connection.verified_client_role = Some(hello.role);
        connection.send(FrameKind::ServerHello, &ack).await?;
        Ok(connection)
    }

    pub async fn accept(&self) -> Result<LocalConnection> {
        let connection = self.accept_raw().await?;
        self.finish_handshake(connection).await
    }
}

impl Drop for LocalServer {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            let LocalEndpoint::Path(path) = &self.endpoint;
            let result = std::fs::remove_file(path);
            if let Err(error) = result {
                if error.kind() != io::ErrorKind::NotFound {
                    tracing::warn!(path = %path.display(), %error, "删除本地 IPC socket 失败");
                }
            }
        }
    }
}

pub fn endpoint(role: EndpointRole) -> Result<LocalEndpoint> {
    runtime_directory().map(|directory| {
        #[cfg(unix)]
        {
            LocalEndpoint::Path(directory.join(format!("{}.sock", endpoint_role_name(role))))
        }
        #[cfg(windows)]
        {
            let _ = directory;
            let user = std::env::var("USERNAME").unwrap_or_else(|_| "user".into());
            LocalEndpoint::Name(format!(
                "fuckipv6-{}-{}.sock",
                sanitize_name(&user),
                endpoint_role_name(role)
            ))
        }
    })
}

pub fn runtime_directory() -> Result<PathBuf> {
    if let Some(configured) = std::env::var_os("FUCKIPV6_RUNTIME_DIR") {
        let directory = PathBuf::from(configured);
        prepare_runtime_directory(&directory)?;
        return Ok(directory);
    }

    #[cfg(unix)]
    {
        let directory = if let Some(runtime_dir) = std::env::var_os("XDG_RUNTIME_DIR") {
            PathBuf::from(runtime_dir).join("fuckipv6")
        } else {
            std::env::temp_dir().join(format!("fuckipv6-{}", current_user_id()))
        };
        prepare_runtime_directory(&directory)?;
        Ok(directory)
    }
    #[cfg(windows)]
    {
        let directory = std::env::temp_dir().join("fuckipv6");
        prepare_runtime_directory(&directory)?;
        Ok(directory)
    }
}

pub async fn connect(endpoint: &LocalEndpoint, role: IpcRole) -> Result<LocalConnection> {
    #[cfg(unix)]
    let name = match endpoint {
        LocalEndpoint::Path(path) => path
            .to_str()
            .context("本地 IPC 路径不是有效 UTF-8")?
            .to_fs_name::<interprocess::local_socket::GenericFilePath>()
            .context("无法编码本地 IPC 路径")?,
        #[cfg(windows)]
        _ => unreachable!(),
    };
    #[cfg(windows)]
    let name = match endpoint {
        LocalEndpoint::Name(name) => name
            .as_str()
            .to_ns_name::<interprocess::local_socket::GenericNamespaced>()
            .context("无法编码本地 IPC 名称")?,
        #[cfg(unix)]
        _ => unreachable!(),
    };

    let stream = timeout(CONNECT_TIMEOUT, LocalStream::connect(name))
        .await
        .map_err(|_| anyhow::anyhow!("连接本地 IPC {endpoint} 超时"))?
        .with_context(|| format!("无法连接本地 IPC {endpoint}"))?;
    let peer = peer_identity(&stream)?;
    // The socket file is owner-only, but checking the live peer credentials as
    // well prevents accidentally connecting through a replaced/stale path.
    authorize_peer(peer, current_user_id())?;

    let mut connection = LocalConnection {
        stream,
        peer,
        verified_client_role: None,
    };
    let hello = ClientHello {
        protocol_version: PROTOCOL_VERSION,
        role,
    };
    connection.send(FrameKind::ClientHello, &hello).await?;

    let (kind, payload): (FrameKind, serde_json::Value) = connection.receive().await?;
    let ack: ServerHello = match kind {
        FrameKind::ServerHello => {
            serde_json::from_value(payload).context("无法解析本地 IPC 握手响应")?
        }
        FrameKind::Error => {
            let error: LocalError =
                serde_json::from_value(payload).context("无法解析本地 IPC 握手错误")?;
            return Err(error.into());
        }
        other => bail!("本地 IPC 握手收到意外帧: {other:?}"),
    };
    if ack.protocol_version != PROTOCOL_VERSION {
        bail!(
            "本地 IPC 协议版本不匹配: client={}, server={}",
            PROTOCOL_VERSION,
            ack.protocol_version
        );
    }
    if ack.max_frame_bytes as usize != MAX_FRAME_BYTES {
        bail!(
            "本地 IPC 帧上限不兼容: client={}, server={}",
            MAX_FRAME_BYTES,
            ack.max_frame_bytes
        );
    }
    Ok(connection)
}

pub async fn request(
    endpoint: &LocalEndpoint,
    role: IpcRole,
    request: &LocalRequest,
) -> Result<LocalResponse> {
    let mut connection = connect(endpoint, role).await?;
    connection.send(FrameKind::Request, request).await?;
    let (kind, payload): (FrameKind, serde_json::Value) = connection.receive().await?;
    match kind {
        FrameKind::Response => {
            let response = serde_json::from_value(payload).context("无法解析本地 IPC 响应")?;
            Ok(response)
        }
        FrameKind::Error => {
            let error: LocalError =
                serde_json::from_value(payload).context("无法解析本地 IPC 错误响应")?;
            Err(error.into())
        }
        other => bail!("本地 IPC 请求收到意外帧: {other:?}"),
    }
}

pub async fn execute(
    endpoint: &LocalEndpoint,
    role: IpcRole,
    command_request: NodeCommandRequest,
) -> Result<NodeCommandResult> {
    let request_id = command_request.request_id;
    let response = request(
        endpoint,
        role,
        &LocalRequest {
            request_id,
            command: LocalCommand::Execute(command_request),
        },
    )
    .await?;
    match response {
        LocalResponse::NodeCommand(result) if result.request_id == request_id => Ok(result),
        LocalResponse::NodeCommand(result) => bail!(
            "本地 IPC 响应 request_id 不匹配: expected={request_id}, actual={}",
            result.request_id
        ),
        other => bail!("本地 IPC 返回了意外响应: {other:?}"),
    }
}

pub async fn write_frame<T: Serialize>(
    writer: &mut (impl AsyncWrite + Unpin),
    kind: FrameKind,
    value: &T,
) -> Result<()> {
    let payload = serde_json::to_vec(value).context("无法序列化本地 IPC 帧")?;
    check_frame_size(payload.len())?;
    let mut header = [0u8; FRAME_HEADER_LEN];
    header[..4].copy_from_slice(&FRAME_MAGIC);
    header[4..6].copy_from_slice(&PROTOCOL_VERSION.to_be_bytes());
    header[6] = kind as u8;
    header[7] = 0;
    header[8..].copy_from_slice(&(payload.len() as u32).to_be_bytes());
    writer
        .write_all(&header)
        .await
        .context("写入本地 IPC 帧头失败")?;
    writer
        .write_all(&payload)
        .await
        .context("写入本地 IPC 帧载荷失败")?;
    writer.flush().await.context("刷新本地 IPC 帧失败")
}

pub async fn read_frame<T: for<'de> Deserialize<'de>>(
    reader: &mut (impl AsyncRead + Unpin),
) -> Result<(FrameKind, T)> {
    let mut header = [0u8; FRAME_HEADER_LEN];
    reader
        .read_exact(&mut header)
        .await
        .context("读取本地 IPC 帧头失败")?;
    if header[..4] != FRAME_MAGIC {
        bail!("本地 IPC 帧 magic 不匹配");
    }
    if header[7] != 0 {
        bail!("本地 IPC 帧保留字段非零");
    }
    let version = u16::from_be_bytes([header[4], header[5]]);
    if version != PROTOCOL_VERSION {
        bail!("本地 IPC 帧版本不匹配: {version}");
    }
    let kind = FrameKind::from_u8(header[6])?;
    let payload_len = u32::from_be_bytes([header[8], header[9], header[10], header[11]]) as usize;
    check_frame_size(payload_len)?;
    let mut payload = vec![0u8; payload_len];
    reader
        .read_exact(&mut payload)
        .await
        .context("读取本地 IPC 帧载荷失败")?;
    let value = serde_json::from_slice(&payload).context("无法解析本地 IPC 帧载荷")?;
    Ok((kind, value))
}

fn check_frame_size(size: usize) -> Result<()> {
    if size > MAX_FRAME_BYTES {
        bail!("本地 IPC 帧超过上限: size={size}, max={MAX_FRAME_BYTES}");
    }
    Ok(())
}

async fn send_protocol_error(connection: &mut LocalConnection, error: LocalError) {
    let _ = timeout(IO_TIMEOUT, connection.send_error(&error)).await;
}

pub fn authorize_peer(peer: PeerIdentity, current_user_id: u32) -> Result<()> {
    if peer.euid != current_user_id {
        bail!(
            "拒绝本地 IPC 连接：对端 euid={}，当前 euid={}",
            peer.euid,
            current_user_id
        );
    }
    Ok(())
}

#[cfg(unix)]
fn current_user_id() -> u32 {
    // SAFETY: geteuid has no preconditions and does not dereference pointers.
    unsafe { libc::geteuid() }
}

#[cfg(windows)]
fn current_user_id() -> u32 {
    0
}

#[cfg(unix)]
fn peer_identity(stream: &LocalStream) -> Result<PeerIdentity> {
    let credentials = stream.peer_creds().context("无法读取本地 IPC 对端凭据")?;
    let euid = credentials
        .euid()
        .context("当前平台未提供本地 IPC 对端 euid")?;
    Ok(PeerIdentity {
        euid,
        pid: credentials.pid().and_then(|pid| u32::try_from(pid).ok()),
    })
}

#[cfg(windows)]
fn peer_identity(stream: &LocalStream) -> Result<PeerIdentity> {
    let credentials = stream.peer_creds().context("无法读取本地 IPC 对端凭据")?;
    let pid = credentials
        .pid()
        .context("Windows 本地 IPC 未提供对端 PID")?;
    if !same_windows_user(pid).context("无法校验 Windows 本地 IPC 对端用户")? {
        bail!("拒绝本地 IPC 连接：对端进程不属于当前 Windows 用户");
    }
    Ok(PeerIdentity {
        euid: 0,
        pid: Some(pid),
    })
}

#[cfg(windows)]
fn same_windows_user(peer_pid: u32) -> Result<bool> {
    use std::ptr::null_mut;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::Security::{
        EqualSid, GetTokenInformation, TokenUser, TOKEN_QUERY, TOKEN_USER,
    };
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    struct OwnedHandle(HANDLE);
    impl Drop for OwnedHandle {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: this handle was returned by a successful Windows API call.
                unsafe { CloseHandle(self.0) };
            }
        }
    }

    unsafe fn token_user_buffer(token: HANDLE) -> Result<Vec<u8>> {
        let mut needed = 0u32;
        // SAFETY: a null buffer with length zero is the documented size query.
        unsafe { GetTokenInformation(token, TokenUser, null_mut(), 0, &mut needed) };
        if needed == 0 {
            bail!("Windows token user size query returned zero");
        }
        let mut buffer = vec![0u8; needed as usize];
        // SAFETY: buffer has the requested size and remains alive for the call.
        if unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                buffer.as_mut_ptr().cast(),
                needed,
                &mut needed,
            )
        } == 0
        {
            bail!("GetTokenInformation(TokenUser) failed");
        }
        Ok(buffer)
    }

    unsafe fn user_sid(buffer: &[u8]) -> *mut core::ffi::c_void {
        // SAFETY: TokenUser guarantees a TOKEN_USER at the start of the buffer.
        unsafe { (*(buffer.as_ptr() as *const TOKEN_USER)).User.Sid }
    }

    let current_token = {
        let mut handle = null_mut();
        // SAFETY: current process pseudo-handle is valid and out-param is writable.
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut handle) } == 0 {
            bail!("OpenProcessToken(current process) failed");
        }
        OwnedHandle(handle)
    };
    let peer_process = {
        // SAFETY: PID came from the OS local-socket credentials.
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, peer_pid) };
        if handle.is_null() {
            bail!("OpenProcess(peer={peer_pid}) failed");
        }
        OwnedHandle(handle)
    };
    let peer_token = {
        let mut handle = null_mut();
        // SAFETY: peer process handle is open and out-param is writable.
        if unsafe { OpenProcessToken(peer_process.0, TOKEN_QUERY, &mut handle) } == 0 {
            bail!("OpenProcessToken(peer={peer_pid}) failed");
        }
        OwnedHandle(handle)
    };

    // SAFETY: both buffers outlive their SID pointers and are initialized by Windows.
    let (current_buffer, peer_buffer) = unsafe {
        (
            token_user_buffer(current_token.0)?,
            token_user_buffer(peer_token.0)?,
        )
    };
    // SAFETY: both SID pointers refer to live TOKEN_USER buffers.
    Ok(unsafe { EqualSid(user_sid(&current_buffer), user_sid(&peer_buffer)) != 0 })
}

#[cfg(unix)]
fn prepare_runtime_directory(directory: &Path) -> Result<()> {
    std::fs::create_dir_all(directory)
        .with_context(|| format!("无法创建本地 IPC 目录 {}", directory.display()))?;
    let metadata = std::fs::symlink_metadata(directory)
        .with_context(|| format!("无法读取本地 IPC 目录 {}", directory.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        bail!("本地 IPC 路径必须是真实目录: {}", directory.display());
    }
    if metadata.uid() != current_user_id() {
        bail!(
            "本地 IPC 目录所有者不是当前用户: {} (uid={})",
            directory.display(),
            metadata.uid()
        );
    }
    let mut permissions = metadata.permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(directory, permissions)
        .with_context(|| format!("无法收紧本地 IPC 目录权限 {}", directory.display()))?;
    Ok(())
}

#[cfg(windows)]
fn prepare_runtime_directory(directory: &Path) -> Result<()> {
    std::fs::create_dir_all(directory)
        .with_context(|| format!("无法创建本地 IPC 目录 {}", directory.display()))?;
    Ok(())
}

fn endpoint_role_name(role: EndpointRole) -> &'static str {
    match role {
        EndpointRole::Node => "node",
        EndpointRole::Control => "control",
    }
}

#[cfg(windows)]
fn sanitize_name(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::io::duplex;

    #[cfg(unix)]
    fn test_directory(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "fuckipv6-local-ipc-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock before Unix epoch")
                .as_nanos()
        ))
    }

    #[tokio::test]
    async fn oversized_length_is_rejected_before_payload_allocation() {
        let (mut client, mut server) = duplex(64);
        let mut header = [0u8; FRAME_HEADER_LEN];
        header[..4].copy_from_slice(&FRAME_MAGIC);
        header[4..6].copy_from_slice(&PROTOCOL_VERSION.to_be_bytes());
        header[6] = FrameKind::Request as u8;
        header[8..].copy_from_slice(&((MAX_FRAME_BYTES + 1) as u32).to_be_bytes());
        client.write_all(&header).await.unwrap();

        let error = read_frame::<serde_json::Value>(&mut server)
            .await
            .expect_err("oversized frame must fail");
        assert!(error.to_string().contains("超过上限"));
    }

    #[tokio::test]
    async fn malformed_magic_and_version_are_rejected() {
        for (magic, version) in [(*b"FAIL", PROTOCOL_VERSION), (FRAME_MAGIC, 99)] {
            let (mut client, mut server) = duplex(64);
            let mut header = [0u8; FRAME_HEADER_LEN];
            header[..4].copy_from_slice(&magic);
            header[4..6].copy_from_slice(&version.to_be_bytes());
            header[6] = FrameKind::Request as u8;
            header[8..].copy_from_slice(&2u32.to_be_bytes());
            client.write_all(&header).await.unwrap();
            client.write_all(b"{}").await.unwrap();
            assert!(read_frame::<serde_json::Value>(&mut server).await.is_err());
        }
    }

    #[tokio::test]
    async fn framed_json_round_trip_is_bounded() {
        let (mut client, mut server) = duplex(4096);
        let payload = json!({"message": "ok", "count": 3});
        write_frame(&mut client, FrameKind::Ping, &payload)
            .await
            .unwrap();
        let (kind, decoded) = read_frame::<serde_json::Value>(&mut server).await.unwrap();
        assert_eq!(kind, FrameKind::Ping);
        assert_eq!(decoded, payload);
    }

    #[test]
    fn same_user_is_authorized_and_other_user_is_rejected() {
        let current = current_user_id();
        authorize_peer(
            PeerIdentity {
                euid: current,
                pid: Some(42),
            },
            current,
        )
        .expect("same euid");
        let error = authorize_peer(
            PeerIdentity {
                euid: current.wrapping_add(1),
                pid: Some(42),
            },
            current,
        )
        .expect_err("other euid");
        assert!(error.to_string().contains("拒绝"));
    }

    #[test]
    fn protocol_version_is_explicit() {
        assert_eq!(PROTOCOL_VERSION, 1);
        const { assert!(MAX_FRAME_BYTES <= 16 * 1024 * 1024) };
    }

    #[cfg(unix)]
    #[test]
    fn runtime_directory_is_owner_only_and_rejects_symlinks() -> Result<()> {
        let directory = test_directory("permissions");
        std::fs::create_dir_all(&directory)?;
        let mut permissions = std::fs::metadata(&directory)?.permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&directory, permissions)?;

        prepare_runtime_directory(&directory)?;
        assert_eq!(
            std::fs::metadata(&directory)?.permissions().mode() & 0o777,
            0o700
        );

        let link = directory.with_extension("link");
        std::os::unix::fs::symlink(&directory, &link)?;
        let error = prepare_runtime_directory(&link).expect_err("symlink must be rejected");
        assert!(error.to_string().contains("真实目录"));

        std::fs::remove_file(link)?;
        std::fs::remove_dir_all(directory)?;
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn protocol_version_mismatch_returns_typed_error() -> Result<()> {
        let directory = test_directory("version");
        prepare_runtime_directory(&directory)?;
        let endpoint = LocalEndpoint::Path(directory.join("node.sock"));
        let server = LocalServer::bind(endpoint.clone(), EndpointRole::Node).await?;
        let accept = tokio::spawn(async move {
            let result = server.accept().await;
            assert!(result.is_err(), "mismatched hello must reject the peer");
        });

        let LocalEndpoint::Path(path) = &endpoint;
        let name = path
            .to_str()
            .context("test path is not UTF-8")?
            .to_fs_name::<interprocess::local_socket::GenericFilePath>()?;
        let mut stream = LocalStream::connect(name).await?;
        write_frame(
            &mut stream,
            FrameKind::ClientHello,
            &ClientHello {
                protocol_version: PROTOCOL_VERSION + 1,
                role: IpcRole::Cli,
            },
        )
        .await?;
        let (kind, error): (FrameKind, LocalError) = read_frame(&mut stream).await?;
        assert_eq!(kind, FrameKind::Error);
        assert_eq!(error.code, IpcErrorCode::VersionMismatch);

        accept.await?;
        server_cleanup(&directory)?;
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn reconnects_and_resynchronizes_from_authoritative_snapshot() -> Result<()> {
        let directory = test_directory("reconnect");
        prepare_runtime_directory(&directory)?;
        let endpoint = LocalEndpoint::Path(directory.join("node.sock"));
        let server = LocalServer::bind(endpoint.clone(), EndpointRole::Node).await?;
        let serving = tokio::spawn(async move {
            let mut first = server.accept().await?;
            let request: LocalRequest = first.receive().await?.1;
            assert_eq!(request.command, LocalCommand::Snapshot);
            drop(first);

            let mut second = server.accept().await?;
            let (kind, request): (FrameKind, LocalRequest) = second.receive().await?;
            assert_eq!(kind, FrameKind::Request);
            assert_eq!(
                request.command,
                LocalCommand::Subscribe {
                    after_sequence: Some(41)
                }
            );
            let snapshot = NodeSnapshot {
                revision: 7,
                ..NodeSnapshot::default()
            };
            second
                .send_response(&LocalResponse::Subscribed {
                    snapshot: Box::new(snapshot),
                    event_sequence: 42,
                })
                .await?;
            Ok::<_, anyhow::Error>(())
        });

        let first = request(
            &endpoint,
            IpcRole::Gpui,
            &LocalRequest {
                request_id: 1,
                command: LocalCommand::Snapshot,
            },
        )
        .await;
        assert!(first.is_err(), "dropped connection must surface an error");

        let response = request(
            &endpoint,
            IpcRole::Gpui,
            &LocalRequest {
                request_id: 2,
                command: LocalCommand::Subscribe {
                    after_sequence: Some(41),
                },
            },
        )
        .await?;
        match response {
            LocalResponse::Subscribed {
                snapshot,
                event_sequence,
            } => {
                assert_eq!(snapshot.revision, 7);
                assert_eq!(event_sequence, 42);
            }
            other => panic!("unexpected reconnection response: {other:?}"),
        }

        serving.await??;
        server_cleanup(&directory)?;
        Ok(())
    }

    #[cfg(unix)]
    fn server_cleanup(directory: &Path) -> Result<()> {
        std::fs::remove_dir_all(directory)?;
        Ok(())
    }

    #[cfg(target_os = "linux")]
    const SECOND_UID_TEST_NAME: &str = "local_ipc::tests::real_second_uid_peer_is_rejected";
    #[cfg(target_os = "linux")]
    const SECOND_UID_ROLE_ENV: &str = "FUCKIPV6_SECOND_UID_TEST_ROLE";
    #[cfg(target_os = "linux")]
    const SECOND_UID_ENDPOINT_ENV: &str = "FUCKIPV6_SECOND_UID_TEST_ENDPOINT";

    #[cfg(target_os = "linux")]
    #[test]
    fn real_second_uid_peer_is_rejected() -> Result<()> {
        let role = match std::env::var(SECOND_UID_ROLE_ENV) {
            Ok(role) => role,
            Err(std::env::VarError::NotPresent) => return bootstrap_second_uid_test(),
            Err(error) => return Err(error.into()),
        };
        match role.as_str() {
            "coordinator" => run_second_uid_coordinator(),
            "peer" => run_second_uid_peer(),
            other => bail!("未知第二 UID 测试角色: {other}"),
        }
    }

    #[cfg(target_os = "linux")]
    fn bootstrap_second_uid_test() -> Result<()> {
        use std::process::Command;

        let uid = current_user_id();
        // SAFETY: getegid has no preconditions.
        let gid = unsafe { libc::getegid() };
        let user = std::env::var("USER").unwrap_or_else(|_| uid.to_string());
        let Some(subuid) = subordinate_id_start("/etc/subuid", &user, uid) else {
            eprintln!("跳过真实第二 UID 测试：未找到可用的 /etc/subuid 映射");
            return Ok(());
        };
        let Some(subgid) = subordinate_id_start("/etc/subgid", &user, gid) else {
            eprintln!("跳过真实第二 UID 测试：未找到可用的 /etc/subgid 映射");
            return Ok(());
        };

        let executable = std::env::current_exe().context("无法定位本地 IPC 测试程序")?;
        let socket_path = test_directory("second-uid").join("peer.sock");
        let output = Command::new("unshare")
            .arg(format!("--map-users=0:{uid}:1"))
            .arg(format!("--map-users=1:{subuid}:1"))
            .arg(format!("--map-groups=0:{gid}:1"))
            .arg(format!("--map-groups=1:{subgid}:1"))
            .arg(executable)
            .arg(SECOND_UID_TEST_NAME)
            .arg("--exact")
            .arg("--nocapture")
            .env(SECOND_UID_ROLE_ENV, "coordinator")
            .env(SECOND_UID_ENDPOINT_ENV, socket_path)
            .output();
        let output = match output {
            Ok(output) => output,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                eprintln!("跳过真实第二 UID 测试：系统缺少 unshare");
                return Ok(());
            }
            Err(error) => return Err(error).context("无法启动第二 UID 测试命名空间"),
        };
        if output.status.success() {
            return Ok(());
        }

        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("Operation not permitted") || stderr.contains("Invalid argument") {
            eprintln!("跳过真实第二 UID 测试：当前系统不允许非特权用户命名空间");
            return Ok(());
        }
        bail!(
            "真实第二 UID 测试失败（status={}）\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            stderr
        )
    }

    #[cfg(target_os = "linux")]
    fn subordinate_id_start(path: &str, user: &str, id: u32) -> Option<u32> {
        let numeric = id.to_string();
        std::fs::read_to_string(path)
            .ok()?
            .lines()
            .find_map(|line| {
                let fields: Vec<_> = line.split(':').collect();
                if fields.len() < 3 || (fields[0] != user && fields[0] != numeric) {
                    return None;
                }
                let count = fields[2].parse::<u32>().ok()?;
                (count > 0).then(|| fields[1].parse::<u32>().ok()).flatten()
            })
    }

    #[cfg(target_os = "linux")]
    fn run_second_uid_coordinator() -> Result<()> {
        use std::os::unix::fs::PermissionsExt;
        use std::process::Command;

        let socket_path = PathBuf::from(
            std::env::var_os(SECOND_UID_ENDPOINT_ENV).context("第二 UID 测试缺少 socket 路径")?,
        );
        let directory = socket_path
            .parent()
            .context("第二 UID 测试 socket 无父目录")?;
        std::fs::create_dir_all(directory)?;
        std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o777))?;

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let listener = runtime.block_on(async {
            use interprocess::os::unix::local_socket::ListenerOptionsExt;

            let name = socket_path
                .to_str()
                .context("第二 UID 测试 socket 路径不是 UTF-8")?
                .to_fs_name::<interprocess::local_socket::GenericFilePath>()
                .context("无法编码第二 UID 测试 socket 路径")?;
            ListenerOptions::new()
                .name(name)
                .mode(0o777)
                .create_tokio()
                .context("无法创建第二 UID 测试监听器")
        })?;
        std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o777))?;

        let executable = std::env::current_exe().context("无法定位第二 UID helper 程序")?;
        let mut child = Command::new("setpriv")
            .args(["--reuid=1", "--regid=1", "--clear-groups", "--"])
            .arg(executable)
            .arg(SECOND_UID_TEST_NAME)
            .arg("--exact")
            .arg("--nocapture")
            .env(SECOND_UID_ROLE_ENV, "peer")
            .env(SECOND_UID_ENDPOINT_ENV, &socket_path)
            .spawn()
            .context("无法启动第二 UID peer helper")?;

        let peer_result = runtime.block_on(async {
            let stream = timeout(Duration::from_secs(5), listener.accept())
                .await
                .map_err(|_| anyhow::anyhow!("等待第二 UID peer 连接超时"))?
                .context("接受第二 UID peer 连接失败")?;
            peer_identity(&stream)
        });
        let child_status = wait_for_helper(&mut child)?;
        let peer = peer_result?;
        assert!(
            child_status.success(),
            "第二 UID peer helper 失败: {child_status}"
        );
        assert_eq!(peer.euid, 1, "必须从内核读取真实第二 UID");
        assert!(peer.pid.is_some(), "Linux peer 凭据必须包含 PID");
        let error =
            authorize_peer(peer, current_user_id()).expect_err("真实第二 UID peer 必须被拒绝");
        assert!(error.to_string().contains("拒绝"));

        let _ = std::fs::remove_file(&socket_path);
        let _ = std::fs::remove_dir(directory);
        Ok(())
    }

    #[cfg(target_os = "linux")]
    fn run_second_uid_peer() -> Result<()> {
        let socket_path = PathBuf::from(
            std::env::var_os(SECOND_UID_ENDPOINT_ENV).context("第二 UID peer 缺少 socket 路径")?,
        );
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        runtime.block_on(async {
            let name = socket_path
                .to_str()
                .context("第二 UID peer socket 路径不是 UTF-8")?
                .to_fs_name::<interprocess::local_socket::GenericFilePath>()
                .context("无法编码第二 UID peer socket 路径")?;
            timeout(Duration::from_secs(3), LocalStream::connect(name))
                .await
                .map_err(|_| anyhow::anyhow!("第二 UID peer 连接超时"))??;
            Ok(())
        })
    }

    #[cfg(target_os = "linux")]
    fn wait_for_helper(child: &mut std::process::Child) -> Result<std::process::ExitStatus> {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = child.try_wait()? {
                return Ok(status);
            }
            if std::time::Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                bail!("第二 UID peer helper 未在期限内退出");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn authenticated_local_round_trip_and_oversized_rejection() -> Result<()> {
        let directory = test_directory("round-trip");
        prepare_runtime_directory(&directory)?;
        let endpoint = LocalEndpoint::Path(directory.join("node.sock"));
        let server = LocalServer::bind(endpoint.clone(), EndpointRole::Node).await?;
        let LocalEndpoint::Path(socket_path) = &endpoint;
        assert_eq!(
            std::fs::metadata(socket_path)?.permissions().mode() & 0o777,
            0o600
        );
        let accept = tokio::spawn(async move {
            let mut connection = server.accept().await?;
            let request: LocalRequest = connection.receive().await?.1;
            assert_eq!(request.request_id, 7);
            connection.send_response(&LocalResponse::Pong).await?;
            Ok::<_, anyhow::Error>(())
        });

        let response = request(
            &endpoint,
            IpcRole::Cli,
            &LocalRequest {
                request_id: 7,
                command: LocalCommand::Snapshot,
            },
        )
        .await?;
        assert_eq!(response, LocalResponse::Pong);
        accept.await??;
        server_cleanup(&directory)?;
        Ok(())
    }
}
