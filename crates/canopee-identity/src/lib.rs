mod device;
pub mod envelope;
mod identity;
pub mod pairing;
pub use device::DeviceKey;
pub use envelope::{EnvelopeError, is_envelope};
pub(crate) use identity::dh_secret_for;
pub use identity::{Identity, IdentityId};

/*
✅ create identity
✅ load identity
✅ save identity
✅ sign data
✅ verify signatures
 */
