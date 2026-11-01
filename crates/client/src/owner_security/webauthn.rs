use anyhow::{Context, Result};
use fuckipv6_common::domain::NodeId;
use sha2::{Digest, Sha256};
use webauthn_rs::prelude::{
    CreationChallengeResponse, Passkey, PasskeyRegistration, RegisterPublicKeyCredential, Url,
    Uuid, Webauthn, WebauthnBuilder,
};

const LOCAL_RP_ID: &str = "localhost";

pub(crate) struct OwnerWebauthn {
    webauthn: Webauthn,
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
        Ok(Self { webauthn, origin })
    }

    pub(crate) fn origin(&self) -> &str {
        &self.origin
    }

    pub(crate) fn start(
        &self,
        node_id: &NodeId,
        display_name: &str,
    ) -> Result<(CreationChallengeResponse, PasskeyRegistration)> {
        let digest = Sha256::digest(node_id.as_str().as_bytes());
        let mut user_id = [0u8; 16];
        user_id.copy_from_slice(&digest[..16]);
        self.webauthn
            .start_passkey_registration(
                Uuid::from_bytes(user_id),
                node_id.as_str(),
                display_name,
                None,
            )
            .context("无法开始 Passkey 注册")
    }

    pub(crate) fn finish(
        &self,
        response: &RegisterPublicKeyCredential,
        state: &PasskeyRegistration,
    ) -> Result<Passkey> {
        self.webauthn
            .finish_passkey_registration(response, state)
            .context("Passkey 注册验证失败")
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
        let (challenge, _state) = owner.start(&node_id, "Owner")?;
        assert_eq!(challenge.public_key.rp.id, "localhost");
        assert!(!challenge.public_key.challenge.is_empty());
        assert_eq!(challenge.public_key.user.id.len(), 16);
        assert_eq!(challenge.public_key.user.name, node_id.as_str());
        Ok(())
    }

    #[test]
    fn port_zero_is_rejected() {
        assert!(OwnerWebauthn::new(0).is_err());
    }
}
