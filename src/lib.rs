//! Resource caching and publication services.
//!
//! The ownership foundation provides retained backing, scoped views, caller-driven
//! cleanup, declared allocation accounting and independent family policy measures.
//! Operations are synchronous; no executor or application-specific types are needed.
//! Keyed caching, production and publication remain planned capabilities.
//! See `pubdocs/README.md` and `examples/foundation.rs` for standalone use.

#![deny(unsafe_op_in_unsafe_fn)]

mod accounting;
mod cache;
mod demand;
mod dependency;
mod lifecycle;
mod ownership;
mod policy;
mod pool;
mod production;
mod progress;
mod publication;
mod range;
mod storage;

pub use accounting::{
    SCAccountingDomain, SCAccountingError, SCAccountingSnapshot, SCAllocationCharge,
    SCAllocationClass, SCChargeTransferFailure,
};
pub use ownership::{SCAcquisitionError, SCBacking, SCCleanupContext, SCView};
pub use policy::{
    SC_REFERENCE_PAYLOAD_RETENTION, SC_REFERENCE_PRODUCTION_LIMIT, SC_REFERENCE_PROVIDER_SLOTS,
    SC_REFERENCE_RANGE_ENTRIES, SCByteLimit, SCCountLimit, SCPolicyCounter, SCPolicyCounterError,
};
