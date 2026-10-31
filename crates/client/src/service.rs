//! Platform service installation and lifecycle management.
//!
//! The service definitions invoke the internal `__service` role directly. That
//! process owns the control lease and supervises node and WebUI, so the service
//! manager never supervises only a UI shell.

use std::ffi::{OsStr, OsString};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use clap::{Args, Subcommand, ValueEnum};

pub(crate) const SERVICE_NAME: &str = "fuckipv6";
const SYSTEMD_UNIT: &str = "fuckipv6.service";
const LAUNCHD_LABEL: &str = "cc.xlei.fuckipv6";
const OPTIONS_FILE: &str = "service.json";
const STOP_TIMEOUT_SECONDS: u64 = 75;

#[derive(Debug, Args)]
pub struct ServiceArgs {
    #[command(subcommand)]
    pub action: ServiceAction,

    /// Install or manage a system-wide service (default) or a per-user service.
    #[arg(long, value_enum, default_value_t = ServiceScope::System, global = true)]
    pub scope: ServiceScope,

    /// WebUI port written only when creating a new service configuration.
    #[arg(long, default_value_t = 8080, global = true)]
    pub webui_port: u16,
}

#[derive(Debug, Clone, Copy, Subcommand)]
pub enum ServiceAction {
    /// Install the current binary and start the service.
    Install,
    /// Replace the installed binary while preserving configuration.
    Upgrade,
    /// Start the installed service.
    Start,
    /// Stop the installed service.
    Stop,
    /// Restart the installed service.
    Restart,
    /// Show the native service manager status.
    Status,
    /// Remove the service and binary while preserving configuration and state.
    Uninstall,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ServiceScope {
    System,
    User,
}

pub fn run(args: ServiceArgs) -> Result<()> {
    let layout = ServiceLayout::detect(args.scope)?;
    let source = std::env::current_exe().context("无法定位当前 fuckipv6 二进制")?;
    let manager = ServiceManager::new(layout, Arc::new(SystemCommandRunner), source);

    match args.action {
        ServiceAction::Install => manager.install(args.webui_port)?,
        ServiceAction::Upgrade => manager.upgrade(args.webui_port)?,
        ServiceAction::Start => manager.start()?,
        ServiceAction::Stop => manager.stop()?,
        ServiceAction::Restart => manager.restart()?,
        ServiceAction::Status => println!("{}", manager.status()?),
        ServiceAction::Uninstall => manager.uninstall()?,
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // Platform-only variants are rendered and tested on every host.
enum ServicePlatform {
    Systemd,
    Launchd,
    Windows,
}

#[derive(Debug, Clone)]
struct ServiceLayout {
    platform: ServicePlatform,
    scope: ServiceScope,
    binary: PathBuf,
    config_dir: PathBuf,
    state_dir: PathBuf,
    log_dir: PathBuf,
    runtime_dir: PathBuf,
    service_file: PathBuf,
}

impl ServiceLayout {
    fn detect(scope: ServiceScope) -> Result<Self> {
        #[cfg(target_os = "linux")]
        {
            return Self::linux(scope);
        }
        #[cfg(target_os = "macos")]
        {
            return Self::macos(scope);
        }
        #[cfg(windows)]
        {
            if scope == ServiceScope::User {
                bail!("Windows Service 仅支持系统作用域");
            }
            return Self::windows();
        }
        #[allow(unreachable_code)]
        {
            let _ = scope;
            bail!("当前平台没有可用的服务管理器适配器")
        }
    }

    #[cfg(target_os = "linux")]
    fn linux(scope: ServiceScope) -> Result<Self> {
        if scope == ServiceScope::System {
            return Ok(Self {
                platform: ServicePlatform::Systemd,
                scope,
                binary: PathBuf::from("/opt/fuckipv6/fuckipv6"),
                config_dir: PathBuf::from("/etc/fuckipv6"),
                state_dir: PathBuf::from("/var/lib/fuckipv6"),
                log_dir: PathBuf::from("/var/log/fuckipv6"),
                runtime_dir: PathBuf::from("/run/fuckipv6"),
                service_file: PathBuf::from("/etc/systemd/system/fuckipv6.service"),
            });
        }

        let home = env_path("HOME").context("用户服务需要 HOME")?;
        let config_root = env_path("XDG_CONFIG_HOME").unwrap_or_else(|| home.join(".config"));
        let state_root = env_path("XDG_STATE_HOME").unwrap_or_else(|| home.join(".local/state"));
        let runtime_dir = env_path("XDG_RUNTIME_DIR")
            .unwrap_or_else(|| std::env::temp_dir().join(format!("fuckipv6-{}", current_uid())));
        Ok(Self {
            platform: ServicePlatform::Systemd,
            scope,
            binary: home.join(".local/bin/fuckipv6"),
            config_dir: config_root.join("fuckipv6"),
            state_dir: state_root.join("fuckipv6"),
            log_dir: state_root.join("fuckipv6/log"),
            runtime_dir: runtime_dir.join("fuckipv6"),
            service_file: config_root.join("systemd/user/fuckipv6.service"),
        })
    }

    #[cfg(target_os = "macos")]
    fn macos(scope: ServiceScope) -> Result<Self> {
        if scope == ServiceScope::System {
            return Ok(Self {
                platform: ServicePlatform::Launchd,
                scope,
                binary: PathBuf::from("/usr/local/fuckipv6/fuckipv6"),
                config_dir: PathBuf::from("/usr/local/etc/fuckipv6"),
                state_dir: PathBuf::from("/usr/local/var/lib/fuckipv6"),
                log_dir: PathBuf::from("/usr/local/var/log/fuckipv6"),
                runtime_dir: PathBuf::from("/usr/local/var/run/fuckipv6"),
                service_file: PathBuf::from("/Library/LaunchDaemons/cc.xlei.fuckipv6.plist"),
            });
        }

        let home = env_path("HOME").context("用户服务需要 HOME")?;
        let support = home.join("Library/Application Support/fuckipv6");
        let runtime_dir = std::env::temp_dir().join("fuckipv6");
        Ok(Self {
            platform: ServicePlatform::Launchd,
            scope,
            binary: home.join(".local/bin/fuckipv6"),
            config_dir: support.join("config"),
            state_dir: support.join("state"),
            log_dir: home.join("Library/Logs/fuckipv6"),
            runtime_dir,
            service_file: home.join("Library/LaunchAgents/cc.xlei.fuckipv6.plist"),
        })
    }

    #[cfg(windows)]
    fn windows() -> Result<Self> {
        let program_data = env_path("ProgramData").context("缺少 ProgramData")?;
        let program_files = env_path("ProgramFiles").context("缺少 ProgramFiles")?;
        let root = program_data.join("fuckipv6");
        Ok(Self {
            platform: ServicePlatform::Windows,
            scope: ServiceScope::System,
            binary: program_files.join("fuckipv6/fuckipv6.exe"),
            config_dir: root.join("config"),
            state_dir: root.join("state"),
            log_dir: root.join("log"),
            runtime_dir: root.join("runtime"),
            service_file: root.join("service.json"),
        })
    }

    fn options_file(&self) -> PathBuf {
        self.config_dir.join(OPTIONS_FILE)
    }
}

#[cfg(target_os = "linux")]
fn current_uid() -> u32 {
    // SAFETY: geteuid has no preconditions and does not dereference pointers.
    unsafe { libc::geteuid() }
}

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name).map(PathBuf::from)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CommandResult {
    success: bool,
    stdout: String,
    stderr: String,
}

impl CommandResult {
    fn combined_output(&self) -> String {
        match (self.stdout.trim(), self.stderr.trim()) {
            ("", "") => String::new(),
            (stdout, "") => stdout.to_string(),
            ("", stderr) => stderr.to_string(),
            (stdout, stderr) => format!("{stdout}\n{stderr}"),
        }
    }
}

trait CommandRunner: Send + Sync {
    fn run(&self, program: &OsStr, args: &[OsString]) -> Result<CommandResult>;
}

struct SystemCommandRunner;

impl CommandRunner for SystemCommandRunner {
    fn run(&self, program: &OsStr, args: &[OsString]) -> Result<CommandResult> {
        let output = Command::new(program)
            .args(args)
            .output()
            .with_context(|| format!("无法执行 {}", Path::new(program).display()))?;
        Ok(CommandResult {
            success: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

struct ServiceManager {
    layout: ServiceLayout,
    runner: Arc<dyn CommandRunner>,
    source_binary: PathBuf,
}

impl ServiceManager {
    fn new(layout: ServiceLayout, runner: Arc<dyn CommandRunner>, source_binary: PathBuf) -> Self {
        Self {
            layout,
            runner,
            source_binary,
        }
    }

    fn install(&self, webui_port: u16) -> Result<()> {
        if self.is_registered()? {
            bail!("服务已安装；升级请使用 `fuckipv6 service upgrade`");
        }
        self.create_layout()?;
        self.write_default_options(webui_port)?;
        self.install_binary()?;
        self.write_service_definition()?;
        self.register()?;
        self.start()?;
        println!(
            "已安装并启动 fuckipv6 {} 服务；访问 http://127.0.0.1:{webui_port}",
            scope_name(self.layout.scope)
        );
        Ok(())
    }

    fn upgrade(&self, webui_port: u16) -> Result<()> {
        if !self.is_registered()? {
            bail!("服务尚未安装；请先运行 `fuckipv6 service install`");
        }
        self.create_layout()?;
        self.write_default_options(webui_port)?;
        let was_active = self.is_active()?;
        if was_active {
            self.stop()?;
        }

        let mut backup = None;
        let result: Result<()> = (|| {
            backup = self.backup_installed_binary()?;
            self.install_binary()?;
            self.write_service_definition()?;
            self.register()?;
            if was_active {
                self.start()?;
            }
            Ok(())
        })();

        if result.is_err() {
            if let Some(backup) = backup.as_deref() {
                if let Err(rollback_error) = copy_atomic(backup, &self.layout.binary, 0o755) {
                    eprintln!("升级失败后恢复旧二进制也失败: {rollback_error:#}");
                }
            }
            if was_active {
                if let Err(restart_error) = self.start() {
                    eprintln!("升级失败后恢复原服务状态也失败: {restart_error:#}");
                }
            }
        }
        if let Some(backup) = backup {
            let _ = fs::remove_file(backup);
        }
        result?;
        println!(
            "已升级 fuckipv6 二进制；原服务状态{}，配置未覆盖",
            if was_active {
                "已恢复运行"
            } else {
                "保持停止"
            }
        );
        Ok(())
    }

    fn start(&self) -> Result<()> {
        match self.layout.platform {
            ServicePlatform::Systemd => {
                self.systemctl_checked(&["start", SYSTEMD_UNIT])?;
            }
            ServicePlatform::Launchd => self.start_launchd()?,
            ServicePlatform::Windows => {
                if !self.is_active()? {
                    self.sc_checked(&["start", SERVICE_NAME])?;
                }
            }
        }
        Ok(())
    }

    fn stop(&self) -> Result<()> {
        match self.layout.platform {
            ServicePlatform::Systemd => {
                self.systemctl_checked(&["stop", SYSTEMD_UNIT])?;
            }
            ServicePlatform::Launchd => self.bootout_launchd(true)?,
            ServicePlatform::Windows => {
                if self.is_active()? {
                    self.sc_checked(&["stop", SERVICE_NAME])?;
                }
            }
        }
        Ok(())
    }

    fn restart(&self) -> Result<()> {
        match self.layout.platform {
            ServicePlatform::Systemd => {
                self.systemctl_checked(&["restart", SYSTEMD_UNIT])?;
            }
            ServicePlatform::Launchd => {
                self.bootout_launchd(true)?;
                self.bootstrap_launchd()?;
            }
            ServicePlatform::Windows => {
                self.stop()?;
                self.start()?;
            }
        }
        Ok(())
    }

    fn status(&self) -> Result<String> {
        let result = match self.layout.platform {
            ServicePlatform::Systemd => {
                self.systemctl(&["status", "--no-pager", "--full", SYSTEMD_UNIT])?
            }
            ServicePlatform::Launchd => {
                let domain = self.launchd_domain()?;
                self.run_os(
                    OsStr::new("launchctl"),
                    &[
                        OsString::from("print"),
                        OsString::from(format!("{domain}/{LAUNCHD_LABEL}")),
                    ],
                )?
            }
            ServicePlatform::Windows => self.sc(&["query", SERVICE_NAME])?,
        };
        let output = result.combined_output();
        if output.is_empty() {
            Ok(format!(
                "服务状态: {}",
                if result.success { "ok" } else { "failed" }
            ))
        } else {
            Ok(output)
        }
    }

    fn uninstall(&self) -> Result<()> {
        if !self.is_registered()? {
            bail!("服务尚未安装");
        }
        if self.is_active()? {
            self.stop()?;
        }
        match self.layout.platform {
            ServicePlatform::Systemd => {
                let _ = self.systemctl(&["disable", SYSTEMD_UNIT]);
                fs::remove_file(&self.layout.service_file).with_context(|| {
                    format!("无法删除服务定义 {}", self.layout.service_file.display())
                })?;
                self.systemctl_checked(&["daemon-reload"])?;
            }
            ServicePlatform::Launchd => {
                let _ = fs::remove_file(&self.layout.service_file);
            }
            ServicePlatform::Windows => {
                self.sc_checked(&["delete", SERVICE_NAME])?;
            }
        }
        if self.layout.binary.exists() {
            fs::remove_file(&self.layout.binary)
                .with_context(|| format!("无法删除 {}", self.layout.binary.display()))?;
        }
        println!("已卸载服务与二进制；配置、身份和状态数据已保留");
        Ok(())
    }

    fn create_layout(&self) -> Result<()> {
        for directory in [
            self.layout.binary.parent(),
            Some(self.layout.config_dir.as_path()),
            Some(self.layout.state_dir.as_path()),
            Some(self.layout.log_dir.as_path()),
            Some(self.layout.runtime_dir.as_path()),
            self.layout.service_file.parent(),
        ]
        .into_iter()
        .flatten()
        {
            fs::create_dir_all(directory)
                .with_context(|| format!("无法创建目录 {}", directory.display()))?;
        }
        for directory in [
            &self.layout.config_dir,
            &self.layout.state_dir,
            &self.layout.log_dir,
            &self.layout.runtime_dir,
        ] {
            set_mode(directory, 0o700)?;
        }
        Ok(())
    }

    fn write_default_options(&self, webui_port: u16) -> Result<()> {
        let path = self.layout.options_file();
        if path.exists() {
            return Ok(());
        }
        let encoded = crate::runtime_process::service_options_json(webui_port)?;
        write_atomic(&path, encoded.as_bytes(), 0o600)
    }

    fn backup_installed_binary(&self) -> Result<Option<PathBuf>> {
        if !self.layout.binary.exists() {
            return Ok(None);
        }
        let backup = self.layout.binary.with_extension(format!(
            "upgrade-backup-{}-{}",
            std::process::id(),
            monotonic_nonce()
        ));
        copy_atomic(&self.layout.binary, &backup, 0o755)?;
        Ok(Some(backup))
    }

    fn install_binary(&self) -> Result<()> {
        let source = fs::canonicalize(&self.source_binary)
            .with_context(|| format!("无法读取待安装二进制 {}", self.source_binary.display()))?;
        let target =
            fs::canonicalize(&self.layout.binary).unwrap_or_else(|_| self.layout.binary.clone());
        if source == target {
            bail!("升级时必须从新版本二进制运行，不能从已安装二进制原地升级");
        }
        copy_atomic(&source, &self.layout.binary, 0o755)
    }

    fn write_service_definition(&self) -> Result<()> {
        match self.layout.platform {
            ServicePlatform::Systemd => {
                let definition = self.render_systemd()?;
                write_atomic(&self.layout.service_file, definition.as_bytes(), 0o644)
            }
            ServicePlatform::Launchd => {
                let definition = self.render_launchd()?;
                write_atomic(&self.layout.service_file, definition.as_bytes(), 0o644)
            }
            ServicePlatform::Windows => Ok(()),
        }
    }

    fn render_systemd(&self) -> Result<String> {
        let executable = quote_systemd(path_str(&self.layout.binary)?);
        let options = quote_systemd(path_str(&self.layout.options_file())?);
        let config = quote_systemd(path_str(&self.layout.config_dir)?);
        let runtime = quote_systemd(path_str(&self.layout.runtime_dir)?);
        let state = quote_systemd(path_str(&self.layout.state_dir)?);
        let log = quote_systemd(path_str(&self.layout.log_dir)?);
        let mut definition = String::from(
            "[Unit]\nDescription=fuckipv6 node and control service\nAfter=network-online.target\nWants=network-online.target\n\n[Service]\nType=simple\n",
        );
        if self.layout.scope == ServiceScope::System {
            definition.push_str("User=root\nGroup=root\nNoNewPrivileges=false\n");
            definition.push_str("AmbientCapabilities=CAP_NET_ADMIN CAP_NET_RAW\n");
            definition.push_str("CapabilityBoundingSet=CAP_NET_ADMIN CAP_NET_RAW\n");
            definition.push_str("ProtectSystem=strict\nProtectHome=true\nPrivateTmp=true\n");
            definition.push_str(&format!(
                "ReadWritePaths={} {} {} {}\n",
                config, state, log, runtime
            ));
        }
        definition.push_str(&format!(
            "WorkingDirectory={}\nExecStart={} __service --options-file {} --config-dir {}\nEnvironment={}\nEnvironment={}\n",
            quote_systemd(path_str(&self.layout.state_dir)?),
            executable,
            options,
            config,
            quote_systemd(&format!("FUCKIPV6_RUNTIME_DIR={}", self.layout.runtime_dir.display())),
            quote_systemd(&format!("RUST_LOG={}", "info")),
        ));
        definition.push_str(
            "UMask=0077\nRestart=on-failure\nRestartSec=5s\nStartLimitIntervalSec=30s\nStartLimitBurst=3\n",
        );
        definition.push_str(&format!(
            "KillMode=mixed\nTimeoutStopSec={STOP_TIMEOUT_SECONDS}s\nStandardOutput=journal\nStandardError=journal\nSyslogIdentifier=fuckipv6\n\n[Install]\n{}\n",
            if self.layout.scope == ServiceScope::System {
                "WantedBy=multi-user.target"
            } else {
                "WantedBy=default.target"
            },
        ));
        Ok(definition)
    }

    fn render_launchd(&self) -> Result<String> {
        let environment = [
            ("FUCKIPV6_RUNTIME_DIR", path_str(&self.layout.runtime_dir)?),
            ("RUST_LOG", "info"),
        ];
        let mut environment_xml = String::new();
        for (key, value) in environment {
            environment_xml.push_str(&format!(
                "    <key>{}</key>\n    <string>{}</string>\n",
                xml_escape(key),
                xml_escape(value)
            ));
        }
        Ok(format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n  <key>Label</key>\n  <string>{LAUNCHD_LABEL}</string>\n  <key>ProgramArguments</key>\n  <array>\n    <string>{}</string>\n    <string>__service</string>\n    <string>--options-file</string>\n    <string>{}</string>\n    <string>--config-dir</string>\n    <string>{}</string>\n  </array>\n  <key>WorkingDirectory</key>\n  <string>{}</string>\n  <key>EnvironmentVariables</key>\n  <dict>\n{}  </dict>\n  <key>RunAtLoad</key>\n  <true/>\n  <key>KeepAlive</key>\n  <dict>\n    <key>SuccessfulExit</key>\n    <false/>\n  </dict>\n  <key>ProcessType</key>\n  <string>Background</string>\n  <key>ThrottleInterval</key>\n  <integer>5</integer>\n  <key>ExitTimeOut</key>\n  <integer>{STOP_TIMEOUT_SECONDS}</integer>\n  <key>StandardOutPath</key>\n  <string>{}</string>\n  <key>StandardErrorPath</key>\n  <string>{}</string>\n</dict>\n</plist>\n",
            xml_escape(path_str(&self.layout.binary)?),
            xml_escape(path_str(&self.layout.options_file())?),
            xml_escape(path_str(&self.layout.config_dir)?),
            xml_escape(path_str(&self.layout.state_dir)?),
            environment_xml,
            xml_escape(path_str(&self.layout.log_dir.join("node.log"))?),
            xml_escape(path_str(&self.layout.log_dir.join("node.err.log"))?),
        ))
    }

    fn register(&self) -> Result<()> {
        match self.layout.platform {
            ServicePlatform::Systemd => {
                self.systemctl_checked(&["daemon-reload"])?;
                self.systemctl_checked(&["enable", SYSTEMD_UNIT])?;
            }
            ServicePlatform::Launchd => {
                let _ = self.bootout_launchd(false);
                self.bootstrap_launchd()?;
            }
            ServicePlatform::Windows => {
                let bin_path = windows_bin_path(&self.layout)?;
                self.sc_checked(&[
                    "create",
                    SERVICE_NAME,
                    &format!("binPath= {bin_path}"),
                    "start=",
                    "auto",
                    "DisplayName=",
                    "fuckipv6",
                ])?;
            }
        }
        Ok(())
    }

    fn is_registered(&self) -> Result<bool> {
        match self.layout.platform {
            ServicePlatform::Systemd | ServicePlatform::Launchd => {
                Ok(self.layout.service_file.exists())
            }
            ServicePlatform::Windows => Ok(self.sc(&["query", SERVICE_NAME])?.success),
        }
    }

    fn is_active(&self) -> Result<bool> {
        match self.layout.platform {
            ServicePlatform::Systemd => {
                let result = self.systemctl(&["is-active", SYSTEMD_UNIT])?;
                Ok(result.success && result.stdout.trim() == "active")
            }
            ServicePlatform::Launchd => {
                let domain = self.launchd_domain()?;
                Ok(self
                    .run_os(
                        OsStr::new("launchctl"),
                        &[
                            OsString::from("print"),
                            OsString::from(format!("{domain}/{LAUNCHD_LABEL}")),
                        ],
                    )?
                    .success)
            }
            ServicePlatform::Windows => {
                let result = self.sc(&["query", SERVICE_NAME])?;
                Ok(result.success && result.stdout.to_ascii_uppercase().contains("RUNNING"))
            }
        }
    }

    fn start_launchd(&self) -> Result<()> {
        let domain = self.launchd_domain()?;
        let target = format!("{domain}/{LAUNCHD_LABEL}");
        let loaded = self.run_os(
            OsStr::new("launchctl"),
            &[OsString::from("print"), OsString::from(&target)],
        )?;
        if loaded.success {
            self.checked_os(
                OsStr::new("launchctl"),
                &[
                    OsString::from("kickstart"),
                    OsString::from("-k"),
                    OsString::from(target),
                ],
            )?;
        } else {
            self.bootstrap_launchd()?;
        }
        Ok(())
    }

    fn bootstrap_launchd(&self) -> Result<()> {
        let domain = self.launchd_domain()?;
        self.checked_os(
            OsStr::new("launchctl"),
            &[
                OsString::from("bootstrap"),
                OsString::from(domain),
                self.layout.service_file.as_os_str().to_owned(),
            ],
        )?;
        Ok(())
    }

    fn bootout_launchd(&self, required: bool) -> Result<()> {
        let domain = self.launchd_domain()?;
        let result = self.run_os(
            OsStr::new("launchctl"),
            &[
                OsString::from("bootout"),
                OsString::from(domain),
                self.layout.service_file.as_os_str().to_owned(),
            ],
        )?;
        if required && !result.success {
            bail!("launchctl bootout 失败: {}", result.combined_output());
        }
        Ok(())
    }

    fn launchd_domain(&self) -> Result<String> {
        Ok(match self.layout.scope {
            ServiceScope::System => "system".into(),
            ServiceScope::User => format!("user/{}", current_user_id()?),
        })
    }

    fn systemctl(&self, args: &[&str]) -> Result<CommandResult> {
        let mut arguments = Vec::with_capacity(args.len() + 1);
        if self.layout.scope == ServiceScope::User {
            arguments.push(OsString::from("--user"));
        }
        arguments.extend(args.iter().map(OsString::from));
        self.run_os(OsStr::new("systemctl"), &arguments)
    }

    fn systemctl_checked(&self, args: &[&str]) -> Result<CommandResult> {
        let result = self.systemctl(args)?;
        ensure_success("systemctl", &result)
    }

    fn sc(&self, args: &[&str]) -> Result<CommandResult> {
        self.run_os(
            OsStr::new("sc.exe"),
            &args.iter().map(OsString::from).collect::<Vec<_>>(),
        )
    }

    fn sc_checked(&self, args: &[&str]) -> Result<CommandResult> {
        let result = self.sc(args)?;
        ensure_success("sc.exe", &result)
    }

    fn run_os(&self, program: &OsStr, args: &[OsString]) -> Result<CommandResult> {
        self.runner.run(program, args)
    }

    fn checked_os(&self, program: &OsStr, args: &[OsString]) -> Result<CommandResult> {
        let result = self.run_os(program, args)?;
        ensure_success(&Path::new(program).display().to_string(), &result)
    }
}

fn ensure_success(program: &str, result: &CommandResult) -> Result<CommandResult> {
    if result.success {
        return Ok(result.clone());
    }
    let output = result.combined_output();
    if output.is_empty() {
        bail!("{program} 返回失败状态")
    }
    bail!("{program} 执行失败: {output}")
}

fn scope_name(scope: ServiceScope) -> &'static str {
    match scope {
        ServiceScope::System => "系统",
        ServiceScope::User => "用户",
    }
}

#[cfg(unix)]
fn current_user_id() -> Result<u32> {
    // SAFETY: geteuid has no preconditions and does not dereference pointers.
    Ok(unsafe { libc::geteuid() })
}

#[cfg(windows)]
fn current_user_id() -> Result<u32> {
    Ok(0)
}

fn path_str(path: &Path) -> Result<&str> {
    path.to_str().context("服务路径不是有效 UTF-8")
}

fn quote_systemd(value: &str) -> String {
    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(windows)]
fn windows_bin_path(layout: &ServiceLayout) -> Result<String> {
    Ok(format!(
        "\"{}\" __service --windows-service --options-file \"{}\" --config-dir \"{}\"",
        path_str(&layout.binary)?,
        path_str(&layout.options_file())?,
        path_str(&layout.config_dir)?
    ))
}

#[cfg(not(windows))]
fn windows_bin_path(layout: &ServiceLayout) -> Result<String> {
    Ok(format!(
        "{:?} __service --windows-service --options-file {:?} --config-dir {:?}",
        layout.binary,
        layout.options_file(),
        layout.config_dir
    ))
}

fn write_atomic(path: &Path, contents: &[u8], mode: u32) -> Result<()> {
    let parent = path.parent().context("目标路径缺少父目录")?;
    fs::create_dir_all(parent).with_context(|| format!("无法创建目录 {}", parent.display()))?;
    let temporary = parent.join(format!(
        ".{}.{}.{}.tmp",
        path.file_name()
            .and_then(OsStr::to_str)
            .unwrap_or("fuckipv6"),
        std::process::id(),
        monotonic_nonce()
    ));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(mode);
    }
    let mut file = options
        .open(&temporary)
        .with_context(|| format!("无法创建临时文件 {}", temporary.display()))?;
    if let Err(error) = (|| -> Result<()> {
        file.write_all(contents)?;
        file.sync_all()?;
        set_mode(&temporary, mode)?;
        fs::rename(&temporary, path).with_context(|| format!("无法原子替换 {}", path.display()))?;
        Ok(())
    })() {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    Ok(())
}

fn copy_atomic(source: &Path, target: &Path, mode: u32) -> Result<()> {
    let parent = target.parent().context("目标路径缺少父目录")?;
    fs::create_dir_all(parent).with_context(|| format!("无法创建目录 {}", parent.display()))?;
    let temporary = parent.join(format!(
        ".{}.{}.{}.tmp",
        target
            .file_name()
            .and_then(OsStr::to_str)
            .unwrap_or("fuckipv6"),
        std::process::id(),
        monotonic_nonce()
    ));
    fs::copy(source, &temporary)
        .with_context(|| format!("无法复制 {} 到 {}", source.display(), temporary.display()))?;
    if let Err(error) = set_mode(&temporary, mode) {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    let backup = target.with_extension(format!("previous-{}", std::process::id()));
    let had_target = target.exists();
    if had_target {
        if let Err(error) = fs::rename(target, &backup) {
            let _ = fs::remove_file(&temporary);
            return Err(error).with_context(|| format!("无法备份 {}", target.display()));
        }
    }
    if let Err(error) = fs::rename(&temporary, target) {
        let _ = fs::remove_file(&temporary);
        if had_target {
            let _ = fs::rename(&backup, target);
        }
        return Err(error).with_context(|| format!("无法安装到 {}", target.display()));
    }
    if had_target {
        let _ = fs::remove_file(backup);
    }
    Ok(())
}

fn set_mode(path: &Path, mode: u32) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode))
            .with_context(|| format!("无法设置权限 {}", path.display()))?;
    }
    #[cfg(not(unix))]
    {
        let _ = (path, mode);
    }
    Ok(())
}

fn monotonic_nonce() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Invocation {
        program: OsString,
        args: Vec<OsString>,
    }

    struct FakeRunner {
        active: Mutex<bool>,
        launchd_loaded: Mutex<bool>,
        fail_next_start: Mutex<bool>,
        fail_next_bootout: Mutex<bool>,
        invocations: Mutex<Vec<Invocation>>,
    }

    impl FakeRunner {
        fn new(active: bool) -> Self {
            Self {
                active: Mutex::new(active),
                launchd_loaded: Mutex::new(active),
                fail_next_start: Mutex::new(false),
                fail_next_bootout: Mutex::new(false),
                invocations: Mutex::new(Vec::new()),
            }
        }

        fn fail_next_start(&self) {
            *self.fail_next_start.lock().unwrap() = true;
        }

        fn fail_next_bootout(&self) {
            *self.fail_next_bootout.lock().unwrap() = true;
        }

        fn invocations(&self) -> Vec<Invocation> {
            self.invocations.lock().unwrap().clone()
        }
    }

    impl CommandRunner for FakeRunner {
        fn run(&self, program: &OsStr, args: &[OsString]) -> Result<CommandResult> {
            self.invocations.lock().unwrap().push(Invocation {
                program: program.to_owned(),
                args: args.to_vec(),
            });
            let first = args
                .first()
                .and_then(|arg| arg.to_str())
                .unwrap_or_default();
            if program == "launchctl" {
                let (output, success) = match first {
                    "print" => {
                        let loaded = *self.launchd_loaded.lock().unwrap();
                        (if loaded { "loaded\n" } else { "not found\n" }, loaded)
                    }
                    "bootstrap" | "kickstart" => {
                        *self.launchd_loaded.lock().unwrap() = true;
                        ("", true)
                    }
                    "bootout" if *self.fail_next_bootout.lock().unwrap() => {
                        *self.fail_next_bootout.lock().unwrap() = false;
                        ("simulated bootout failure\n", false)
                    }
                    "bootout" => {
                        *self.launchd_loaded.lock().unwrap() = false;
                        ("", true)
                    }
                    _ => ("", true),
                };
                return Ok(CommandResult {
                    success,
                    stdout: output.into(),
                    stderr: String::new(),
                });
            }
            let active = *self.active.lock().unwrap();
            let (output, success) = match first {
                "is-active" if active => ("active\n", true),
                "is-active" => ("inactive\n", true),
                "is-enabled" => ("enabled\n", true),
                "start" if *self.fail_next_start.lock().unwrap() => {
                    *self.fail_next_start.lock().unwrap() = false;
                    ("simulated start failure\n", false)
                }
                "start" | "restart" => {
                    *self.active.lock().unwrap() = true;
                    ("", true)
                }
                "stop" => {
                    *self.active.lock().unwrap() = false;
                    ("", true)
                }
                "status" => ("active (running)\n", true),
                _ => ("", true),
            };
            Ok(CommandResult {
                success,
                stdout: output.into(),
                stderr: String::new(),
            })
        }
    }

    fn temp_root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "fuckipv6-service-{name}-{}-{}",
            std::process::id(),
            monotonic_nonce()
        ))
    }

    fn test_layout(root: &Path) -> ServiceLayout {
        ServiceLayout {
            platform: ServicePlatform::Systemd,
            scope: ServiceScope::System,
            binary: root.join("opt/fuckipv6/fuckipv6"),
            config_dir: root.join("etc/fuckipv6"),
            state_dir: root.join("var/lib/fuckipv6"),
            log_dir: root.join("var/log/fuckipv6"),
            runtime_dir: root.join("run/fuckipv6"),
            service_file: root.join("etc/systemd/system/fuckipv6.service"),
        }
    }

    fn setup(name: &str, active: bool) -> (PathBuf, Arc<FakeRunner>, ServiceManager, PathBuf) {
        let root = temp_root(name);
        let source = root.join("source/fuckipv6");
        fs::create_dir_all(source.parent().unwrap()).unwrap();
        fs::write(&source, "binary-v1").unwrap();
        let runner = Arc::new(FakeRunner::new(active));
        let manager = ServiceManager::new(test_layout(&root), runner.clone(), source.clone());
        (root, runner, manager, source)
    }

    #[test]
    fn install_uses_service_role_and_preserves_configuration_contract() -> Result<()> {
        let (root, runner, manager, _) = setup("install", false);
        manager.install(8088)?;

        let unit = fs::read_to_string(&manager.layout.service_file)?;
        assert!(unit.contains("__service --options-file"));
        assert!(unit.contains("--config-dir"));
        assert!(unit.contains("KillMode=mixed"));
        assert!(unit.contains("TimeoutStopSec=75s"));
        assert!(!unit.contains("--headless"));
        assert_eq!(fs::read_to_string(&manager.layout.binary)?, "binary-v1");
        let options = fs::read_to_string(manager.layout.options_file())?;
        assert!(options.contains("\"webui_port\":8088"));
        assert!(options.contains("\"routing_mode\":null"));
        assert!(runner.invocations().iter().any(|invocation| {
            invocation.program == "systemctl"
                && invocation.args == vec![OsString::from("enable"), OsString::from(SYSTEMD_UNIT)]
        }));

        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn upgrade_replaces_binary_without_overwriting_options_and_restores_running_state() -> Result<()>
    {
        let (root, runner, manager, source) = setup("upgrade", false);
        manager.install(8080)?;
        let options_path = manager.layout.options_file();
        fs::write(&options_path, "operator configured values")?;
        fs::write(&source, "binary-v2")?;
        *runner.active.lock().unwrap() = true;

        manager.upgrade(9999)?;

        assert_eq!(fs::read_to_string(&manager.layout.binary)?, "binary-v2");
        assert_eq!(
            fs::read_to_string(&options_path)?,
            "operator configured values"
        );
        assert!(*runner.active.lock().unwrap());
        assert!(runner.invocations().iter().any(|invocation| {
            invocation.program == "systemctl"
                && invocation.args == vec![OsString::from("stop"), OsString::from(SYSTEMD_UNIT)]
        }));
        assert!(runner.invocations().iter().any(|invocation| {
            invocation.program == "systemctl"
                && invocation.args == vec![OsString::from("start"), OsString::from(SYSTEMD_UNIT)]
        }));

        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn failed_upgrade_restores_previous_binary() -> Result<()> {
        let (root, runner, manager, source) = setup("upgrade-rollback", false);
        manager.install(8080)?;
        fs::write(&source, "binary-v2")?;
        *runner.active.lock().unwrap() = true;
        runner.fail_next_start();

        assert!(manager.upgrade(8080).is_err());
        assert_eq!(fs::read_to_string(&manager.layout.binary)?, "binary-v1");
        assert!(
            fs::read_dir(manager.layout.binary.parent().unwrap())?.all(|entry| {
                !entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .contains("upgrade-backup")
            })
        );

        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn uninstall_preserves_configuration_and_identity_data() -> Result<()> {
        let (root, _, manager, _) = setup("uninstall", false);
        manager.install(8080)?;
        fs::write(
            manager.layout.config_dir.join("node_identity.pem"),
            "identity",
        )?;

        manager.uninstall()?;

        assert!(!manager.layout.binary.exists());
        assert!(!manager.layout.service_file.exists());
        assert_eq!(
            fs::read_to_string(manager.layout.config_dir.join("node_identity.pem"))?,
            "identity"
        );
        assert!(manager.layout.options_file().exists());

        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn launchd_can_restart_after_stop_and_failed_stop_blocks_uninstall() -> Result<()> {
        let (root, runner, _, source) = setup("launchd-lifecycle", false);
        let mut layout = test_layout(&root);
        layout.platform = ServicePlatform::Launchd;
        layout.service_file = root.join("Library/LaunchDaemons/cc.xlei.fuckipv6.plist");
        let manager = ServiceManager::new(layout, runner.clone(), source);

        manager.install(8080)?;
        assert!(*runner.launchd_loaded.lock().unwrap());

        manager.stop()?;
        assert!(!*runner.launchd_loaded.lock().unwrap());
        manager.start()?;
        assert!(*runner.launchd_loaded.lock().unwrap());

        runner.fail_next_bootout();
        assert!(manager.uninstall().is_err());
        assert!(manager.layout.binary.exists());
        assert!(manager.layout.service_file.exists());
        assert!(*runner.launchd_loaded.lock().unwrap());

        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn launchd_and_windows_definitions_invoke_the_service_role() -> Result<()> {
        let root = temp_root("render");
        let mut layout = test_layout(&root);
        layout.platform = ServicePlatform::Launchd;
        let manager = ServiceManager::new(
            layout.clone(),
            Arc::new(FakeRunner::new(false)),
            root.join("source"),
        );
        let launchd = manager.render_launchd()?;
        assert!(launchd.contains("<string>__service</string>"));
        assert!(launchd.contains("--options-file"));
        assert!(launchd.contains("--config-dir"));
        assert!(launchd.contains("<key>ExitTimeOut</key>"));

        layout.platform = ServicePlatform::Windows;
        let windows = windows_bin_path(&layout)?;
        assert!(windows.contains("__service --windows-service --options-file"));
        assert!(windows.contains("--config-dir"));
        fs::remove_dir_all(root).ok();
        Ok(())
    }
}
