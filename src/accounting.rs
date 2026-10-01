//! Declared allocation costs, separate from family policy counters and admission.
//!
//! Costs describe caller-declared backing capacity, not measured process memory.
//! Each charge occupies exactly one lifecycle class; changing valid payload length
//! does not change its cost. Shared backing must own one charge through final cleanup.

use std::error::Error;
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard};

/// The current ownership role of declared allocation capacity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SCAllocationClass {
    /// Backing owned during production or temporary use.
    Temporary,
    /// Backing retained for active or reusable payload access.
    Resident,
    /// Backing awaiting its actual final release, including deferred cleanup.
    Retiring,
    /// Idle backing capacity retained for later checkout or reuse.
    RetainedCapacity,
}

/// A coherent observation of one accounting domain, in declared bytes.
///
/// Classes are exclusive, so their sum equals `total_declared_bytes`. This does
/// not report policy occupancy, measured memory, or domain shutdown completion.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SCAccountingSnapshot {
    pub temporary_bytes: u64,
    pub resident_bytes: u64,
    pub retiring_bytes: u64,
    pub retained_capacity_bytes: u64,
    pub total_declared_bytes: u64,
}

impl SCAccountingSnapshot {
    fn bucket_mut(&mut self, class: SCAllocationClass) -> &mut u64 {
        match class {
            SCAllocationClass::Temporary => &mut self.temporary_bytes,
            SCAllocationClass::Resident => &mut self.resident_bytes,
            SCAllocationClass::Retiring => &mut self.retiring_bytes,
            SCAllocationClass::RetainedCapacity => &mut self.retained_capacity_bytes,
        }
    }

    fn add(&mut self, bytes: u64, class: SCAllocationClass) -> Result<(), SCAccountingError> {
        let total = self
            .total_declared_bytes
            .checked_add(bytes)
            .ok_or(SCAccountingError::Overflow)?;
        // Every bucket is at most the total; the checked total proves this addition.
        *self.bucket_mut(class) += bytes;
        self.total_declared_bytes = total;
        Ok(())
    }

    fn remove(&mut self, bytes: u64, class: SCAllocationClass) {
        // Each non-cloneable charge removes only its own committed contribution.
        *self.bucket_mut(class) -= bytes;
        self.total_declared_bytes -= bytes;
    }

    fn transition(&mut self, bytes: u64, from: SCAllocationClass, to: SCAllocationClass) {
        *self.bucket_mut(from) -= bytes;
        *self.bucket_mut(to) += bytes;
    }
}

/// An accounting operation could not represent the resulting declared byte total.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SCAccountingError {
    Overflow,
}

impl fmt::Display for SCAccountingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Overflow => formatter.write_str("declared allocation byte total overflows u64"),
        }
    }
}

impl Error for SCAccountingError {}

/// Shared bookkeeping for one caller-selected allocation ownership domain.
///
/// Operations synchronously acquire bookkeeping mutexes and never invoke user
/// code under those locks. There is no admission limit or global hard cap.
/// Charges keep the domain alive independently of these cloned observation handles.
#[derive(Clone, Default)]
pub struct SCAccountingDomain {
    counters: Arc<Mutex<SCAccountingSnapshot>>,
}

impl fmt::Debug for SCAccountingDomain {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Formatting invokes the caller's writer, which may reenter accounting.
        let snapshot = self.snapshot();
        formatter
            .debug_struct("SCAccountingDomain")
            .field("snapshot", &snapshot)
            .finish()
    }
}

impl SCAccountingDomain {
    pub fn new() -> Self {
        Self::default()
    }

    /// Declare backing capacity owned by the returned non-cloneable charge.
    ///
    /// Zero-byte charges are valid. Overflow leaves the domain unchanged. The
    /// caller supplies the cost and must bind this charge to actual ownership;
    /// this operation neither allocates payload storage nor measures its cost.
    pub fn charge(
        &self,
        declared_bytes: u64,
        class: SCAllocationClass,
    ) -> Result<SCAllocationCharge, SCAccountingError> {
        lock(&self.counters).add(declared_bytes, class)?;
        Ok(SCAllocationCharge {
            counters: Arc::clone(&self.counters),
            declared_bytes,
            class,
        })
    }

    /// Observe this domain without performing cleanup or changing accounting.
    pub fn snapshot(&self) -> SCAccountingSnapshot {
        *lock(&self.counters)
    }
}

// Only checked, callback-free bookkeeping executes under these guards. A poison
// marker therefore does not imply a partially executed user operation. Recover
// the protected state rather than panic from release during unrelated unwinding.
fn lock(counters: &Mutex<SCAccountingSnapshot>) -> MutexGuard<'_, SCAccountingSnapshot> {
    counters.lock().unwrap_or_else(|poison| poison.into_inner())
}

/// One declared allocation cost, retained until actual backing cleanup finishes.
///
/// Move this charge with backing ownership. Shared handles retain the same backing
/// and charge rather than making another charge. Drop removes its contribution;
/// it is the caller's obligation to drop the charge only after payload cleanup.
/// Forgetting it deliberately retains its accounting contribution.
pub struct SCAllocationCharge {
    counters: Arc<Mutex<SCAccountingSnapshot>>,
    declared_bytes: u64,
    class: SCAllocationClass,
}

impl fmt::Debug for SCAllocationCharge {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Drop the guard before any caller-controlled formatting runs.
        let snapshot = *lock(&self.counters);
        formatter
            .debug_struct("SCAllocationCharge")
            .field("domain_snapshot", &snapshot)
            .field("declared_bytes", &self.declared_bytes)
            .field("class", &self.class)
            .finish()
    }
}

impl SCAllocationCharge {
    pub fn declared_bytes(&self) -> u64 {
        self.declared_bytes
    }

    pub fn class(&self) -> SCAllocationClass {
        self.class
    }

    /// Move the complete capacity charge between exclusive lifecycle classes.
    ///
    /// The total remains constant. Retirement records must retain this charge
    /// until their payload or external backing is actually released.
    pub fn transition(&mut self, class: SCAllocationClass) {
        lock(&self.counters).transition(self.declared_bytes, self.class, class);
        self.class = class;
    }

    /// Move the charge to another domain and lifecycle class.
    ///
    /// Both domains commit under locks acquired in a stable order. Overflow
    /// returns the unchanged original charge; neither domain changes. Transfers
    /// to the same domain are class transitions. Independently sampled domains
    /// do not constitute one atomic global snapshot. No payload cleanup occurs.
    pub fn transfer(
        mut self,
        destination: &SCAccountingDomain,
        class: SCAllocationClass,
    ) -> Result<Self, SCChargeTransferFailure> {
        if Arc::ptr_eq(&self.counters, &destination.counters) {
            self.transition(class);
            return Ok(self);
        }
        let result = {
            let (mut source, mut target) =
                if Arc::as_ptr(&self.counters) < Arc::as_ptr(&destination.counters) {
                    let source = lock(&self.counters);
                    let target = lock(&destination.counters);
                    (source, target)
                } else {
                    let target = lock(&destination.counters);
                    let source = lock(&self.counters);
                    (source, target)
                };
            target.add(self.declared_bytes, class).map(|()| {
                source.remove(self.declared_bytes, self.class);
            })
        };
        match result {
            Ok(()) => {
                self.counters = Arc::clone(&destination.counters);
                self.class = class;
                Ok(self)
            }
            Err(error) => Err(SCChargeTransferFailure {
                error,
                charge: self,
            }),
        }
    }
}

impl Drop for SCAllocationCharge {
    fn drop(&mut self) {
        lock(&self.counters).remove(self.declared_bytes, self.class);
    }
}

/// Failed transfer retaining the complete original allocation charge.
#[derive(Debug)]
pub struct SCChargeTransferFailure {
    error: SCAccountingError,
    charge: SCAllocationCharge,
}

impl SCChargeTransferFailure {
    pub fn error(&self) -> SCAccountingError {
        self.error
    }

    /// Recover the failure reason and original charge for retry or settlement.
    pub fn into_parts(self) -> (SCAccountingError, SCAllocationCharge) {
        (self.error, self.charge)
    }
}
