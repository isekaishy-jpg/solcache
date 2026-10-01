//! Exclusive reusable storage, separate from result validity and execution.

use std::error::Error;
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard};

use crate::accounting::{SCAllocationCharge, SCAllocationClass};
use crate::ownership::{SCCleanupContext, SCCleanupSink};

// Field order keeps the charge alive through payload destruction, even unwinding.
struct Item<T> {
    value: Option<T>,
    charge: Option<SCAllocationCharge>,
    layout: u64,
}

impl<T> Item<T> {
    fn declared_bytes(&self) -> u64 {
        self.charge
            .as_ref()
            .expect("owned pool item retains its allocation charge")
            .declared_bytes()
    }
}

impl<T> Drop for Item<T> {
    fn drop(&mut self) {
        if let Some(charge) = &mut self.charge {
            charge.transition(SCAllocationClass::Retiring);
        }
    }
}

struct State<T> {
    closed: bool,
    idle: Vec<Item<T>>,
    checked_out: usize,
}

struct PoolInner<'cleanup, T> {
    state: Mutex<State<T>>,
    cleanup: Option<SCCleanupSink<'cleanup, T>>,
}

impl<T> PoolInner<'_, T> {
    fn state(&self) -> MutexGuard<'_, State<T>> {
        // Only callback-free bookkeeping runs under this mutex.
        self.state.lock().unwrap_or_else(|error| error.into_inner())
    }

    fn retire(&self, mut item: Item<T>) {
        if let Some(cleanup) = &self.cleanup {
            let value = item
                .value
                .take()
                .expect("owned pool item retains its payload");
            let charge = item
                .charge
                .take()
                .expect("owned pool item retains its allocation charge");
            cleanup.retire(value, charge);
        } else {
            drop(item);
        }
    }

    fn retire_batch(&self, items: Vec<Item<T>>) -> usize {
        let count = items.len();
        // Deferred retirement does not run user code. For direct destruction,
        // unwinding also retires and destroys the remaining iterator entries.
        for item in items {
            self.retire(item);
        }
        count
    }
}

/// Observation of pool storage, without running reset or cleanup code.
///
/// Declared idle bytes sum only the charges supplied directly with the items.
/// Independently charged nested owners remain in their own accounting domains.
/// The wider sum accommodates charges from multiple domains without overflow.
/// Checked-out counts end when access/reset ends and ownership is surrendered;
/// they do not include pending deferred cleanup or certify external final use.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SCPoolSnapshot {
    pub closed: bool,
    pub idle_items: usize,
    pub checked_out_items: usize,
    pub retained_capacity_bytes: u128,
}

/// Checkout was attempted after the pool committed closure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SCPoolClosed;

impl fmt::Display for SCPoolClosed {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("reusable storage pool is closed")
    }
}

impl Error for SCPoolClosed {}

/// Insertion rejected by closure, retaining the original payload and charge.
pub struct SCPoolRejected<T> {
    value: T,
    charge: SCAllocationCharge,
}

impl<T> SCPoolRejected<T> {
    /// Recovers caller ownership without changing the original charge's class.
    pub fn into_parts(self) -> (T, SCAllocationCharge) {
        (self.value, self.charge)
    }
}

impl<T> fmt::Debug for SCPoolRejected<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SCPoolRejected")
            .finish_non_exhaustive()
    }
}

/// The ownership disposition after a lease's final access and reset.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SCPoolReturn {
    /// Reset storage is idle and eligible for a later compatible checkout.
    Retained,
    /// Closure surrendered storage to direct or deferred cleanup.
    ///
    /// This does not certify that deferred destruction has completed.
    Retired,
}

/// Independently usable exclusive object or buffer storage.
///
/// The caller assigns layout keys and supplies complete declared allocation
/// charges. Equal layout keys must mean compatible storage; the optional minimum
/// byte requirement screens declared capacity, without measuring or validating
/// payload layout. Keys say nothing about the validity of previous results.
/// There is no factory, executor, automatic reset, admission limit or byte cap.
/// Nested backing belongs to `T` and remains owned until reset or destruction.
///
/// Operations synchronously lock bookkeeping; reset and payload destruction run
/// outside those locks. Direct cleanup runs on the retiring caller's thread;
/// [`Self::with_cleanup`] instead queues final payload destruction to its borrowed
/// context. Reset still runs on the returning caller's thread. `Send` and `Sync`
/// require `T: Send`; a caller needing stronger reset affinity must return there.
/// Closing, including pool destruction, drains idle storage without invalidating
/// existing leases. Leases retain cleanup state for later return or discard.
pub struct SCPool<'cleanup, T> {
    inner: Arc<PoolInner<'cleanup, T>>,
}

impl<T> SCPool<'static, T> {
    /// Creates an empty pool with direct destruction on the retiring thread.
    pub fn new() -> Self {
        Self::build(None)
    }
}

impl<'cleanup, T> SCPool<'cleanup, T> {
    /// Creates storage whose final destruction is queued to this context.
    ///
    /// The pool and all leases borrow the context, including across scoped
    /// threads. Closing empties idle storage but does not drive the cleanup queue.
    pub fn with_cleanup(context: &'cleanup SCCleanupContext<T>) -> Self {
        Self::build(Some(context.sink()))
    }

    fn build(cleanup: Option<SCCleanupSink<'cleanup, T>>) -> Self {
        Self {
            inner: Arc::new(PoolInner {
                state: Mutex::new(State {
                    closed: false,
                    idle: Vec::new(),
                    checked_out: 0,
                }),
                cleanup,
            }),
        }
    }

    /// Moves an already reset item and its existing charge into idle storage.
    ///
    /// Success changes the charge to retained capacity without recharging it.
    /// Closure returns both inputs unchanged. The direct charge must describe
    /// this item's retained allocation capacity; separately charged nested owners
    /// must not also be counted in it.
    pub fn insert(
        &self,
        layout: u64,
        value: T,
        mut charge: SCAllocationCharge,
    ) -> Result<(), SCPoolRejected<T>> {
        let mut state = self.inner.state();
        if state.closed {
            return Err(SCPoolRejected { value, charge });
        }
        charge.transition(SCAllocationClass::RetainedCapacity);
        state.idle.push(Item {
            value: Some(value),
            charge: Some(charge),
            layout,
        });
        Ok(())
    }

    /// Checks out compatible idle storage exclusively, or reports a miss.
    ///
    /// The charge enters temporary use. A miss creates no storage or work.
    /// No ordering or best-fit selection among compatible items is promised.
    pub fn checkout(
        &self,
        layout: u64,
        minimum_declared_bytes: u64,
    ) -> Result<Option<SCPoolLease<'cleanup, T>>, SCPoolClosed> {
        let mut state = self.inner.state();
        if state.closed {
            return Err(SCPoolClosed);
        }
        let Some(index) = state.idle.iter().position(|item| {
            item.layout == layout && item.declared_bytes() >= minimum_declared_bytes
        }) else {
            return Ok(None);
        };
        let mut item = state.idle.swap_remove(index);
        item.charge
            .as_mut()
            .expect("owned pool item retains its allocation charge")
            .transition(SCAllocationClass::Temporary);
        state.checked_out += 1;
        Ok(Some(SCPoolLease {
            inner: Arc::clone(&self.inner),
            item: Some(item),
        }))
    }

    pub fn snapshot(&self) -> SCPoolSnapshot {
        let state = self.inner.state();
        SCPoolSnapshot {
            closed: state.closed,
            idle_items: state.idle.len(),
            checked_out_items: state.checked_out,
            retained_capacity_bytes: state
                .idle
                .iter()
                .map(|item| u128::from(item.declared_bytes()))
                .sum(),
        }
    }

    /// Retires at most `maximum_items` currently idle items, leaving checkout open.
    ///
    /// The action count does not bound destructor execution time. Concurrent
    /// returns may supply new idle storage after extraction. If destruction
    /// panics, the extracted batch still retires outside the bookkeeping lock.
    pub fn trim(&self, maximum_items: usize) -> usize {
        let items = {
            let mut state = self.inner.state();
            let retained = state.idle.len().saturating_sub(maximum_items);
            state.idle.split_off(retained)
        };
        self.inner.retire_batch(items)
    }

    /// Commits closure and retires all currently idle storage.
    ///
    /// Closure is idempotent, precedes any destructor, and blocks future checkout
    /// and insertion. Checked-out leases remain valid; later returns retire them.
    /// The returned count names extracted idle items only, without waiting for
    /// borrowers, external users, or deferred cleanup. A destructor panic leaves
    /// closure committed and the whole extracted batch owned through unwinding.
    pub fn close(&self) -> usize {
        let items = {
            let mut state = self.inner.state();
            state.closed = true;
            std::mem::take(&mut state.idle)
        };
        self.inner.retire_batch(items)
    }
}

impl<T> Default for SCPool<'static, T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Drop for SCPool<'_, T> {
    fn drop(&mut self) {
        self.close();
    }
}

/// A noncloneable exclusive checkout retaining its payload and cleanup state.
///
/// Borrows from this lease must end before consuming return or discard. Scoped
/// threads therefore join their borrowed work before reset/reuse; cancellation
/// alone provides no permission to return storage. External raw/device access
/// likewise requires the caller's actual final-use proof before surrendering it.
/// Dropping a lease discards its item, so unfinished or panicking reset never
/// silently publishes dirty storage as reusable.
pub struct SCPoolLease<'cleanup, T> {
    inner: Arc<PoolInner<'cleanup, T>>,
    item: Option<Item<T>>,
}

impl<T> SCPoolLease<'_, T> {
    pub fn get(&self) -> &T {
        self.item
            .as_ref()
            .and_then(|item| item.value.as_ref())
            .expect("live lease retains its payload")
    }

    /// Mutates exclusively borrowed storage.
    ///
    /// The caller must keep its declared charge appropriate for the retained
    /// allocation. Changing valid length does not reduce capacity accounting;
    /// increasing backing beyond its declaration requires a new charged item.
    /// Do not move charged backing out with `mem::take` or `mem::replace` and
    /// then let it outlive this lease's charge. Independently owned nested backing
    /// can move safely when its own charge moves with it. Generic `T` mutation
    /// cannot enforce these accounting obligations or validate a correct reset.
    pub fn get_mut(&mut self) -> &mut T {
        self.item
            .as_mut()
            .and_then(|item| item.value.as_mut())
            .expect("live lease retains its payload")
    }

    pub fn declared_bytes(&self) -> u64 {
        self.item
            .as_ref()
            .expect("live lease retains its item")
            .declared_bytes()
    }

    /// Runs the caller's reset after final borrowing, then returns the item.
    ///
    /// Reset runs synchronously outside pool locks, including after closure. It
    /// may reenter this or another pool. If it panics, the item is discarded and
    /// its charge remains owned through cleanup; the pool stays usable.
    pub fn reset_and_return(mut self, reset: impl FnOnce(&mut T)) -> SCPoolReturn {
        reset(self.get_mut());
        self.return_to_pool()
    }

    /// Returns storage whose caller has already completed reset and final use.
    ///
    /// This operation does not clear payloads or validate nested/domain state.
    pub fn return_to_pool(mut self) -> SCPoolReturn {
        let mut item = self.item.take().expect("live lease retains its item");
        let retired = {
            let mut state = self.inner.state();
            state.checked_out -= 1;
            if state.closed {
                Some(item)
            } else {
                item.charge
                    .as_mut()
                    .expect("owned pool item retains its allocation charge")
                    .transition(SCAllocationClass::RetainedCapacity);
                state.idle.push(item);
                None
            }
        };
        if let Some(item) = retired {
            self.inner.retire(item);
            SCPoolReturn::Retired
        } else {
            SCPoolReturn::Retained
        }
    }
}

impl<T> Drop for SCPoolLease<'_, T> {
    fn drop(&mut self) {
        if let Some(item) = self.item.take() {
            self.inner.state().checked_out -= 1;
            self.inner.retire(item);
        }
    }
}
