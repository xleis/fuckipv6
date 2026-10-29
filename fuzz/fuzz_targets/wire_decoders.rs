#![no_main]

use fuckipv6_secure_session::confirmation::ConfirmationTag;
use fuckipv6_secure_session::cookie::{decode_cookie_challenge, Cookie};
use fuckipv6_secure_session::pq::{
    MlDsa65Signature, MlDsa65VerifyingKey, MlKem768Ciphertext, MlKem768PublicKey,
};
use fuckipv6_secure_session::rekey::{RekeyAccept, RekeyConfirm, RekeyInit};
use fuckipv6_secure_session::wire::decode_control;
use fuckipv6_secure_session::DataHeader;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = DataHeader::decode(data);
    let _ = Cookie::decode(data);
    let _ = decode_cookie_challenge(data);
    let _ = RekeyInit::decode(data);
    let _ = RekeyAccept::decode(data);
    let _ = RekeyConfirm::decode(data);
    let _ = decode_control(data);
    let _ = ConfirmationTag::from_slice(data);
    let _ = MlKem768PublicKey::decode(data);
    let _ = MlKem768Ciphertext::decode(data);
    let _ = MlDsa65VerifyingKey::decode(data);
    let _ = MlDsa65Signature::decode(data);
});
