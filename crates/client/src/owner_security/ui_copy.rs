//! Canonical owner-authentication copy shared by GPUI and every browser auth surface.
//!
//! These strings intentionally describe security boundaries rather than branding.
//! The browser asset is generated from the same Rust constants so GPUI and WebUI
//! cannot silently drift on the local-identity and recovery guidance.

pub(crate) const LOGIN_TITLE: &str = "节点所有者登录";
pub(crate) const LOGIN_DESCRIPTION: &str =
    "使用已注册的 Passkey 验证节点所有者身份。会话只保存在 HttpOnly、SameSite=Strict Cookie 中，页面不读取或持久化访问令牌。";
pub(crate) const LOGIN_HELP: &str =
    "Passkey 需要受信任的 HTTPS 地址或 http://localhost。若浏览器在页面加载前报告证书或域名错误，必须修复证书与 WebAuthn origin；应用不会提供跳过校验。";
pub(crate) const SECURE_CONTEXT_REQUIREMENTS: &str =
    "当前页面不是受信任的 WebAuthn 安全上下文。请使用配置的 HTTPS 公网地址或 http://localhost；证书或域名错误必须先修复，不能跳过校验。";
pub(crate) const PASSKEY_UNSUPPORTED: &str =
    "当前浏览器或平台不支持所需的 Passkey 能力。请使用支持 WebAuthn 的现代浏览器和系统认证器。";
pub(crate) const ORIGIN_MISMATCH_TEMPLATE: &str =
    "WebAuthn 地址不匹配。预期 {expected}，当前 {actual}。请使用 {expected} 打开；若浏览器报告证书错误，必须先修复证书，不能跳过校验。";
pub(crate) const BOOTSTRAP_DESCRIPTION: &str =
    "这个节点只属于一位所有者。请使用节点所有者可访问终端中显示的一次性初始化码注册 Passkey，并离线保存随后生成的恢复码。";
pub(crate) const BOOTSTRAP_CODE_HELP: &str =
    "初始化码只通过节点所有者可访问的终端显示，不会写入 URL、日志或磁盘明文。";
pub(crate) const RECOVERY_CODE_WARNING: &str =
    "恢复码只显示这一次。全部 Passkey 丢失时，它是本机恢复的唯一凭据，请立即保存到离线安全位置。";
pub(crate) const RECOVERY_CODE_CONFIRMATION: &str =
    "我已将恢复码保存到离线安全位置，并确认页面关闭后无法再次查看。";
pub(crate) const LOCAL_IDENTITY_TITLE: &str = "本地 GPUI 身份";
pub(crate) const LOCAL_IDENTITY_DETAIL: &str =
    "本地 GPUI 使用操作系统用户身份与本地 IPC，不显示网页 WebAuthn 会话。";
pub(crate) const WEBUI_IDENTITY_TITLE: &str = "WebUI 认证";
pub(crate) const WEBUI_IDENTITY_DETAIL: &str =
    "公网 WebUI 使用 Passkey 和 HttpOnly 同源会话；会话状态只在 WebUI 中实时显示。";
pub(crate) const PASSKEY_RECOVERY_HELP: &str =
    "全部 Passkey 丢失时，只能在节点主机执行 fuckipv6 recover-owner；不提供公网找回入口。";
pub(crate) const REAUTH_TITLE: &str = "确认所有者身份";
pub(crate) const REAUTH_DESCRIPTION: &str =
    "该安全操作要求最近五分钟内完成 Passkey 重认证。重认证成功后会自动继续原操作。";
pub(crate) const OPERATION_NOT_EXECUTED: &str = "操作未执行：Passkey 重认证已取消。";
pub(crate) const LAST_PASSKEY_ERROR: &str = "不能撤销最后一个可用 Passkey。请先注册新的 Passkey。";

/// Build the browser copy asset from the canonical Rust values.
pub(crate) fn browser_script() -> String {
    let copy = serde_json::json!({
        "loginTitle": LOGIN_TITLE,
        "loginDescription": LOGIN_DESCRIPTION,
        "loginHelp": LOGIN_HELP,
        "secureContextRequirements": SECURE_CONTEXT_REQUIREMENTS,
        "passkeyUnsupported": PASSKEY_UNSUPPORTED,
        "originMismatchTemplate": ORIGIN_MISMATCH_TEMPLATE,
        "bootstrapDescription": BOOTSTRAP_DESCRIPTION,
        "bootstrapCodeHelp": BOOTSTRAP_CODE_HELP,
        "recoveryCodeWarning": RECOVERY_CODE_WARNING,
        "recoveryCodeConfirmation": RECOVERY_CODE_CONFIRMATION,
        "localIdentityTitle": LOCAL_IDENTITY_TITLE,
        "localIdentityDetail": LOCAL_IDENTITY_DETAIL,
        "webuiIdentityTitle": WEBUI_IDENTITY_TITLE,
        "webuiIdentityDetail": WEBUI_IDENTITY_DETAIL,
        "passkeyRecoveryHelp": PASSKEY_RECOVERY_HELP,
        "reauthTitle": REAUTH_TITLE,
        "reauthDescription": REAUTH_DESCRIPTION,
        "operationNotExecuted": OPERATION_NOT_EXECUTED,
        "lastPasskeyError": LAST_PASSKEY_ERROR,
    });
    format!(
        "(function (root) {{\n  \"use strict\";\n  root.FuckIPV6OwnerAuthCopy = Object.freeze({copy});\n}})(typeof globalThis !== \"undefined\" ? globalThis : this);\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_copy_is_generated_from_canonical_auth_boundaries() {
        let script = browser_script();
        assert!(script.contains("FuckIPV6OwnerAuthCopy"));
        assert!(script.contains(LOGIN_TITLE));
        assert!(script.contains(SECURE_CONTEXT_REQUIREMENTS));
        assert!(script.contains(PASSKEY_RECOVERY_HELP));
        assert!(script.contains("originMismatchTemplate"));
        assert!(!script.contains("localStorage"));
        assert!(!script.contains("sessionStorage"));
    }

    #[test]
    fn local_and_webui_identity_copy_are_distinct_and_explicit() {
        assert!(LOCAL_IDENTITY_DETAIL.contains("操作系统用户身份"));
        assert!(LOCAL_IDENTITY_DETAIL.contains("本地 IPC"));
        assert!(WEBUI_IDENTITY_DETAIL.contains("Passkey"));
        assert!(WEBUI_IDENTITY_DETAIL.contains("HttpOnly"));
        assert!(PASSKEY_RECOVERY_HELP.contains("recover-owner"));
        assert!(LOGIN_HELP.contains("不会提供跳过校验"));
    }
}
