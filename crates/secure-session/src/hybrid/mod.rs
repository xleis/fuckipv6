//! Experimental hybrid post-quantum building blocks.
//!
//! These modules implement the cryptographic constructions required by the
//! production-candidate v2 suites without changing the currently deployed
//! classic wire profile. The upstream ML-KEM and ML-DSA implementations have
//! not been independently audited, so this module must not be presented as a
//! production post-quantum security claim.

pub mod identity;
pub mod xwing;

pub use identity::{
    CompositeIdentity, CompositePublicKey, CompositeSignature, IdentityError, IdentityId,
    COMPOSITE_IDENTITY_PUBLIC_KEY_LEN, COMPOSITE_SIGNATURE_LEN, IDENTITY_ID_LEN,
};
pub use xwing::{
    XWingCiphertext, XWingDecapsulationKey, XWingEncapsulationKey, XWingError, XWingSharedSecret,
    XWING_CIPHERTEXT_LEN, XWING_ENCAPSULATION_KEY_LEN, XWING_SEED_LEN, XWING_SHARED_SECRET_LEN,
};
