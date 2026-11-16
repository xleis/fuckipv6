//! Desktop shell for the browser surface.
//!
//! The node keeps its privileged runtime in `fuckipv6 --webview-shell`; this
//! process only owns the native window and renders the same WebUI bundle the
//! browser uses. Because WebKitGTK cannot run the owner's WebAuthn ceremony,
//! the shell redeems the single-use bootstrap token the node writes into its
//! runtime directory and keeps the resulting owner session cookie in this
//! webview only.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::time::Duration;

use tauri::webview::cookie::Cookie;
use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

const DEFAULT_RUNTIME_DIR: &str = "/run/fuckipv6-gpui-root";
const TOKEN_FILE: &str = "webview-shell.token";

fn token_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
        if !dir.trim().is_empty() {
            candidates.push(PathBuf::from(dir).join(TOKEN_FILE));
        }
    }
    if let Ok(dir) = std::env::var("FUCKIPV6_RUNTIME_DIR") {
        if !dir.trim().is_empty() {
            candidates.push(PathBuf::from(dir).join(TOKEN_FILE));
        }
    }
    candidates.push(PathBuf::from(DEFAULT_RUNTIME_DIR).join(TOKEN_FILE));
    candidates
}

fn read_shell_token() -> Option<String> {
    for path in token_candidates() {
        if let Ok(token) = std::fs::read_to_string(&path) {
            let token = token.trim().to_string();
            if !token.is_empty() {
                return Some(token);
            }
        }
    }
    None
}

/// Minimal HTTP/1.1 POST: the shell only ever talks to the loopback node, so a
/// full HTTP client would add a dependency without adding capability.
fn redeem_session(host: &str, port: u16, token: &str) -> Result<String, String> {
    let address = format!("{host}:{port}");
    let mut stream = TcpStream::connect(&address).map_err(|error| error.to_string())?;
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .map_err(|error| error.to_string())?;
    let body = format!("{{\"token\":\"{token}\"}}");
    let request = format!(
        "POST /api/auth/local-session HTTP/1.1\r\nHost: {address}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    stream
        .write_all(request.as_bytes())
        .map_err(|error| error.to_string())?;
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .map_err(|error| error.to_string())?;
    let status = response.lines().next().unwrap_or_default().to_string();
    if !status.contains(" 200 ") {
        return Err(status);
    }
    for line in response.lines() {
        let lower = line.to_ascii_lowercase();
        if let Some(value) = lower.strip_prefix("set-cookie:") {
            let offset = line.len() - value.len();
            return Ok(line[offset..].trim().to_string());
        }
    }
    Err("missing Set-Cookie in local-session response".to_string())
}

fn main() {
    let url = std::env::var("FUCKIPV6_WEBUI_URL")
        .unwrap_or_else(|_| "http://localhost:8090/".to_string());
    let parsed = url.parse::<tauri::Url>().expect("valid FUCKIPV6_WEBUI_URL");
    let host = parsed.host_str().unwrap_or("localhost").to_string();
    let port = parsed.port_or_known_default().unwrap_or(8090);
    let origin = format!("{scheme}://{host}:{port}/", scheme = parsed.scheme());

    tauri::Builder::default()
        .setup(move |app| {
            let window = WebviewWindowBuilder::new(app, "main", WebviewUrl::External(parsed.clone()))
                .title("fuckipv6")
                .inner_size(1440.0, 900.0)
                .min_inner_size(960.0, 640.0)
                .build()?;

            match read_shell_token() {
                Some(token) => match redeem_session(&host, port, &token) {
                    Ok(set_cookie) => match Cookie::parse(set_cookie) {
                        Ok(mut cookie) => {
                            cookie.set_domain(host.clone());
                            if let Err(error) = window.set_cookie(cookie) {
                                eprintln!("[shell] could not install owner session: {error}");
                            } else {
                                eprintln!("[shell] local owner session established");
                            }
                        }
                        Err(error) => eprintln!("[shell] unparsable session cookie: {error}"),
                    },
                    Err(error) => eprintln!("[shell] local session rejected: {error}"),
                },
                None => eprintln!(
                    "[shell] no bootstrap token in {:?}; the owner gate will ask for a passkey",
                    token_candidates()
                ),
            }

            window.navigate(origin.parse()?)?;
            window.set_focus()?;
            app.manage(());
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running the fuckipv6 desktop shell");
}
