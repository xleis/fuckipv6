use anyhow::{Context, Result};
use fuckipv6_common::domain::NodeId;
use sha2::{Digest, Sha256};
use webauthn_rs::prelude::{
    CreationChallengeResponse, Passkey, PasskeyAuthentication, PasskeyRegistration,
    PublicKeyCredential, RegisterPublicKeyCredential, RequestChallengeResponse, Url, Uuid,
    Webauthn, WebauthnBuilder,
};

const LOCAL_RP_ID: &str = "localhost";

pub(crate) struct OwnerWebauthn {
    webauthn: Webauthn,
    rp_id: String,
    origin: String,
}

impl OwnerWebauthn {
    pub(crate) fn new(port: u16) -> Result<Self> {
        if port == 0 {
            anyhow::bail!("WebAuthn 需要已绑定的本地 WebUI 端口");
        }
        let origin = format!("http://{LOCAL_RP_ID}:{port}");
        let origin_url = Url::parse(&origin).context("本地 WebAuthn origin 无效")?;
        let webauthn = WebauthnBuilder::new(LOCAL_RP_ID, &origin_url)
            .context("无法构造本地 WebAuthn 配置")?
            .build()
            .context("无法初始化本地 WebAuthn")?;
        Ok(Self {
            webauthn,
            rp_id: LOCAL_RP_ID.to_string(),
            origin,
        })
    }

    pub(crate) fn public(origin: &Url) -> Result<Self> {
        if origin.scheme() != "https" {
            anyhow::bail!("公网 WebAuthn origin 必须使用 https");
        }
        let host = origin
            .host_str()
            .ok_or_else(|| anyhow::anyhow!("公网 WebAuthn origin 缺少主机名"))?;
        if host.eq_ignore_ascii_case(LOCAL_RP_ID) {
            anyhow::bail!("公网 WebAuthn 不能使用 localhost 作为 RP ID");
        }
        let origin = origin.origin().ascii_serialization();
        let origin_url = Url::parse(&origin).context("公网 WebAuthn origin 无效")?;
        let webauthn = WebauthnBuilder::new(host, &origin_url)
            .context("无法构造公网 WebAuthn 配置")?
            .build()
            .context("无法初始化公网 WebAuthn")?;
        Ok(Self {
            webauthn,
            rp_id: host.to_string(),
            origin,
        })
    }

    pub(crate) fn origin(&self) -> &str {
        &self.origin
    }

    pub(crate) fn rp_id(&self) -> &str {
        &self.rp_id
    }

    pub(crate) fn start_registration(
        &self,
        node_id: &NodeId,
        display_name: &str,
        excluded_credentials: &[Passkey],
    ) -> Result<(CreationChallengeResponse, PasskeyRegistration)> {
        let digest = Sha256::digest(node_id.as_str().as_bytes());
        let mut user_id = [0u8; 16];
        user_id.copy_from_slice(&digest[..16]);
        let excluded_credentials = (!excluded_credentials.is_empty()).then(|| {
            excluded_credentials
                .iter()
                .map(|credential| credential.cred_id().to_vec())
                .collect()
        });
        self.webauthn
            .start_passkey_registration(
                Uuid::from_bytes(user_id),
                node_id.as_str(),
                display_name,
                excluded_credentials,
            )
            .context("无法开始 Passkey 注册")
    }

    pub(crate) fn finish_registration(
        &self,
        response: &RegisterPublicKeyCredential,
        state: &PasskeyRegistration,
    ) -> Result<Passkey> {
        self.webauthn
            .finish_passkey_registration(response, state)
            .context("Passkey 注册验证失败")
    }

    pub(crate) fn start_authentication(
        &self,
        credentials: &[Passkey],
    ) -> Result<(RequestChallengeResponse, PasskeyAuthentication)> {
        if credentials.is_empty() {
            anyhow::bail!("没有可用的 Passkey");
        }
        self.webauthn
            .start_passkey_authentication(credentials)
            .context("无法开始 Passkey 认证")
    }

    pub(crate) fn finish_authentication(
        &self,
        response: &PublicKeyCredential,
        state: &PasskeyAuthentication,
    ) -> Result<webauthn_rs::prelude::AuthenticationResult> {
        self.webauthn
            .finish_passkey_authentication(response, state)
            .context("Passkey 认证验证失败")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_origin_and_registration_challenge_are_stable() -> Result<()> {
        let owner = OwnerWebauthn::new(8080)?;
        assert_eq!(owner.origin(), "http://localhost:8080");
        let node_id = NodeId::from_public_key(b"owner-webauthn-test");
        let (challenge, _state) = owner.start_registration(&node_id, "Owner", &[])?;
        assert_eq!(challenge.public_key.rp.id, "localhost");
        assert!(!challenge.public_key.challenge.is_empty());
        assert_eq!(challenge.public_key.user.id.len(), 16);
        assert_eq!(challenge.public_key.user.name, node_id.as_str());
        Ok(())
    }

    #[test]
    fn public_origin_binds_rp_id_to_configured_https_host() -> Result<()> {
        let origin = Url::parse("https://relay.xlei.cc")?;
        let owner = OwnerWebauthn::public(&origin)?;
        assert_eq!(owner.origin(), "https://relay.xlei.cc");
        let node_id = NodeId::from_public_key(b"owner-public-webauthn-test");
        let (challenge, _state) = owner.start_registration(&node_id, "Owner", &[])?;
        assert_eq!(challenge.public_key.rp.id, "relay.xlei.cc");
        Ok(())
    }

    #[test]
    fn public_origin_rejects_plaintext_and_localhost() {
        assert!(OwnerWebauthn::public(&Url::parse("http://relay.xlei.cc").unwrap()).is_err());
        assert!(OwnerWebauthn::public(&Url::parse("https://localhost").unwrap()).is_err());
    }

    #[test]
    fn port_zero_is_rejected() {
        assert!(OwnerWebauthn::new(0).is_err());
    }
}
