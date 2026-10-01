//! Independent policy measures and explicit count/byte comparisons.
//!
//! Threshold comparisons do not reserve resources. Admission domains explicitly
//! reserve independent counts; none of these operations evicts values or imposes
//! an aggregate memory ceiling. Callers choose each admission or maintenance point.

use std::error::Error;
use std::fmt;

mod admission;
pub use admission::{SCAdmission, SCAdmissionError, SCAdmissionPermit, SCAdmissionSnapshot};

/// A count threshold, separate from bytes and execution concurrency.
///
/// A zero limit permits no additional item. An existing count over the threshold
/// remains valid state; this comparison neither cancels nor reclaims its owners.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SCCountLimit {
    limit: usize,
}

impl SCCountLimit {
    /// Configures the maximum count for this policy.
    pub const fn new(limit: usize) -> Self {
        Self { limit }
    }

    /// Returns the configured count.
    pub const fn limit(self) -> usize {
        self.limit
    }

    /// Whether one more item may be considered at the supplied current count.
    ///
    /// This is a comparison, not an atomic claim or admission receipt.
    pub const fn allows(self, current: usize) -> bool {
        current < self.limit
    }

    /// Returns the remaining count allowance, clamped at zero.
    pub const fn remaining(self, current: usize) -> usize {
        self.limit.saturating_sub(current)
    }
}

/// A byte-policy threshold with explicit equality and disable semantics.
///
/// Select the comparison used by the particular family. A maintenance target
/// checked before collection and a soft stop checked after insertion are different
/// uses of a threshold; this type performs no insertion or allocation itself.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SCByteLimit {
    /// The byte-policy comparison is disabled.
    Disabled,
    /// The threshold is reached when usage equals or exceeds these bytes.
    AtOrAbove(u64),
    /// The threshold is reached only when usage exceeds these bytes.
    Above(u64),
}

impl SCByteLimit {
    /// Evaluates this threshold against the supplied policy byte count.
    ///
    /// `AtOrAbove(0)` is always reached; `Above(0)` is reached for positive usage;
    /// `Disabled` is never reached. Zero has no implicit global meaning.
    pub const fn reached(self, current_bytes: u64) -> bool {
        match self {
            Self::Disabled => false,
            Self::AtOrAbove(bytes) => current_bytes >= bytes,
            Self::Above(bytes) => current_bytes > bytes,
        }
    }
}

/// Reference production allowance, including production already accounted active.
///
/// Select this setting explicitly for the corresponding family. It does not grant
/// this many additional starts every service pass or configure any worker pool.
pub const SC_REFERENCE_PRODUCTION_LIMIT: SCCountLimit = SCCountLimit::new(16);

/// Reference retained-payload collection threshold of 32 MiB.
///
/// Equality requests collection toward strictly below the threshold. Only eligible
/// unpinned payloads may be collected; this is not a global memory ceiling.
pub const SC_REFERENCE_PAYLOAD_RETENTION: SCByteLimit = SCByteLimit::AtOrAbove(32 * 1024 * 1024);

/// Reference active-provider slot count, independent of queued demand and bytes.
pub const SC_REFERENCE_PROVIDER_SLOTS: SCCountLimit = SCCountLimit::new(8);

/// Reference range-store occupancy, independent of retained backing capacity.
pub const SC_REFERENCE_RANGE_ENTRIES: SCCountLimit = SCCountLimit::new(16);

/// Failure of a checked policy-counter update. The counter is unchanged.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SCPolicyCounterError {
    /// Adding the requested amount would exceed the representable count.
    Overflow,
    /// Removing the requested amount would exceed the current count.
    Underflow,
}

impl fmt::Display for SCPolicyCounterError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Overflow => formatter.write_str("policy counter overflow"),
            Self::Underflow => formatter.write_str("policy counter underflow"),
        }
    }
}

impl Error for SCPolicyCounterError {}

/// An independently maintained family policy measure.
///
/// The caller defines its unit and update points. A read/admission measure can
/// differ from unique retained bytes, and an eviction measure can decrease before
/// physical cleanup. This counter never creates or releases an allocation charge.
/// Updates require exclusive access; no internal synchronization is provided.
#[derive(Debug, Default)]
pub struct SCPolicyCounter {
    value: u64,
}

impl SCPolicyCounter {
    /// Creates a counter with an explicitly supplied initial policy measure.
    pub const fn new(initial: u64) -> Self {
        Self { value: initial }
    }

    /// Returns the current policy measure, not an allocation measurement.
    pub const fn value(&self) -> u64 {
        self.value
    }

    /// Adds an amount, preserving the previous value on overflow.
    pub fn try_add(&mut self, amount: u64) -> Result<(), SCPolicyCounterError> {
        let next = self
            .value
            .checked_add(amount)
            .ok_or(SCPolicyCounterError::Overflow)?;
        self.value = next;
        Ok(())
    }

    /// Removes an amount, preserving the previous value on underflow.
    pub fn try_sub(&mut self, amount: u64) -> Result<(), SCPolicyCounterError> {
        let next = self
            .value
            .checked_sub(amount)
            .ok_or(SCPolicyCounterError::Underflow)?;
        self.value = next;
        Ok(())
    }

    /// Clears this policy measure and returns its old value.
    ///
    /// Clearing a counter does not settle work or release physical storage.
    pub fn clear(&mut self) -> u64 {
        std::mem::take(&mut self.value)
    }
}
