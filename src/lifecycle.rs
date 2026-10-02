//! Domain-scoped closure observation, derived from the owning bookkeeping.
//!
//! Admission closes new roots without revoking accepted access capabilities.
//! The host retains callback-producing services through their actual settlement,
//! then retires facility ownership and drives cleanup in its legal context.
//! Failed external stop/wait never transfers permission to free live backing.

use crate::policy::SCAdmissionSnapshot;

impl SCAdmissionSnapshot {
    /// Whether this count domain is permanently closed with no live permits.
    ///
    /// This is a projection of one coherent snapshot, not an application drain
    /// receipt. It excludes unclaimed outputs, escaped owners, deferred cleanup,
    /// executor delivery and external users not covered by a retained permit.
    /// Production-bound permits cover submission and accepted final accesses;
    /// accepted child discovery extends those accesses without reopening roots.
    pub fn is_drained(&self) -> bool {
        self.closed && self.active == 0
    }
}
