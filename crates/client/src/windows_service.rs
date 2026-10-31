//! Windows Service Control Manager integration for the internal service role.
//!
//! `sc.exe` only registers the service. The process itself must connect to the
//! SCM, report status and translate SCM stop/shutdown controls into the same
//! graceful shutdown path used by SIGTERM on Unix.

use std::ffi::{c_void, OsStr};
use std::os::windows::ffi::OsStrExt;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use tokio::sync::watch;
use windows_sys::Win32::Foundation::{GetLastError, ERROR_FAILED_SERVICE_CONTROLLER_CONNECT};
use windows_sys::Win32::System::Services::{
    RegisterServiceCtrlHandlerExW, SetServiceStatus, StartServiceCtrlDispatcherW,
    SERVICE_ACCEPT_SHUTDOWN, SERVICE_ACCEPT_STOP, SERVICE_CONTROL_SHUTDOWN, SERVICE_CONTROL_STOP,
    SERVICE_RUNNING, SERVICE_START_PENDING, SERVICE_STATUS, SERVICE_STATUS_HANDLE, SERVICE_STOPPED,
    SERVICE_STOP_PENDING, SERVICE_TABLE_ENTRYW, SERVICE_WIN32_OWN_PROCESS,
};

use crate::service::SERVICE_NAME;

const START_WAIT_HINT: Duration = Duration::from_secs(60);
const STOP_WAIT_HINT: Duration = Duration::from_secs(70);

type ServiceEntry = Box<dyn FnOnce(watch::Receiver<bool>) -> Result<()> + Send>;

static SERVICE_ENTRY: OnceLock<Mutex<Option<ServiceEntry>>> = OnceLock::new();
static SERVICE_RESULT: OnceLock<Mutex<Option<Result<()>>>> = OnceLock::new();
static SERVICE_SHUTDOWN: OnceLock<watch::Sender<bool>> = OnceLock::new();
static STATUS_HANDLE: AtomicPtr<c_void> = AtomicPtr::new(null_mut());

/// Connect this process to SCM and run `entry` until SCM requests a stop.
///
/// This must be called on the process main thread before a Tokio runtime is
/// created, because `StartServiceCtrlDispatcherW` owns the dispatcher loop.
pub(crate) fn run<F>(entry: F) -> Result<()>
where
    F: FnOnce(watch::Receiver<bool>) -> Result<()> + Send + 'static,
{
    let mut slot = SERVICE_ENTRY
        .get_or_init(|| Mutex::new(None))
        .lock()
        .map_err(|_| anyhow!("Windows Service 入口锁已损坏"))?;
    if slot.is_some() {
        bail!("Windows Service 入口被重复初始化");
    }
    *slot = Some(Box::new(entry));
    drop(slot);

    let service_name = wide(SERVICE_NAME);
    let table = [
        SERVICE_TABLE_ENTRYW {
            lpServiceName: service_name.as_ptr().cast_mut(),
            lpServiceProc: Some(service_main),
        },
        SERVICE_TABLE_ENTRYW {
            lpServiceName: null_mut(),
            lpServiceProc: None,
        },
    ];

    // SAFETY: both strings and the table live until the blocking dispatcher
    // returns. The callback is a valid `extern "system"` function.
    let started = unsafe { StartServiceCtrlDispatcherW(table.as_ptr()) };
    if started == 0 {
        let code = unsafe { GetLastError() };
        if code == ERROR_FAILED_SERVICE_CONTROLLER_CONNECT {
            bail!("`__service --windows-service` 必须由 Windows SCM 启动");
        }
        return Err(std::io::Error::from_raw_os_error(code as i32))
            .context("Windows Service 无法连接 SCM");
    }

    SERVICE_RESULT
        .get_or_init(|| Mutex::new(None))
        .lock()
        .map_err(|_| anyhow!("Windows Service 结果锁已损坏"))?
        .take()
        .unwrap_or_else(|| Err(anyhow!("Windows Service 未执行服务主函数")))
}

/// Report that node and WebUI initialization completed successfully.
pub(crate) fn report_running() {
    set_status(SERVICE_RUNNING, running_controls(), Duration::ZERO);
}

unsafe extern "system" fn service_main(_argc: u32, _argv: *mut windows_sys::core::PWSTR) {
    let service_name = wide(SERVICE_NAME);
    // SAFETY: the name is NUL-terminated and remains alive for this callback.
    let handle = unsafe {
        RegisterServiceCtrlHandlerExW(service_name.as_ptr(), Some(service_control_handler), null())
    };
    if handle.is_null() {
        store_result(
            Err(std::io::Error::last_os_error()).context("无法注册 Windows Service 控制处理器"),
        );
        return;
    }
    STATUS_HANDLE.store(handle, Ordering::Release);
    set_status(SERVICE_START_PENDING, 0, START_WAIT_HINT);

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let _ = SERVICE_SHUTDOWN.set(shutdown_tx);
    let entry = SERVICE_ENTRY
        .get()
        .and_then(|slot| slot.lock().ok())
        .and_then(|mut slot| slot.take());
    let result = match entry {
        Some(entry) => {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| entry(shutdown_rx)))
                .map_err(|_| anyhow!("Windows Service 主任务发生 panic"))
                .and_then(|result| result)
        }
        None => Err(anyhow!("Windows Service 入口未初始化")),
    };
    store_result(result);

    set_status(SERVICE_STOPPED, 0, Duration::ZERO);
    STATUS_HANDLE.store(null_mut(), Ordering::Release);
}

unsafe extern "system" fn service_control_handler(
    control: u32,
    _event_type: u32,
    _event_data: *mut c_void,
    _context: *mut c_void,
) -> u32 {
    match control {
        SERVICE_CONTROL_STOP | SERVICE_CONTROL_SHUTDOWN => {
            set_status(SERVICE_STOP_PENDING, 0, STOP_WAIT_HINT);
            if let Some(shutdown) = SERVICE_SHUTDOWN.get() {
                shutdown.send_replace(true);
            }
            0
        }
        _ => 0,
    }
}

fn set_status(state: u32, controls_accepted: u32, wait_hint: Duration) {
    let handle: SERVICE_STATUS_HANDLE = STATUS_HANDLE.load(Ordering::Acquire);
    if handle.is_null() {
        return;
    }
    let status = SERVICE_STATUS {
        dwServiceType: SERVICE_WIN32_OWN_PROCESS,
        dwCurrentState: state,
        dwControlsAccepted: controls_accepted,
        dwWin32ExitCode: 0,
        dwServiceSpecificExitCode: 0,
        dwCheckPoint: 0,
        dwWaitHint: wait_hint.as_millis().min(u32::MAX as u128) as u32,
    };
    // SAFETY: `handle` came from RegisterServiceCtrlHandlerExW and remains
    // valid until service_main returns.
    let _ = unsafe { SetServiceStatus(handle, &status) };
}

fn store_result(result: Result<()>) {
    if let Ok(mut slot) = SERVICE_RESULT.get_or_init(|| Mutex::new(None)).lock() {
        *slot = Some(result);
    }
}

fn running_controls() -> u32 {
    SERVICE_ACCEPT_STOP | SERVICE_ACCEPT_SHUTDOWN
}

fn wide(value: &str) -> Vec<u16> {
    OsStr::new(value)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_name_is_nul_terminated() {
        let encoded = wide(SERVICE_NAME);
        assert_eq!(encoded.last(), Some(&0));
        assert_eq!(encoded.len(), SERVICE_NAME.len() + 1);
    }

    #[test]
    fn accepts_stop_and_shutdown() {
        assert_eq!(
            running_controls(),
            SERVICE_ACCEPT_STOP | SERVICE_ACCEPT_SHUTDOWN
        );
    }
}
