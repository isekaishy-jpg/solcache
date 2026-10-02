//! Observation of maintenance and remaining obligations at their owning domains.
//!
//! Queue observations complement admission, production, pool, range, source and
//! accounting snapshots; no aggregate sampling operation certifies their drain.
//! Observation never executes callbacks, maintenance or deferred destruction.

/// A coherent observation of one context's deferred queue processing.
///
/// Counts exclude live backing owners and external final-use obligations. They
/// may change immediately after observation; zero counts do not certify closure.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SCCleanupSnapshot {
    /// Records still queued for a future drain.
    pub pending_records: usize,
    /// Extracted records whose payload or charge destruction has not completed.
    pub in_flight_records: usize,
}
