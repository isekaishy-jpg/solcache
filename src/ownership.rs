//! Retained backing, borrowed projections and explicitly scoped cleanup.

use std::fmt;
use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::{Arc, Mutex, MutexGuard};

use crate::accounting::{
    SCAccountingDomain, SCAccountingError, SCAllocationCharge, SCAllocationClass,
};

/// An acquisition failure that preserves the supplied value.
pub struct SCAcquisitionError<T> {
    value: T,
    reason: SCAccountingError,
}

impl<T> SCAcquisitionError<T> {
    /// The reason no backing owner was acquired.
    pub fn reason(&self) -> &SCAccountingError {
        &self.reason
    }

    /// Returns the original value and accounting failure without copying either.
    pub fn into_parts(self) -> (T, SCAccountingError) {
        (self.value, self.reason)
    }
}

impl<T> fmt::Debug for SCAcquisitionError<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SCAcquisitionError")
            .field("reason", &self.reason)
            .finish_non_exhaustive()
    }
}

// Field order ensures payload destruction precedes charge release, including
// unwinding when the payload destructor panics.
struct CleanupRecord<T> {
    value: T,
    charge: SCAllocationCharge,
}

struct CleanupQueue<T> {
    records: Mutex<Vec<CleanupRecord<T>>>,
}

impl<T> CleanupQueue<T> {
    fn records(&self) -> MutexGuard<'_, Vec<CleanupRecord<T>>> {
        // No user code runs under this lock. Recovering poison preserves any
        // retained records if an unexpected bookkeeping panic occurred.
        self.records
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }

    fn drain(&self) -> usize {
        let records = std::mem::take(&mut *self.records());
        let count = records.len();
        // The bookkeeping guard has ended before any payload destructor runs.
        // Unwinding drops the remaining extracted records on this same thread.
        drop(records);
        count
    }
}

struct Backing<T> {
    record: Option<CleanupRecord<T>>,
    cleanup: Option<Arc<CleanupQueue<T>>>,
}

impl<T> Drop for Backing<T> {
    fn drop(&mut self) {
        if let Some(mut record) = self.record.take() {
            record.charge.transition(SCAllocationClass::Retiring);
            if let Some(cleanup) = &self.cleanup {
                cleanup.records().push(record);
            } else {
                drop(record);
            }
        }
    }
}

/// Shared immutable backing with one allocation charge across all clones.
///
/// Moves and clones retain the actual value, independently of any table or
/// logical identity. Borrowed views remain valid only while an owner is borrowed.
/// Cloning increments an atomic reference count; it does not copy the value or
/// acquire another charge. `Send` and `Sync` require `T: Send + Sync`.
///
/// Direct owners destroy the value on the thread releasing the final clone.
/// Owners acquired from [`SCCleanupContext`] enqueue final cleanup instead and
/// are bounded by that context's borrow. They can cross scoped thread boundaries,
/// but cannot escape into an unbounded task requiring a `'static` owner.
/// Retain an owner through actual external final use; dropping it is no proof
/// that a provider or device has stopped accessing the value.
pub struct SCBacking<'cleanup, T> {
    backing: Arc<Backing<T>>,
    // Construction borrows the context for this lifetime. The marker deliberately
    // does not refer to its !Sync type: only the private queue crosses threads.
    cleanup_lifetime: PhantomData<&'cleanup ()>,
}

impl<T> SCBacking<'static, T> {
    /// Binds a previously acquired charge to direct backing without recharging.
    ///
    /// The caller must supply the complete charge describing this payload's
    /// declared allocation capacity. Moving both here preserves that charge's
    /// domain and class without a release/reacquisition gap. This does not
    /// measure capacity or change its class; final release still enters retirement.
    pub fn from_charged(value: T, charge: SCAllocationCharge) -> Self {
        bind(value, charge, None)
    }

    /// Acquires synchronous backing with cleanup on its final dropping thread.
    ///
    /// The caller declares byte cost and accounting class; no payload sizing,
    /// policy admission, production or publication is inferred. Accounting
    /// rejection returns the original value. Success owns the charge until the
    /// payload destructor completes, including deferred or unwinding cleanup.
    pub fn try_new(
        value: T,
        domain: &SCAccountingDomain,
        declared_bytes: u64,
        class: SCAllocationClass,
    ) -> Result<Self, SCAcquisitionError<T>> {
        acquire(value, domain, declared_bytes, class, None)
    }
}

impl<T> SCBacking<'_, T> {
    /// Observes whether this is currently the only shared owner.
    ///
    /// Counts include cache slots as well as escaped handles. This observation
    /// does not grant mutable access or reserve uniqueness against a concurrent
    /// clone through another reference. Use [`Self::try_transition`] for an
    /// exclusive accounting-class change.
    pub fn is_unique(&self) -> bool {
        Arc::strong_count(&self.backing) == 1
    }

    /// Returns the complete caller-declared cost retained by this backing.
    pub fn declared_bytes(&self) -> u64 {
        self.backing
            .record
            .as_ref()
            .expect("live backing always retains its cleanup record")
            .charge
            .declared_bytes()
    }

    /// Returns the current allocation class shared by this backing's owners.
    pub fn allocation_class(&self) -> SCAllocationClass {
        self.backing
            .record
            .as_ref()
            .expect("live backing always retains its cleanup record")
            .charge
            .class()
    }

    /// Changes the allocation class only while exclusively owning the backing.
    ///
    /// Returns false without mutation if another owner exists. The total charge,
    /// domain and payload remain unchanged; this grants no external final-use
    /// permission. Class changes are unavailable through shared owners.
    pub fn try_transition(&mut self, class: SCAllocationClass) -> bool {
        let Some(backing) = Arc::get_mut(&mut self.backing) else {
            return false;
        };
        backing
            .record
            .as_mut()
            .expect("live backing always retains its cleanup record")
            .charge
            .transition(class);
        true
    }

    /// Borrows the payload without adding an owner or allocation charge.
    pub fn view(&self) -> SCView<'_, T> {
        let record = self
            .backing
            .record
            .as_ref()
            .expect("live backing always retains its cleanup record");
        SCView {
            value: &record.value,
        }
    }
}

impl<T> Clone for SCBacking<'_, T> {
    fn clone(&self) -> Self {
        Self {
            backing: Arc::clone(&self.backing),
            cleanup_lifetime: PhantomData,
        }
    }
}

fn acquire<'cleanup, T>(
    value: T,
    domain: &SCAccountingDomain,
    declared_bytes: u64,
    class: SCAllocationClass,
    cleanup: Option<Arc<CleanupQueue<T>>>,
) -> Result<SCBacking<'cleanup, T>, SCAcquisitionError<T>> {
    let charge = match domain.charge(declared_bytes, class) {
        Ok(charge) => charge,
        Err(reason) => return Err(SCAcquisitionError { value, reason }),
    };
    Ok(bind(value, charge, cleanup))
}

fn bind<'cleanup, T>(
    value: T,
    charge: SCAllocationCharge,
    cleanup: Option<Arc<CleanupQueue<T>>>,
) -> SCBacking<'cleanup, T> {
    SCBacking {
        backing: Arc::new(Backing {
            record: Some(CleanupRecord { value, charge }),
            cleanup,
        }),
        cleanup_lifetime: PhantomData,
    }
}

/// A bounded borrow of a backing payload or projected portion of it.
///
/// This view does not own backing and cannot outlive the owner borrow from which
/// it was created. Projection uses Rust lifetimes, so no pointer arithmetic,
/// layout assumptions or caller-supplied bounds can shorten backing ownership.
pub struct SCView<'owner, T: ?Sized> {
    value: &'owner T,
}

impl<'owner, T: ?Sized> SCView<'owner, T> {
    /// Returns the borrowed value for the original owner borrow's lifetime.
    pub fn get(self) -> &'owner T {
        self.value
    }

    /// Projects a borrow without copying data or adding shared ownership.
    pub fn map<U: ?Sized>(self, project: impl FnOnce(&'owner T) -> &'owner U) -> SCView<'owner, U> {
        SCView {
            value: project(self.value),
        }
    }
}

impl<T: ?Sized> Copy for SCView<'_, T> {}

impl<T: ?Sized> Clone for SCView<'_, T> {
    fn clone(&self) -> Self {
        *self
    }
}

/// An explicitly driven cleanup queue confined to its creating thread.
///
/// This context is neither `Send` nor `Sync`. Acquired owners borrow its lifetime,
/// preventing it from moving or being destroyed while those owners remain live.
/// Final release on another scoped thread enqueues a retiring payload and charge;
/// [`Self::drain`] or context destruction performs cleanup on the owning thread.
/// There are no worker threads, callbacks or hidden scheduling requirements.
///
/// Cleanup is driven only by the caller. A pending record remains physically
/// charged even though no backing clone remains. Draining does not certify any
/// external final-use obligation: callers must retain owners through that use.
pub struct SCCleanupContext<T> {
    queue: Arc<CleanupQueue<T>>,
    owner_thread: PhantomData<Rc<()>>,
}

impl<T> SCCleanupContext<T> {
    /// Creates an empty context bound to the current thread.
    pub fn new() -> Self {
        Self {
            queue: Arc::new(CleanupQueue {
                records: Mutex::new(Vec::new()),
            }),
            owner_thread: PhantomData,
        }
    }

    /// Binds a previously acquired charge to this context's deferred backing.
    ///
    /// The caller must supply the complete charge describing the payload's
    /// declared allocation capacity. Ownership moves without recharging or
    /// changing its domain or class. The returned owner borrows this context;
    /// final release queues the payload and its charge in the retiring class.
    pub fn acquire_charged(&self, value: T, charge: SCAllocationCharge) -> SCBacking<'_, T> {
        bind(value, charge, Some(Arc::clone(&self.queue)))
    }

    /// Acquires backing whose final destruction is deferred to this context.
    ///
    /// Rejection preserves `value`. The returned lifetime is tied to this borrow
    /// even though the private shared queue permits scoped off-thread release.
    pub fn try_acquire(
        &self,
        value: T,
        domain: &SCAccountingDomain,
        declared_bytes: u64,
        class: SCAllocationClass,
    ) -> Result<SCBacking<'_, T>, SCAcquisitionError<T>> {
        acquire(
            value,
            domain,
            declared_bytes,
            class,
            Some(Arc::clone(&self.queue)),
        )
    }

    /// Destroys the currently queued batch and returns its record count.
    ///
    /// Records arriving after extraction remain queued for a later drain. All
    /// payload destructors run outside bookkeeping locks on the owning thread.
    /// If one panics, unwinding still releases its charge and destroys the other
    /// extracted records; records not yet extracted remain owned by the queue.
    pub fn drain(&self) -> usize {
        self.queue.drain()
    }

    /// Reports queued records; concurrent final releases may change this count.
    pub fn pending(&self) -> usize {
        self.queue.records().len()
    }
}

impl<T> Default for SCCleanupContext<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Drop for SCCleanupContext<T> {
    fn drop(&mut self) {
        self.queue.drain();
    }
}
