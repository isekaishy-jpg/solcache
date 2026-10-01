//! Independent count reservations for production or provider operation domains.

use std::error::Error;
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard};

use super::SCCountLimit;

/// A coherent count observation, independent of bytes and executor concurrency.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SCAdmissionSnapshot {
    pub active: usize,
    pub limit: usize,
}

/// A caller-selected count domain with synchronous checked admission.
///
/// Use separate domains for production attempts and active provider operations.
/// Clones observe and reserve the same domain. Lowering a limit never revokes
/// accepted permits. Bookkeeping locks never invoke caller code.
#[derive(Clone)]
pub struct SCAdmission {
    state: Arc<Mutex<SCAdmissionSnapshot>>,
}

impl SCAdmission {
    pub fn new(limit: SCCountLimit) -> Self {
        Self {
            state: Arc::new(Mutex::new(SCAdmissionSnapshot {
                active: 0,
                limit: limit.limit(),
            })),
        }
    }

    /// Reserves one count or leaves the domain unchanged at its limit.
    pub fn try_acquire(&self) -> Result<SCAdmissionPermit, SCAdmissionError> {
        let mut state = lock(&self.state);
        if state.active >= state.limit {
            return Err(SCAdmissionError::AtCapacity);
        }
        // active < limit <= usize::MAX proves the increment representable.
        state.active += 1;
        Ok(SCAdmissionPermit {
            domain: self.clone(),
        })
    }

    /// Changes admission for future reservations without canceling existing work.
    pub fn set_limit(&self, limit: SCCountLimit) {
        lock(&self.state).limit = limit.limit();
    }

    pub fn snapshot(&self) -> SCAdmissionSnapshot {
        *lock(&self.state)
    }
}

impl fmt::Debug for SCAdmission {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let snapshot = self.snapshot();
        formatter
            .debug_struct("SCAdmission")
            .field("snapshot", &snapshot)
            .finish()
    }
}

/// Admission could not reserve another count in this domain.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SCAdmissionError {
    AtCapacity,
}

impl fmt::Display for SCAdmissionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("admission count is at capacity")
    }
}
impl Error for SCAdmissionError {}

/// One nonclone reservation, released by its final owner.
///
/// Move it with the operation it represents. Dropping a provider-slot permit
/// says nothing about child work, input use, or allocation lifetime. Production
/// preparation can bind a separate permit through submission and final access.
pub struct SCAdmissionPermit {
    domain: SCAdmission,
}

impl fmt::Debug for SCAdmissionPermit {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SCAdmissionPermit")
            .field("domain", &self.domain)
            .finish()
    }
}

impl Drop for SCAdmissionPermit {
    fn drop(&mut self) {
        lock(&self.domain.state).active -= 1;
    }
}

fn lock(state: &Mutex<SCAdmissionSnapshot>) -> MutexGuard<'_, SCAdmissionSnapshot> {
    // Only nonpanicking primitive bookkeeping runs while locked.
    state.lock().unwrap_or_else(|poison| poison.into_inner())
}
