//! Public owner-claim exposure policy.
//!
//! This module accepts public bootstrap traffic only when it arrives through a
//! configured TLS-terminating reverse proxy. The security decision binds the
//! TCP peer to an explicit CIDR allowlist and requires an internally consistent
//! external host, HTTPS scheme, origin, and client address. Ambiguous forwarding
//! headers fail closed instead of being interpreted.

use super::tls::ENV_WEBUI_TLS_CERT;
use axum::http::{header, HeaderMap, Method, StatusCode};
use ipnet::IpNet;
use std::net::IpAddr;
use std::str::FromStr;
use webauthn_rs::prelude::Url;

pub(super) const ENV_WEBUI_PUBLIC_ORIGIN: &str = "FUCKIPV6_WEBUI_PUBLIC_ORIGIN";
pub(super) const ENV_WEBUI_TRUSTED_PROXY_CIDRS: &str = "FUCKIPV6_WEBUI_TRUSTED_PROXY_CIDRS";

#[derive(Clone, Debug)]
enum PublicClaimExposure {
    ReverseProxy(Vec<IpNet>),
    DirectTls,
}

#[derive(Clone, Debug)]
pub(super) struct PublicClaimConfig {
    origin: Url,
    origin_string: String,
    authority: String,
    exposure: PublicClaimExposure,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PublicClaimRejection {
    BadRequest(&'static str),
    Forbidden(&'static str),
}

impl PublicClaimRejection {
    pub(super) fn status(self) -> StatusCode {
        match self {
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::Forbidden(_) => StatusCode::FORBIDDEN,
        }
    }

    pub(super) fn message(self) -> &'static str {
        match self {
            Self::BadRequest(message) | Self::Forbidden(message) => message,
        }
    }
}

impl PublicClaimConfig {
    pub(super) fn from_process_env(direct_tls: bool) -> anyhow::Result<Option<Self>> {
        let origin = optional_env(ENV_WEBUI_PUBLIC_ORIGIN)?;
        let trusted_proxies = optional_env(ENV_WEBUI_TRUSTED_PROXY_CIDRS)?;
        if direct_tls {
            let Some(origin) = origin else {
                anyhow::bail!("{ENV_WEBUI_TLS_CERT} 已启用，但 {ENV_WEBUI_PUBLIC_ORIGIN} 未设置");
            };
            if trusted_proxies.is_some() {
                anyhow::bail!(
                    "直接 TLS 与 {ENV_WEBUI_TRUSTED_PROXY_CIDRS} 不能同时设置，传输边界必须唯一"
                );
            }
            return Self::new_direct_tls(&origin).map(Some);
        }
        match (origin, trusted_proxies) {
            (None, None) => Ok(None),
            (None, Some(_)) => anyhow::bail!(
                "{ENV_WEBUI_TRUSTED_PROXY_CIDRS} 已设置，但 {ENV_WEBUI_PUBLIC_ORIGIN} 未设置"
            ),
            (Some(_), None) => anyhow::bail!(
                "{ENV_WEBUI_PUBLIC_ORIGIN} 已设置，但 {ENV_WEBUI_TRUSTED_PROXY_CIDRS} 未设置"
            ),
            (Some(origin), Some(proxies)) => Self::new(&origin, &proxies).map(Some),
        }
    }

    pub(super) fn new(origin: &str, trusted_proxies: &str) -> anyhow::Result<Self> {
        let (origin, origin_string, authority) = parse_origin(origin)?;
        let trusted_proxies = trusted_proxies
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| {
                let network = IpNet::from_str(value)
                    .map_err(|_| anyhow::anyhow!("受信代理 CIDR 无效: {value}"))?;
                if network.prefix_len() == 0 {
                    anyhow::bail!("受信代理 CIDR 不能覆盖整个地址族: {value}");
                }
                Ok(network)
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        if trusted_proxies.is_empty() {
            anyhow::bail!("至少需要一个受信反向代理 CIDR");
        }

        Ok(Self {
            origin,
            origin_string,
            authority,
            exposure: PublicClaimExposure::ReverseProxy(trusted_proxies),
        })
    }

    pub(super) fn new_direct_tls(origin: &str) -> anyhow::Result<Self> {
        let (origin, origin_string, authority) = parse_origin(origin)?;
        Ok(Self {
            origin,
            origin_string,
            authority,
            exposure: PublicClaimExposure::DirectTls,
        })
    }

    pub(super) fn origin_string(&self) -> &str {
        &self.origin_string
    }

    pub(super) fn origin(&self) -> &Url {
        &self.origin
    }

    pub(super) fn host(&self) -> &str {
        self.origin
            .host_str()
            .expect("validated public claim origin")
    }

    pub(super) fn direct_tls(&self) -> bool {
        matches!(self.exposure, PublicClaimExposure::DirectTls)
    }

    pub(super) fn authorize(
        &self,
        peer: IpAddr,
        method: &Method,
        headers: &HeaderMap,
    ) -> Result<IpAddr, PublicClaimRejection> {
        match &self.exposure {
            PublicClaimExposure::ReverseProxy(trusted_proxies) => {
                if !trusted_proxies
                    .iter()
                    .any(|network| network.contains(&peer))
                {
                    return Err(PublicClaimRejection::Forbidden(
                        "公网认领仅允许受信反向代理访问",
                    ));
                }

                if headers.contains_key("forwarded") || headers.contains_key("x-real-ip") {
                    return Err(PublicClaimRejection::BadRequest(
                        "检测到歧义的代理转发头，拒绝公网认领",
                    ));
                }

                let forwarded_proto = single_header(headers, "x-forwarded-proto")?;
                if !forwarded_proto.eq_ignore_ascii_case("https") {
                    return Err(PublicClaimRejection::Forbidden(
                        "公网认领必须由受信代理声明原始请求为 HTTPS",
                    ));
                }

                let forwarded_host = single_header(headers, "x-forwarded-host")?;
                if !forwarded_host.eq_ignore_ascii_case(&self.authority) {
                    return Err(PublicClaimRejection::Forbidden(
                        "公网 Host 与配置的 HTTPS origin 不匹配",
                    ));
                }
            }
            PublicClaimExposure::DirectTls => {
                if [
                    "forwarded",
                    "x-forwarded-for",
                    "x-forwarded-host",
                    "x-forwarded-proto",
                    "x-real-ip",
                ]
                .iter()
                .any(|name| headers.contains_key(*name))
                {
                    return Err(PublicClaimRejection::BadRequest(
                        "直接 HTTPS 模式拒绝代理转发头",
                    ));
                }
                if peer.is_unspecified() {
                    return Err(PublicClaimRejection::Forbidden(
                        "直接 HTTPS 请求缺少有效客户端地址",
                    ));
                }
            }
        }

        let host = single_header(headers, header::HOST.as_str())?;
        if !host.eq_ignore_ascii_case(&self.authority) {
            return Err(PublicClaimRejection::Forbidden(
                "请求 Host 与配置的 HTTPS origin 不匹配",
            ));
        }

        let client_ip = match &self.exposure {
            PublicClaimExposure::ReverseProxy(_) => {
                let forwarded_for = single_header(headers, "x-forwarded-for")?;
                if forwarded_for.contains(',') {
                    return Err(PublicClaimRejection::BadRequest(
                        "X-Forwarded-For 必须是受信代理覆盖后的单个客户端 IP",
                    ));
                }
                IpAddr::from_str(forwarded_for).map_err(|_| {
                    PublicClaimRejection::BadRequest(
                        "X-Forwarded-For 必须是受信代理覆盖后的单个客户端 IP",
                    )
                })?
            }
            PublicClaimExposure::DirectTls => peer,
        };

        if matches!(method, &Method::GET | &Method::HEAD | &Method::OPTIONS) {
            return Ok(client_ip);
        }

        let request_origin = single_header(headers, header::ORIGIN.as_str())?;
        let request_origin = Url::parse(request_origin)
            .map_err(|_| PublicClaimRejection::Forbidden("公网认领请求缺少有效的 Origin"))?;
        if request_origin.origin().ascii_serialization() != self.origin_string {
            return Err(PublicClaimRejection::Forbidden(
                "公网认领请求 Origin 与配置的 HTTPS origin 不匹配",
            ));
        }
        if headers
            .get("sec-fetch-site")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| !value.eq_ignore_ascii_case("same-origin"))
        {
            return Err(PublicClaimRejection::Forbidden("公网认领拒绝跨站请求"));
        }
        Ok(client_ip)
    }
}

fn optional_env(name: &str) -> anyhow::Result<Option<String>> {
    match std::env::var(name) {
        Ok(value) if value.trim().is_empty() => anyhow::bail!("{name} 不能为空"),
        Ok(value) => Ok(Some(value)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => anyhow::bail!("{name} 必须是有效的 UTF-8"),
    }
}

fn parse_origin(origin: &str) -> anyhow::Result<(Url, String, String)> {
    let origin =
        Url::parse(origin).map_err(|error| anyhow::anyhow!("公网 origin 无效: {error}"))?;
    if origin.scheme() != "https" {
        anyhow::bail!("公网 origin 必须使用 https");
    }
    if !origin.username().is_empty() || origin.password().is_some() {
        anyhow::bail!("公网 origin 不能包含用户信息");
    }
    if origin.query().is_some() || origin.fragment().is_some() {
        anyhow::bail!("公网 origin 不能包含 query 或 fragment");
    }
    if origin.path() != "/" {
        anyhow::bail!("公网 origin 不能包含路径");
    }
    let host = origin
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("公网 origin 缺少主机名"))?;
    if host.eq_ignore_ascii_case("localhost") || IpAddr::from_str(host).is_ok() {
        anyhow::bail!("公网 origin 必须使用可验证的 DNS 主机名");
    }
    let origin_string = origin.origin().ascii_serialization();
    let authority = authority_for(&origin, host)?;
    Ok((origin, origin_string, authority))
}

fn authority_for(origin: &Url, host: &str) -> anyhow::Result<String> {
    let port = origin.port().filter(|port| *port != 443);
    Ok(match port {
        Some(port) => format!("{host}:{port}"),
        None => host.to_string(),
    })
}

fn single_header<'a>(
    headers: &'a HeaderMap,
    name: &'static str,
) -> Result<&'a str, PublicClaimRejection> {
    let mut values = headers.get_all(name).iter();
    let Some(value) = values.next() else {
        return Err(PublicClaimRejection::BadRequest(
            "受信 HTTPS 代理缺少必要的转发头",
        ));
    };
    if values.next().is_some() {
        return Err(PublicClaimRejection::BadRequest(
            "代理转发头存在重复值，拒绝公网认领",
        ));
    }
    let value = value
        .to_str()
        .map_err(|_| PublicClaimRejection::BadRequest("代理转发头不是有效 UTF-8"))?
        .trim();
    if value.is_empty() || value.contains('\r') || value.contains('\n') {
        return Err(PublicClaimRejection::BadRequest("代理转发头为空或格式无效"));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> PublicClaimConfig {
        PublicClaimConfig::new("https://relay.xlei.cc", "127.0.0.1/32,::1/128")
            .expect("valid public claim config")
    }

    fn headers() -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, "relay.xlei.cc".parse().unwrap());
        headers.insert("x-forwarded-proto", "https".parse().unwrap());
        headers.insert("x-forwarded-host", "relay.xlei.cc".parse().unwrap());
        headers.insert("x-forwarded-for", "203.0.113.9".parse().unwrap());
        headers.insert(header::ORIGIN, "https://relay.xlei.cc".parse().unwrap());
        headers
    }

    #[test]
    fn parses_strict_public_origin_and_proxy_allowlist() {
        let config = config();
        assert_eq!(config.origin_string(), "https://relay.xlei.cc");
        assert_eq!(config.authority, "relay.xlei.cc");
        assert_eq!(
            config
                .authorize("127.0.0.1".parse().unwrap(), &Method::POST, &headers())
                .expect("authorized public claim"),
            "203.0.113.9".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn rejects_plaintext_ambiguous_and_untrusted_proxy_requests() {
        let config = config();
        assert!(config
            .authorize("192.0.2.1".parse().unwrap(), &Method::GET, &headers())
            .is_err());

        let mut plaintext = headers();
        plaintext.insert("x-forwarded-proto", "http".parse().unwrap());
        assert_eq!(
            config.authorize("127.0.0.1".parse().unwrap(), &Method::POST, &plaintext),
            Err(PublicClaimRejection::Forbidden(
                "公网认领必须由受信代理声明原始请求为 HTTPS"
            ))
        );

        let mut forwarded = headers();
        forwarded.insert("forwarded", "proto=http;host=evil.example".parse().unwrap());
        assert!(matches!(
            config.authorize("127.0.0.1".parse().unwrap(), &Method::POST, &forwarded),
            Err(PublicClaimRejection::BadRequest(_))
        ));
    }

    #[test]
    fn rejects_host_origin_and_forwarded_for_injection() {
        let config = config();

        let mut host = headers();
        host.insert(header::HOST, "evil.example".parse().unwrap());
        assert!(config
            .authorize("127.0.0.1".parse().unwrap(), &Method::POST, &host)
            .is_err());

        let mut origin = headers();
        origin.insert(header::ORIGIN, "https://evil.example".parse().unwrap());
        assert!(config
            .authorize("127.0.0.1".parse().unwrap(), &Method::POST, &origin)
            .is_err());

        let mut forwarded_for = headers();
        forwarded_for.insert(
            "x-forwarded-for",
            "203.0.113.9, 198.51.100.7".parse().unwrap(),
        );
        assert!(config
            .authorize("127.0.0.1".parse().unwrap(), &Method::POST, &forwarded_for)
            .is_err());

        let mut cross_site = headers();
        cross_site.insert("sec-fetch-site", "cross-site".parse().unwrap());
        assert!(config
            .authorize("127.0.0.1".parse().unwrap(), &Method::POST, &cross_site)
            .is_err());
    }

    #[test]
    fn direct_tls_uses_peer_address_and_rejects_proxy_headers() {
        let config = PublicClaimConfig::new_direct_tls("https://relay.xlei.cc")
            .expect("valid direct TLS claim config");
        let peer: IpAddr = "203.0.113.9".parse().unwrap();
        let mut direct = HeaderMap::new();
        direct.insert(header::HOST, "relay.xlei.cc".parse().unwrap());
        direct.insert(header::ORIGIN, "https://relay.xlei.cc".parse().unwrap());
        direct.insert("sec-fetch-site", "same-origin".parse().unwrap());

        assert_eq!(
            config
                .authorize(peer, &Method::POST, &direct)
                .expect("direct TLS claim authorization"),
            peer
        );

        let mut proxied = direct.clone();
        proxied.insert("x-forwarded-for", "198.51.100.7".parse().unwrap());
        assert_eq!(
            config.authorize(peer, &Method::POST, &proxied),
            Err(PublicClaimRejection::BadRequest(
                "直接 HTTPS 模式拒绝代理转发头"
            ))
        );

        let mut wrong_origin = direct.clone();
        wrong_origin.insert(header::ORIGIN, "https://evil.example".parse().unwrap());
        assert!(matches!(
            config.authorize(peer, &Method::POST, &wrong_origin),
            Err(PublicClaimRejection::Forbidden(_))
        ));

        let mut cross_site = direct;
        cross_site.insert("sec-fetch-site", "cross-site".parse().unwrap());
        assert!(matches!(
            config.authorize(peer, &Method::POST, &cross_site),
            Err(PublicClaimRejection::Forbidden(_))
        ));
    }

    #[test]
    fn rejects_unsafe_origins_and_unbounded_proxy_networks() {
        for origin in [
            "http://relay.xlei.cc",
            "https://localhost",
            "https://127.0.0.1",
            "https://relay.xlei.cc/path",
            "https://relay.xlei.cc/?code=secret",
            "https://user@relay.xlei.cc",
        ] {
            assert!(
                PublicClaimConfig::new(origin, "127.0.0.1/32").is_err(),
                "origin should be rejected: {origin}"
            );
        }
        assert!(PublicClaimConfig::new("https://relay.xlei.cc", "0.0.0.0/0").is_err());
        assert!(PublicClaimConfig::new("https://relay.xlei.cc", "").is_err());
    }
}
