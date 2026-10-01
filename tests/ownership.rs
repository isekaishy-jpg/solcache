use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Barrier, Mutex};
use std::thread::{self, ThreadId};

use solcache::{
    SCAccountingDomain, SCAccountingError, SCAllocationClass, SCBacking, SCCleanupContext,
};

struct DropObservation {
    thread: ThreadId,
    retained_bytes: u64,
    retiring_bytes: u64,
}

struct Tracked {
    domain: SCAccountingDomain,
    observations: Arc<Mutex<Vec<DropObservation>>>,
    panic_on_drop: bool,
    barriers: Option<(Arc<Barrier>, Arc<Barrier>)>,
}

impl Tracked {
    fn new(domain: &SCAccountingDomain, observations: &Arc<Mutex<Vec<DropObservation>>>) -> Self {
        Self {
            domain: domain.clone(),
            observations: Arc::clone(observations),
            panic_on_drop: false,
            barriers: None,
        }
    }
}

impl Drop for Tracked {
    fn drop(&mut self) {
        let snapshot = self.domain.snapshot();
        self.observations.lock().unwrap().push(DropObservation {
            thread: thread::current().id(),
            retained_bytes: snapshot.total_declared_bytes,
            retiring_bytes: snapshot.retiring_bytes,
        });
        if let Some((entered, resume)) = &self.barriers {
            entered.wait();
            resume.wait();
        }
        assert!(!self.panic_on_drop, "test payload destructor panic");
    }
}

#[test]
fn class_changes_require_exclusive_backing_and_preserve_its_charge() {
    let domain = SCAccountingDomain::new();
    let mut owner =
        SCBacking::try_new(vec![1_u8, 2, 3], &domain, 3, SCAllocationClass::Temporary).unwrap();
    assert!(owner.is_unique());
    assert_eq!(owner.declared_bytes(), 3);
    let shared = owner.clone();
    assert!(!owner.is_unique());
    assert!(!owner.try_transition(SCAllocationClass::Resident));
    assert_eq!(shared.allocation_class(), SCAllocationClass::Temporary);
    assert_eq!(domain.snapshot().temporary_bytes, 3);
    drop(shared);
    assert!(owner.try_transition(SCAllocationClass::Resident));
    assert_eq!(owner.allocation_class(), SCAllocationClass::Resident);
    assert_eq!(domain.snapshot().temporary_bytes, 0);
    assert_eq!(domain.snapshot().resident_bytes, 3);
    assert_eq!(owner.view().get(), &[1, 2, 3]);
    drop(owner);
    assert_eq!(domain.snapshot().total_declared_bytes, 0);
}

#[test]
fn acquisition_rejection_preserves_the_original_value_and_counters() {
    let domain = SCAccountingDomain::new();
    let existing = domain
        .charge(u64::MAX, SCAllocationClass::Resident)
        .unwrap();
    let context = SCCleanupContext::new();
    for deferred in [false, true] {
        let value = Box::new(String::from("original value"));
        let address = &*value as *const String;
        let result = if deferred {
            context.try_acquire(value, &domain, 1, SCAllocationClass::Temporary)
        } else {
            SCBacking::try_new(value, &domain, 1, SCAllocationClass::Temporary)
        };
        let error = match result {
            Ok(_) => panic!("overflow must reject acquisition"),
            Err(error) => error,
        };
        assert_eq!(error.reason(), &SCAccountingError::Overflow);
        let (returned, reason) = error.into_parts();
        assert_eq!(&*returned as *const String, address);
        assert_eq!(*returned, "original value");
        assert_eq!(reason, SCAccountingError::Overflow);
        assert_eq!(domain.snapshot().total_declared_bytes, u64::MAX);
        assert_eq!(domain.snapshot().temporary_bytes, 0);
        assert_eq!(context.pending(), 0);
    }
    drop(existing);
    assert_eq!(domain.snapshot().total_declared_bytes, 0);
}

#[test]
fn moves_clones_and_container_removal_keep_one_charge_until_final_release() {
    let domain = SCAccountingDomain::new();
    let observations = Arc::new(Mutex::new(Vec::new()));
    let owner = SCBacking::try_new(
        Tracked::new(&domain, &observations),
        &domain,
        29,
        SCAllocationClass::Resident,
    )
    .unwrap();
    let mut container = vec![owner];
    let escaped = container[0].clone();
    container.clear();
    drop(container);
    let moved = escaped;
    assert_eq!(domain.snapshot().resident_bytes, 29);
    assert!(observations.lock().unwrap().is_empty());
    drop(moved);
    let observations = observations.lock().unwrap();
    assert_eq!(observations.len(), 1);
    assert_eq!(observations[0].retained_bytes, 29);
    assert_eq!(observations[0].retiring_bytes, 29);
    assert_eq!(domain.snapshot().total_declared_bytes, 0);
}

#[test]
fn transferred_charge_binds_to_backing_without_recharging_or_changing_domain() {
    for deferred in [false, true] {
        let source = SCAccountingDomain::new();
        let destination = SCAccountingDomain::new();
        let observations = Arc::new(Mutex::new(Vec::new()));
        let charge = source.charge(47, SCAllocationClass::Temporary).unwrap();
        let charge = charge
            .transfer(&destination, SCAllocationClass::Resident)
            .unwrap();
        assert_eq!(source.snapshot().total_declared_bytes, 0);
        assert_eq!(destination.snapshot().resident_bytes, 47);
        let context = SCCleanupContext::new();
        let value = Tracked::new(&destination, &observations);
        let owner = if deferred {
            context.acquire_charged(value, charge)
        } else {
            SCBacking::from_charged(value, charge)
        };
        let shared = owner.clone();
        drop(owner);
        assert_eq!(source.snapshot().total_declared_bytes, 0);
        assert_eq!(destination.snapshot().resident_bytes, 47);
        assert!(observations.lock().unwrap().is_empty());
        drop(shared);
        if deferred {
            assert_eq!(destination.snapshot().retiring_bytes, 47);
            assert_eq!(destination.snapshot().total_declared_bytes, 47);
            assert!(observations.lock().unwrap().is_empty());
            assert_eq!(context.drain(), 1);
        }
        let observations = observations.lock().unwrap();
        assert_eq!(observations.len(), 1);
        assert_eq!(observations[0].retained_bytes, 47);
        assert_eq!(observations[0].retiring_bytes, 47);
        assert_eq!(destination.snapshot().total_declared_bytes, 0);
    }
}

#[test]
fn projected_borrows_reference_the_original_backing_without_extra_charge() {
    let domain = SCAccountingDomain::new();
    let owner = SCBacking::try_new(
        vec![10_u8, 20, 30, 40],
        &domain,
        4,
        SCAllocationClass::Resident,
    )
    .unwrap();
    let projected = owner.view().map(|bytes| &bytes[1..3]);
    let copied_view = projected;
    assert_eq!(projected.get(), &[20, 30]);
    assert_eq!(copied_view.get().as_ptr(), owner.view().get()[1..].as_ptr());
    assert_eq!(domain.snapshot().total_declared_bytes, 4);
    drop(owner);
    assert_eq!(domain.snapshot().total_declared_bytes, 0);
}

#[test]
fn direct_owner_destroys_payload_on_the_final_releasing_thread() {
    let domain = SCAccountingDomain::new();
    let observations = Arc::new(Mutex::new(Vec::new()));
    let owner = SCBacking::try_new(
        Tracked::new(&domain, &observations),
        &domain,
        7,
        SCAllocationClass::Temporary,
    )
    .unwrap();
    let final_thread = thread::spawn(move || {
        let final_thread = thread::current().id();
        drop(owner);
        final_thread
    })
    .join()
    .unwrap();
    let observations = observations.lock().unwrap();
    assert_eq!(observations.len(), 1);
    assert_eq!(observations[0].thread, final_thread);
    assert_eq!(observations[0].retained_bytes, 7);
    assert_eq!(domain.snapshot().total_declared_bytes, 0);
}

#[test]
fn deferred_final_release_retains_retirement_charge_until_owner_thread_drain() {
    let domain = SCAccountingDomain::new();
    let observations = Arc::new(Mutex::new(Vec::new()));
    let context = SCCleanupContext::new();
    let owner = context
        .try_acquire(
            Tracked::new(&domain, &observations),
            &domain,
            31,
            SCAllocationClass::Resident,
        )
        .unwrap();
    let shared = owner.clone();
    drop(owner);
    thread::scope(|scope| {
        scope.spawn(move || {
            assert_eq!(shared.view().get().domain.snapshot().resident_bytes, 31);
            drop(shared);
        });
    });
    assert!(observations.lock().unwrap().is_empty());
    assert_eq!(context.pending(), 1);
    assert_eq!(domain.snapshot().resident_bytes, 0);
    assert_eq!(domain.snapshot().retiring_bytes, 31);
    assert_eq!(domain.snapshot().total_declared_bytes, 31);
    assert_eq!(context.drain(), 1);
    assert_eq!(context.drain(), 0);
    let observations = observations.lock().unwrap();
    assert_eq!(observations.len(), 1);
    assert_eq!(observations[0].thread, thread::current().id());
    assert_eq!(observations[0].retained_bytes, 31);
    assert_eq!(domain.snapshot().total_declared_bytes, 0);
}

#[test]
fn context_destruction_drains_pending_payloads_on_its_owning_thread() {
    let domain = SCAccountingDomain::new();
    let observations = Arc::new(Mutex::new(Vec::new()));
    {
        let context = SCCleanupContext::new();
        let owner = context
            .try_acquire(
                Tracked::new(&domain, &observations),
                &domain,
                17,
                SCAllocationClass::RetainedCapacity,
            )
            .unwrap();
        drop(owner);
        assert_eq!(domain.snapshot().retained_capacity_bytes, 0);
        assert_eq!(domain.snapshot().retiring_bytes, 17);
    }
    let observations = observations.lock().unwrap();
    assert_eq!(observations.len(), 1);
    assert_eq!(observations[0].thread, thread::current().id());
    assert_eq!(domain.snapshot().total_declared_bytes, 0);
}

#[test]
fn final_release_can_enqueue_while_a_payload_destructor_is_running() {
    let domain = SCAccountingDomain::new();
    let observations = Arc::new(Mutex::new(Vec::new()));
    let context = SCCleanupContext::new();
    let entered = Arc::new(Barrier::new(2));
    let resume = Arc::new(Barrier::new(2));
    let mut first = Tracked::new(&domain, &observations);
    first.barriers = Some((Arc::clone(&entered), Arc::clone(&resume)));
    let first_owner = context
        .try_acquire(first, &domain, 5, SCAllocationClass::Resident)
        .unwrap();
    let second_owner = context
        .try_acquire(
            Tracked::new(&domain, &observations),
            &domain,
            11,
            SCAllocationClass::Resident,
        )
        .unwrap();
    drop(first_owner);
    thread::scope(|scope| {
        scope.spawn(move || {
            entered.wait();
            drop(second_owner);
            resume.wait();
        });
        assert_eq!(context.drain(), 1);
    });
    assert_eq!(context.pending(), 1);
    assert_eq!(domain.snapshot().retiring_bytes, 11);
    assert_eq!(context.drain(), 1);
    assert_eq!(observations.lock().unwrap().len(), 2);
    assert_eq!(domain.snapshot().total_declared_bytes, 0);
}

#[test]
fn destructor_panic_releases_charges_and_deferred_batch_without_poisoning_queue() {
    for deferred in [false, true] {
        let domain = SCAccountingDomain::new();
        let observations = Arc::new(Mutex::new(Vec::new()));
        let context = SCCleanupContext::new();
        let mut value = Tracked::new(&domain, &observations);
        value.panic_on_drop = true;
        let owner = if deferred {
            context.try_acquire(value, &domain, 23, SCAllocationClass::Temporary)
        } else {
            SCBacking::try_new(value, &domain, 23, SCAllocationClass::Temporary)
        }
        .unwrap();
        let result = catch_unwind(AssertUnwindSafe(|| {
            drop(owner);
            if deferred {
                let second_owner = context
                    .try_acquire(
                        Tracked::new(&domain, &observations),
                        &domain,
                        13,
                        SCAllocationClass::Resident,
                    )
                    .unwrap();
                drop(second_owner);
                context.drain();
            }
        }));
        assert!(result.is_err());
        assert_eq!(domain.snapshot().total_declared_bytes, 0);
        assert_eq!(context.pending(), 0);
        assert_eq!(context.drain(), 0);
        assert_eq!(
            observations.lock().unwrap().len(),
            if deferred { 2 } else { 1 }
        );
    }
}
