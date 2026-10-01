//! Resource caching and publication services.
//!
//! Keyed storage provides cache-scoped identity, retained backing, scoped views
//! and bounded idle collection. Caller-driven cleanup, declared allocation
//! accounting and independent family policy measures preserve ownership costs.
//! Operations are synchronous; no executor or application-specific types are needed.
//! Shared production retains demand and inputs across explicit provider decisions.
//! Publication validates current authority and declared dependency revisions.
//! See `pubdocs/README.md` and `examples/` for standalone use.

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
mod source;
mod storage;

pub use accounting::{
    SCAccountingDomain, SCAccountingError, SCAccountingSnapshot, SCAllocationCharge,
    SCAllocationClass, SCChargeTransferFailure,
};
pub use cache::{
    SCCache, SCIdentity, SCInstallError, SCInstallErrorReason, SCInstallResult, SCLookup,
    SCRetentionReport, SCStoredPayload,
};
pub use demand::{SCConsumerIdentity, SCDemand, SCDemandHandle, SCDemandSnapshot, SCUrgencyUpdate};
pub use dependency::{
    SCDependencies, SCDependency, SCDependencyError, SCDependencyScope, SCDependencySnapshot,
};
pub use ownership::{SCAcquisitionError, SCBacking, SCCleanupContext, SCView};
pub use policy::{
    SC_REFERENCE_PAYLOAD_RETENTION, SC_REFERENCE_PRODUCTION_LIMIT, SC_REFERENCE_PROVIDER_SLOTS,
    SC_REFERENCE_RANGE_ENTRIES, SCAdmission, SCAdmissionError, SCAdmissionPermit,
    SCAdmissionSnapshot, SCByteLimit, SCCountLimit, SCPolicyCounter, SCPolicyCounterError,
};
pub use pool::{SCPool, SCPoolClosed, SCPoolLease, SCPoolRejected, SCPoolReturn, SCPoolSnapshot};
pub use production::{
    SCChildRejected, SCCompletionRejected, SCCompletionRejectionReason, SCProducer, SCProduction,
    SCProductionAccess, SCProductionAttempt, SCProductionClaim, SCProductionError, SCProductionMap,
    SCProductionPhase, SCProductionSnapshot, SCProductionStart, SCProviderDecision,
    SCRejectedSubmission, SCStartRejected, SCStartRejectionReason, SCSubmission,
    SCSubmissionOutcome,
};
pub use publication::{
    SCAuthority, SCPublication, SCPublicationAttempt, SCPublicationRejected,
    SCPublicationRejectionReason,
};
pub use range::{
    SCRangeAllocation, SCRangeError, SCRangeInsert, SCRangeRead, SCRangeSnapshot, SCRangeStore,
    SCSourceSnapshot,
};
pub use source::{
    SCSourceBufferSnapshot, SCSourceBuffers, SCSourceInsert, SCSourceRejected,
    SCSourceRejectionReason,
};
