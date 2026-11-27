//! JNI entrypoints for `cc.xlei.fuckipv6.android.service.NativeNodeBridge`.
//!
//! All calls catch Rust panics before returning to JNI so a panic can never
//! unwind across the C ABI boundary.

#![cfg(target_os = "android")]

use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_void};
use std::path::PathBuf;
use std::ptr;
use std::sync::Arc;

use fuckipv6_client::node::{NodeProblem, NodeProblemCode};
use jni_sys::{
    jboolean, jint, jlong, jmethodID, jobject, jstring, JNIEnv, JavaVM, JNI_OK, JNI_TRUE,
};

use crate::platform::AndroidHostCallbacks;
use crate::AndroidNodeHandle;

struct JniHostCallbacks {
    vm: *mut JavaVM,
    callback_global_ref: jobject,
    open_method: jmethodID,
    close_method: jmethodID,
}

// SAFETY: JavaVM* and JNI global references are thread-safe across JVM threads
// once the calling thread attaches to JavaVM.
unsafe impl Send for JniHostCallbacks {}
unsafe impl Sync for JniHostCallbacks {}

impl Drop for JniHostCallbacks {
    fn drop(&mut self) {
        if let Ok(guard) = AttachedEnv::attach(self.vm) {
            unsafe {
                ((**guard.env).v1_6.DeleteGlobalRef)(guard.env, self.callback_global_ref);
            }
        }
    }
}

struct AttachedEnv {
    vm: *mut JavaVM,
    env: *mut JNIEnv,
    detach_on_drop: bool,
}

impl AttachedEnv {
    fn attach(vm: *mut JavaVM) -> Result<Self, NodeProblem> {
        let mut env_ptr: *mut c_void = ptr::null_mut();
        let status = unsafe { ((**vm).v1_4.GetEnv)(vm, &mut env_ptr, jni_sys::JNI_VERSION_1_6) };
        if status == JNI_OK && !env_ptr.is_null() {
            return Ok(Self {
                vm,
                env: env_ptr.cast::<JNIEnv>(),
                detach_on_drop: false,
            });
        }
        let attach_status =
            unsafe { ((**vm).v1_4.AttachCurrentThread)(vm, &mut env_ptr, ptr::null_mut()) };
        if attach_status != JNI_OK || env_ptr.is_null() {
            return Err(NodeProblem::new(
                NodeProblemCode::Internal,
                format!("JNI AttachCurrentThread failed ({attach_status})"),
            ));
        }
        Ok(Self {
            vm,
            env: env_ptr.cast::<JNIEnv>(),
            detach_on_drop: true,
        })
    }
}

impl Drop for AttachedEnv {
    fn drop(&mut self) {
        if self.detach_on_drop {
            unsafe {
                ((**self.vm).v1_4.DetachCurrentThread)(self.vm);
            }
        }
    }
}

impl AndroidHostCallbacks for JniHostCallbacks {
    fn open_network(&self, settings_json: &str) -> Result<i32, NodeProblem> {
        let guard = AttachedEnv::attach(self.vm)?;
        let c_settings = CString::new(settings_json).map_err(|_| {
            NodeProblem::new(
                NodeProblemCode::InvalidRequest,
                "settings JSON contained interior NUL",
            )
        })?;
        unsafe {
            let j_settings = ((**guard.env).v1_6.NewStringUTF)(guard.env, c_settings.as_ptr());
            if j_settings.is_null() {
                clear_pending_exception(guard.env);
                return Err(NodeProblem::new(
                    NodeProblemCode::Internal,
                    "JNI NewStringUTF failed for tunnel settings",
                ));
            }
            let fd = ((**guard.env).v1_6.CallIntMethod)(
                guard.env,
                self.callback_global_ref,
                self.open_method,
                j_settings,
            );
            ((**guard.env).v1_6.DeleteLocalRef)(guard.env, j_settings);
            if clear_pending_exception(guard.env) {
                return Err(NodeProblem::new(
                    NodeProblemCode::Unavailable,
                    "Android VpnService.openNetwork threw an exception",
                ));
            }
            if fd < 0 {
                return Err(NodeProblem::new(
                    NodeProblemCode::PermissionDenied,
                    format!("Android VpnService.openNetwork refused tunnel (code {fd})"),
                ));
            }
            Ok(fd)
        }
    }

    fn close_network(&self, network_name: &str) -> Result<(), NodeProblem> {
        let guard = AttachedEnv::attach(self.vm)?;
        let c_name = CString::new(network_name).map_err(|_| {
            NodeProblem::new(
                NodeProblemCode::InvalidRequest,
                "network name contained interior NUL",
            )
        })?;
        unsafe {
            let j_name = ((**guard.env).v1_6.NewStringUTF)(guard.env, c_name.as_ptr());
            if j_name.is_null() {
                clear_pending_exception(guard.env);
                return Ok(());
            }
            ((**guard.env).v1_6.CallVoidMethod)(
                guard.env,
                self.callback_global_ref,
                self.close_method,
                j_name,
            );
            ((**guard.env).v1_6.DeleteLocalRef)(guard.env, j_name);
            let _ = clear_pending_exception(guard.env);
        }
        Ok(())
    }
}

unsafe fn clear_pending_exception(env: *mut JNIEnv) -> bool {
    if ((**env).v1_6.ExceptionCheck)(env) == JNI_TRUE {
        ((**env).v1_6.ExceptionClear)(env);
        true
    } else {
        false
    }
}

unsafe fn jstring_to_string(env: *mut JNIEnv, input: jstring) -> Option<String> {
    if input.is_null() {
        return None;
    }
    let chars = ((**env).v1_6.GetStringUTFChars)(env, input, ptr::null_mut());
    if chars.is_null() {
        return None;
    }
    let result = CStr::from_ptr(chars).to_string_lossy().into_owned();
    ((**env).v1_6.ReleaseStringUTFChars)(env, input, chars);
    Some(result)
}

unsafe fn string_to_jstring(env: *mut JNIEnv, value: &str) -> jstring {
    let sanitized = value.replace('\0', "");
    let cstr = CString::new(sanitized).unwrap_or_else(|_| CString::new("{}").expect("valid json"));
    ((**env).v1_6.NewStringUTF)(env, cstr.as_ptr())
}

fn json_error(code: &str, message: &str) -> String {
    serde_json::json!({
        "ok": false,
        "code": code,
        "message": message,
    })
    .to_string()
}

#[no_mangle]
pub unsafe extern "system" fn Java_cc_xlei_fuckipv6_android_service_NativeNodeBridge_nativeStartRuntime(
    env: *mut JNIEnv,
    _class: jobject,
    storage_root: jstring,
    host_callback: jobject,
) -> jstring {
    let outcome = std::panic::catch_unwind(|| {
        let Some(root) = jstring_to_string(env, storage_root) else {
            return json_error("invalid_request", "storage_root was null");
        };
        if host_callback.is_null() {
            return json_error("invalid_request", "host_callback was null");
        }
        let mut vm: *mut JavaVM = ptr::null_mut();
        if ((**env).v1_6.GetJavaVM)(env, &mut vm) != JNI_OK || vm.is_null() {
            return json_error("internal", "GetJavaVM failed");
        }
        let callback_class = ((**env).v1_6.GetObjectClass)(env, host_callback);
        if callback_class.is_null() {
            clear_pending_exception(env);
            return json_error("internal", "GetObjectClass failed");
        }
        let open_name = CString::new("openNetwork").expect("valid cstr");
        let open_sig = CString::new("(Ljava/lang/String;)I").expect("valid cstr");
        let close_name = CString::new("closeNetwork").expect("valid cstr");
        let close_sig = CString::new("(Ljava/lang/String;)V").expect("valid cstr");
        let open_method = ((**env).v1_6.GetMethodID)(
            env,
            callback_class,
            open_name.as_ptr() as *const c_char,
            open_sig.as_ptr() as *const c_char,
        );
        let close_method = ((**env).v1_6.GetMethodID)(
            env,
            callback_class,
            close_name.as_ptr() as *const c_char,
            close_sig.as_ptr() as *const c_char,
        );
        ((**env).v1_6.DeleteLocalRef)(env, callback_class);
        if open_method.is_null() || close_method.is_null() || clear_pending_exception(env) {
            return json_error("internal", "host callback methods not found");
        }
        let global_ref = ((**env).v1_6.NewGlobalRef)(env, host_callback);
        if global_ref.is_null() {
            return json_error("internal", "NewGlobalRef failed");
        }
        let callbacks: Arc<dyn AndroidHostCallbacks> = Arc::new(JniHostCallbacks {
            vm,
            callback_global_ref: global_ref,
            open_method,
            close_method,
        });
        match AndroidNodeHandle::start(PathBuf::from(root), callbacks) {
            Ok(handle) => {
                let api_url = handle.api_url().to_owned();
                let token = handle.api_token().to_owned();
                let node_id = handle.node_id().to_owned();
                let ptr = Box::into_raw(Box::new(handle)) as jlong;
                serde_json::json!({
                    "ok": true,
                    "handle": ptr,
                    "api_url": api_url,
                    "token": token,
                    "node_id": node_id,
                })
                .to_string()
            }
            Err(problem) => json_error(&format!("{:?}", problem.code), &problem.message),
        }
    });
    let payload = outcome.unwrap_or_else(|_| json_error("panic", "nativeStartRuntime panicked"));
    string_to_jstring(env, &payload)
}

#[no_mangle]
pub unsafe extern "system" fn Java_cc_xlei_fuckipv6_android_service_NativeNodeBridge_nativeStopRuntime(
    env: *mut JNIEnv,
    _class: jobject,
    handle: jlong,
) -> jstring {
    let outcome = std::panic::catch_unwind(|| {
        if handle == 0 {
            return serde_json::json!({"ok": true}).to_string();
        }
        let boxed = Box::from_raw(handle as *mut AndroidNodeHandle);
        match boxed.stop() {
            Ok(()) => serde_json::json!({"ok": true}).to_string(),
            Err(problem) => json_error(&format!("{:?}", problem.code), &problem.message),
        }
    });
    let payload = outcome.unwrap_or_else(|_| json_error("panic", "nativeStopRuntime panicked"));
    string_to_jstring(env, &payload)
}

#[no_mangle]
pub unsafe extern "system" fn Java_cc_xlei_fuckipv6_android_service_NativeNodeBridge_nativeNotifyNetworkPathChanged(
    _env: *mut JNIEnv,
    _class: jobject,
    handle: jlong,
) -> jboolean {
    let outcome = std::panic::catch_unwind(|| {
        if handle == 0 {
            return false;
        }
        let runtime = &*(handle as *const AndroidNodeHandle);
        runtime.notify_network_path_changed().is_ok()
    });
    outcome.unwrap_or(false)
}

#[no_mangle]
pub unsafe extern "system" fn Java_cc_xlei_fuckipv6_android_service_NativeNodeBridge_nativeGetTelemetryJson(
    env: *mut JNIEnv,
    _class: jobject,
    handle: jlong,
) -> jstring {
    let outcome = std::panic::catch_unwind(|| {
        if handle == 0 {
            return json_error("unavailable", "runtime handle is null");
        }
        let runtime = &*(handle as *const AndroidNodeHandle);
        serde_json::to_string(&runtime.telemetry())
            .unwrap_or_else(|_| json_error("internal", "failed to serialize telemetry"))
    });
    let payload =
        outcome.unwrap_or_else(|_| json_error("panic", "nativeGetTelemetryJson panicked"));
    string_to_jstring(env, &payload)
}

#[no_mangle]
pub unsafe extern "system" fn Java_cc_xlei_fuckipv6_android_service_NativeNodeBridge_nativeAbiVersion(
    _env: *mut JNIEnv,
    _class: jobject,
) -> jint {
    1
}
