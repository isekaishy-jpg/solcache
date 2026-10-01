use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier, Mutex};
use std::thread::{self, ThreadId};

use solcache::{
    SCAccountingDomain, SCAllocationClass, SCCleanupContext, SCPool, SCPoolClosed, SCPoolReturn,
};

struct Probe {
    id: usize,
    drops: Arc<Mutex<Vec<(usize, ThreadId)>>>,
    on_drop: Option<Box<dyn FnOnce() + Send>>,
}

impl Probe {
    fn new(id: usize, drops: &Arc<Mutex<Vec<(usize, ThreadId)>>>) -> Self {
        Self {
            id,
            drops: Arc::clone(drops),
            on_drop: None,
        }
    }
}

impl Drop for Probe {
    fn drop(&mut self) {
        self.drops
            .lock()
            .unwrap()
            .push((self.id, thread::current().id()));
        if let Some(callback) = self.on_drop.take() {
            callback();
        }
    }
}

fn assert_panic(result: std::thread::Result<()>, expected: &str) {
    let payload = result.expect_err("expected the deliberate callback panic");
    let message = payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str));
    assert_eq!(message, Some(expected));
}

#[test]
fn compatible_checkout_reuses_capacity_without_reusing_valid_results() {
    let domain = SCAccountingDomain::new();
    let pool = SCPool::new();
    let mut buffer = Vec::<u8>::with_capacity(64);
    buffer.extend_from_slice(&[1, 2, 3]);
    let capacity = buffer.capacity();
    let address = buffer.as_ptr();
    pool.insert(
        10,
        buffer,
        domain
            .charge(capacity as u64, SCAllocationClass::Temporary)
            .unwrap(),
    )
    .unwrap();
    assert!(pool.checkout(11, 0).unwrap().is_none());
    assert!(pool.checkout(10, capacity as u64 + 1).unwrap().is_none());
    assert_eq!(pool.snapshot().idle_items, 1);
    assert_eq!(pool.snapshot().retained_capacity_bytes, capacity as u128);
    assert_eq!(domain.snapshot().retained_capacity_bytes, capacity as u64);

    let lease = pool.checkout(10, capacity as u64).unwrap().unwrap();
    assert_eq!(lease.get().as_ptr(), address);
    assert_eq!(lease.get().as_slice(), &[1, 2, 3]);
    assert_eq!(lease.declared_bytes(), capacity as u64);
    assert!(pool.checkout(10, 0).unwrap().is_none());
    assert_eq!(pool.snapshot().checked_out_items, 1);
    assert_eq!(domain.snapshot().temporary_bytes, capacity as u64);
    assert_eq!(domain.snapshot().retained_capacity_bytes, 0);
    assert_eq!(lease.reset_and_return(Vec::clear), SCPoolReturn::Retained);
    assert_eq!(pool.snapshot().checked_out_items, 0);
    assert_eq!(domain.snapshot().retained_capacity_bytes, capacity as u64);

    let lease = pool.checkout(10, 1).unwrap().unwrap();
    assert!(lease.get().is_empty());
    assert_eq!(lease.get().capacity(), capacity);
    assert_eq!(lease.get().as_ptr(), address);
    drop(lease);
    assert!(pool.checkout(10, 0).unwrap().is_none());
    assert_eq!(domain.snapshot().total_declared_bytes, 0);
}

#[test]
fn nested_checkout_ownership_ends_during_reset_before_outer_reuse() {
    let domain = SCAccountingDomain::new();
    let child = SCPool::new();
    child
        .insert(
            1,
            vec![4_u8; 16],
            domain.charge(16, SCAllocationClass::Temporary).unwrap(),
        )
        .unwrap();
    let outer = SCPool::new();
    outer
        .insert(
            2,
            Some(child.checkout(1, 16).unwrap().unwrap()),
            domain.charge(8, SCAllocationClass::Temporary).unwrap(),
        )
        .unwrap();
    assert_eq!(outer.snapshot().retained_capacity_bytes, 8);
    assert_eq!(domain.snapshot().total_declared_bytes, 24);
    let lease = outer.checkout(2, 8).unwrap().unwrap();
    assert_eq!(lease.get().as_ref().unwrap().get().as_slice(), &[4_u8; 16]);
    assert_eq!(child.snapshot().checked_out_items, 1);
    assert_eq!(
        lease.reset_and_return(|nested| {
            nested.take().unwrap().reset_and_return(Vec::clear);
            assert_eq!(child.snapshot().idle_items, 1);
            assert_eq!(outer.snapshot().checked_out_items, 1);
        }),
        SCPoolReturn::Retained
    );
    let lease = outer.checkout(2, 0).unwrap().unwrap();
    assert!(lease.get().is_none());
    drop(lease);
    assert_eq!(domain.snapshot().total_declared_bytes, 16);
    assert_eq!(child.close(), 1);
    assert_eq!(domain.snapshot().total_declared_bytes, 0);
}

#[test]
fn close_commits_before_reentrant_destruction_and_late_return() {
    let domain = SCAccountingDomain::new();
    let drops = Arc::new(Mutex::new(Vec::new()));
    let pool = Arc::new(SCPool::new());
    for id in [1, 2] {
        let mut probe = Probe::new(id, &drops);
        let weak = Arc::downgrade(&pool);
        let observation = domain.clone();
        probe.on_drop = Some(Box::new(move || {
            let pool = weak.upgrade().unwrap();
            assert!(pool.snapshot().closed);
            assert_eq!(pool.snapshot().idle_items, 0);
            assert!(matches!(pool.checkout(1, 0), Err(SCPoolClosed)));
            assert!(observation.snapshot().retiring_bytes >= 7);
            assert_eq!(pool.close(), 0);
        }));
        pool.insert(
            1,
            probe,
            domain.charge(7, SCAllocationClass::Resident).unwrap(),
        )
        .unwrap();
    }
    let lease = pool.checkout(1, 0).unwrap().unwrap();
    let active_id = lease.get().id;
    assert_eq!(pool.close(), 1);
    assert_eq!(pool.snapshot().checked_out_items, 1);
    assert_eq!(lease.get().id, active_id);
    assert_eq!(domain.snapshot().temporary_bytes, 7);
    assert_eq!(drops.lock().unwrap().len(), 1);
    assert_eq!(lease.return_to_pool(), SCPoolReturn::Retired);
    assert_eq!(pool.snapshot().checked_out_items, 0);
    assert_eq!(drops.lock().unwrap().len(), 2);
    assert_eq!(domain.snapshot().total_declared_bytes, 0);
}

#[test]
fn rejected_insertion_returns_unchanged_value_and_charge() {
    let domain = SCAccountingDomain::new();
    let pool = SCPool::new();
    pool.close();
    let value = Box::new(vec![8_u8, 9]);
    let address = &*value as *const Vec<u8>;
    let rejected = pool
        .insert(
            1,
            value,
            domain.charge(29, SCAllocationClass::Resident).unwrap(),
        )
        .unwrap_err();
    let (value, charge) = rejected.into_parts();
    assert_eq!(&*value as *const Vec<u8>, address);
    assert_eq!(value.as_slice(), &[8, 9]);
    assert_eq!(charge.class(), SCAllocationClass::Resident);
    assert_eq!(charge.declared_bytes(), 29);
    assert_eq!(domain.snapshot().resident_bytes, 29);
    drop(value);
    drop(charge);
    assert_eq!(domain.snapshot().total_declared_bytes, 0);
}

#[test]
fn trim_bounds_extraction_and_runs_destruction_outside_pool_locks() {
    let domain = SCAccountingDomain::new();
    let drops = Arc::new(Mutex::new(Vec::new()));
    let pool = Arc::new(SCPool::<Probe>::new());
    for id in 0..3 {
        let mut probe = Probe::new(id, &drops);
        if id == 2 {
            let weak = Arc::downgrade(&pool);
            probe.on_drop = Some(Box::new(move || {
                let pool = weak.upgrade().unwrap();
                assert!(!pool.snapshot().closed);
                assert_eq!(pool.snapshot().idle_items, 2);
                let lease = pool.checkout(1, 0).unwrap().unwrap();
                assert_ne!(lease.get().id, 2);
                assert_eq!(lease.return_to_pool(), SCPoolReturn::Retained);
            }));
        }
        pool.insert(
            1,
            probe,
            domain.charge(5, SCAllocationClass::Temporary).unwrap(),
        )
        .unwrap();
    }
    assert_eq!(pool.trim(0), 0);
    assert_eq!(pool.trim(1), 1);
    assert_eq!(drops.lock().unwrap().len(), 1);
    assert_eq!(drops.lock().unwrap()[0].0, 2);
    assert_eq!(pool.snapshot().idle_items, 2);
    assert_eq!(domain.snapshot().retained_capacity_bytes, 10);
    assert_eq!(pool.trim(usize::MAX), 2);
    assert_eq!(domain.snapshot().total_declared_bytes, 0);
    assert_eq!(pool.close(), 0);
}

#[test]
fn reset_reentry_can_close_and_reset_panic_never_returns_dirty_storage() {
    let domain = SCAccountingDomain::new();
    let pool = SCPool::new();
    pool.insert(
        1,
        vec![2_u8; 8],
        domain.charge(8, SCAllocationClass::Resident).unwrap(),
    )
    .unwrap();
    let lease = pool.checkout(1, 8).unwrap().unwrap();
    assert_panic(
        catch_unwind(AssertUnwindSafe(|| {
            lease.reset_and_return(|value| {
                value[0] = 90;
                assert_eq!(pool.snapshot().checked_out_items, 1);
                panic!("intentional pool reset panic");
            });
        })),
        "intentional pool reset panic",
    );
    assert_eq!(pool.snapshot().checked_out_items, 0);
    assert!(pool.checkout(1, 0).unwrap().is_none());
    assert_eq!(domain.snapshot().total_declared_bytes, 0);
    pool.insert(
        1,
        vec![3_u8; 8],
        domain.charge(8, SCAllocationClass::Temporary).unwrap(),
    )
    .unwrap();
    let lease = pool.checkout(1, 8).unwrap().unwrap();
    assert_eq!(
        lease.reset_and_return(|value| {
            assert_eq!(value.as_slice(), &[3_u8; 8]);
            assert_eq!(pool.close(), 0);
            value.clear();
        }),
        SCPoolReturn::Retired
    );
    assert!(pool.snapshot().closed);
    assert_eq!(domain.snapshot().total_declared_bytes, 0);
}

#[test]
fn close_destructor_panic_settles_extracted_batch_and_remains_closed() {
    let domain = SCAccountingDomain::new();
    let drops = Arc::new(Mutex::new(Vec::new()));
    let pool = SCPool::new();
    for id in 0..3 {
        let mut probe = Probe::new(id, &drops);
        if id == 0 {
            let domain = domain.clone();
            probe.on_drop = Some(Box::new(move || {
                assert_eq!(domain.snapshot().retiring_bytes, 11);
                assert_eq!(domain.snapshot().total_declared_bytes, 33);
                panic!("intentional pool destruction panic");
            }));
        }
        pool.insert(
            1,
            probe,
            domain.charge(11, SCAllocationClass::Resident).unwrap(),
        )
        .unwrap();
    }
    assert_panic(
        catch_unwind(AssertUnwindSafe(|| {
            pool.close();
        })),
        "intentional pool destruction panic",
    );
    assert!(pool.snapshot().closed);
    assert_eq!(pool.snapshot().idle_items, 0);
    assert_eq!(pool.close(), 0);
    assert_eq!(drops.lock().unwrap().len(), 3);
    assert_eq!(domain.snapshot().total_declared_bytes, 0);
}

#[test]
fn close_during_reset_preserves_exclusive_access_then_retires_the_return() {
    let domain = SCAccountingDomain::new();
    let pool = SCPool::new();
    pool.insert(
        1,
        vec![17_u8; 8],
        domain.charge(8, SCAllocationClass::Temporary).unwrap(),
    )
    .unwrap();
    let lease = pool.checkout(1, 8).unwrap().unwrap();
    let entered = Arc::new(Barrier::new(2));
    let resume = Arc::new(Barrier::new(2));
    thread::scope(|scope| {
        let entered_worker = Arc::clone(&entered);
        let resume_worker = Arc::clone(&resume);
        let worker = scope.spawn(move || {
            lease.reset_and_return(|value| {
                entered_worker.wait();
                resume_worker.wait();
                assert_eq!(value.as_slice(), &[17_u8; 8]);
                value.clear();
            })
        });
        entered.wait();
        assert_eq!(pool.snapshot().checked_out_items, 1);
        assert_eq!(pool.close(), 0);
        assert_eq!(domain.snapshot().temporary_bytes, 8);
        resume.wait();
        assert_eq!(worker.join().unwrap(), SCPoolReturn::Retired);
    });
    assert_eq!(pool.snapshot().checked_out_items, 0);
    assert_eq!(pool.snapshot().idle_items, 0);
    assert_eq!(domain.snapshot().total_declared_bytes, 0);
}

#[test]
fn discarded_lease_destructor_panic_releases_charge_and_checkout_membership() {
    let domain = SCAccountingDomain::new();
    let drops = Arc::new(Mutex::new(Vec::new()));
    let pool = SCPool::new();
    let mut probe = Probe::new(1, &drops);
    let observation = domain.clone();
    probe.on_drop = Some(Box::new(move || {
        assert_eq!(observation.snapshot().retiring_bytes, 23);
        panic!("intentional discarded lease panic");
    }));
    pool.insert(
        1,
        probe,
        domain.charge(23, SCAllocationClass::Temporary).unwrap(),
    )
    .unwrap();
    let lease = pool.checkout(1, 0).unwrap().unwrap();
    assert_panic(
        catch_unwind(AssertUnwindSafe(|| drop(lease))),
        "intentional discarded lease panic",
    );
    assert_eq!(pool.snapshot().checked_out_items, 0);
    assert!(pool.checkout(1, 0).unwrap().is_none());
    assert!(!pool.snapshot().closed);
    assert_eq!(drops.lock().unwrap().len(), 1);
    assert_eq!(domain.snapshot().total_declared_bytes, 0);
}

#[test]
fn error_and_cancellation_join_borrowed_work_before_storage_reset() {
    let domain = SCAccountingDomain::new();
    for cancelled in [false, true] {
        let pool = SCPool::new();
        pool.insert(
            1,
            vec![42_u8; 16],
            domain.charge(16, SCAllocationClass::Temporary).unwrap(),
        )
        .unwrap();
        let lease = pool.checkout(1, 16).unwrap().unwrap();
        let entered = Arc::new(Barrier::new(2));
        let resume = Arc::new(Barrier::new(2));
        let cancel = AtomicBool::new(false);
        let result: Result<(), &str> = thread::scope(|scope| {
            let bytes = lease.get();
            let entered_worker = Arc::clone(&entered);
            let resume_worker = Arc::clone(&resume);
            let cancel = &cancel;
            let worker = scope.spawn(move || {
                entered_worker.wait();
                resume_worker.wait();
                assert_eq!(bytes.as_slice(), &[42_u8; 16]);
                if cancel.load(Ordering::Acquire) {
                    Err("cancelled")
                } else {
                    Err("provider error")
                }
            });
            entered.wait();
            cancel.store(cancelled, Ordering::Release);
            assert_eq!(pool.snapshot().checked_out_items, 1);
            assert!(pool.checkout(1, 0).unwrap().is_none());
            assert_eq!(pool.close(), 0);
            resume.wait();
            worker.join().unwrap()
        });
        assert_eq!(
            result,
            Err(if cancelled {
                "cancelled"
            } else {
                "provider error"
            })
        );
        assert_eq!(lease.reset_and_return(Vec::clear), SCPoolReturn::Retired);
        assert_eq!(domain.snapshot().total_declared_bytes, 0);
    }
}

#[test]
fn deferred_close_and_late_returns_keep_charge_until_owner_context_drain() {
    let domain = SCAccountingDomain::new();
    let drops = Arc::new(Mutex::new(Vec::new()));
    let context = SCCleanupContext::new();
    let pool = SCPool::with_cleanup(&context);
    for id in 0..2 {
        pool.insert(
            1,
            Probe::new(id, &drops),
            domain.charge(13, SCAllocationClass::Resident).unwrap(),
        )
        .unwrap();
    }
    let lease = pool.checkout(1, 0).unwrap().unwrap();
    assert_eq!(pool.close(), 1);
    assert_eq!(context.pending(), 1);
    assert_eq!(domain.snapshot().retiring_bytes, 13);
    assert_eq!(domain.snapshot().temporary_bytes, 13);
    thread::scope(|scope| {
        scope.spawn(move || {
            assert!(lease.get().id < 2);
            assert_eq!(lease.return_to_pool(), SCPoolReturn::Retired);
        });
    });
    assert_eq!(pool.snapshot().checked_out_items, 0);
    assert!(drops.lock().unwrap().is_empty());
    assert_eq!(context.pending(), 2);
    assert_eq!(domain.snapshot().retiring_bytes, 26);
    assert_eq!(context.drain(), 2);
    assert_eq!(domain.snapshot().total_declared_bytes, 0);
    let observations = drops.lock().unwrap();
    assert_eq!(observations.len(), 2);
    assert!(
        observations
            .iter()
            .all(|(_, id)| *id == thread::current().id())
    );
}

#[test]
fn pool_destruction_closes_without_invalidating_surviving_lease() {
    let domain = SCAccountingDomain::new();
    let drops = Arc::new(Mutex::new(Vec::new()));
    let pool = SCPool::new();
    for id in 0..2 {
        pool.insert(
            1,
            Probe::new(id, &drops),
            domain.charge(19, SCAllocationClass::Resident).unwrap(),
        )
        .unwrap();
    }
    let lease = pool.checkout(1, 0).unwrap().unwrap();
    let active_id = lease.get().id;
    drop(pool);
    assert_eq!(drops.lock().unwrap().len(), 1);
    assert_eq!(lease.get().id, active_id);
    assert_eq!(domain.snapshot().temporary_bytes, 19);
    assert_eq!(lease.return_to_pool(), SCPoolReturn::Retired);
    assert_eq!(drops.lock().unwrap().len(), 2);
    assert_eq!(domain.snapshot().total_declared_bytes, 0);
}

#[test]
fn independent_domains_do_not_overflow_pool_capacity_observation() {
    // Synthetic declared-cost arithmetic, not a claim about physical allocations.
    let first = SCAccountingDomain::new();
    let second = SCAccountingDomain::new();
    let pool = SCPool::new();
    for domain in [&first, &second] {
        pool.insert(
            1,
            (),
            domain
                .charge(u64::MAX, SCAllocationClass::Temporary)
                .unwrap(),
        )
        .unwrap();
    }
    assert_eq!(
        pool.snapshot().retained_capacity_bytes,
        u128::from(u64::MAX) * 2
    );
    assert_eq!(pool.close(), 2);
    assert_eq!(first.snapshot().total_declared_bytes, 0);
    assert_eq!(second.snapshot().total_declared_bytes, 0);
}
