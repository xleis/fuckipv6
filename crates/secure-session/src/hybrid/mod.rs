//! Experimental hybrid post-quantum building blocks.
//!
//! These modules implement the cryptographic constructions required by the
//! production-candidate v2 suites without changing the currently deployed
//! classic wire profile. The upstream ML-KEM and ML-DSA implementations have
//! not been independently audited, so this module must not be presented as a
//! production post-quantum security claim.

pub mod driver;
pub mod handshake;
pub mod identity;
pub mod profile;
pub mod wire;
pub mod xwing;

pub use driver::{HybridDriverError, HybridInitiatorHandshake, HybridResponderHandshake};
pub use handshake::{
    HybridAccept, HybridAcceptTranscript, HybridAccepted, HybridFinished, HybridHandshakeError,
    HybridHello, HybridInitiator, HybridResponder, HYBRID_ACCEPT_LEN, HYBRID_COOKIE_LEN,
    HYBRID_HELLO_LEN, HYBRID_NONCE_LEN, HYBRID_PROFILE_ID, MAX_CONTROL_FRAME_LEN,
};
pub use identity::{
    CompositeIdentity, CompositePublicKey, CompositeSignature, IdentityError, IdentityId,
    COMPOSITE_IDENTITY_PUBLIC_KEY_LEN, COMPOSITE_SIGNATURE_LEN, IDENTITY_ID_LEN,
};
pub use profile::{
    DataAead, HandshakeKdf, HybridProfilePolicy, HybridSuite, KemAlgorithm,
    NegotiatedHybridProfile, ProfileNegotiationError, SignatureAlgorithm, XWING_DRAFT_VERSION,
};
pub use wire::{
    decode_control, decode_control_for_profile, HybridConfirm, HybridConfirmError,
    HybridControlCodecError, HybridControlKind, HybridControlMessage, HYBRID_ACCEPT_BODY_LEN,
    HYBRID_CONFIRM_BODY_LEN, HYBRID_CONTROL_HEADER_LEN, HYBRID_CONTROL_MAGIC,
    HYBRID_HELLO_BODY_LEN, MAX_HYBRID_CONTROL_FRAME_LEN,
};
pub use xwing::{
    XWingCiphertext, XWingDecapsulationKey, XWingEncapsulationKey, XWingError, XWingSharedSecret,
    XWING_CIPHERTEXT_LEN, XWING_ENCAPSULATION_KEY_LEN, XWING_SEED_LEN, XWING_SHARED_SECRET_LEN,
};
