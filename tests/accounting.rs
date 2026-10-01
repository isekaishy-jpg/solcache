use solcache::{SCAccountingDomain, SCAccountingError, SCAllocationClass, SCPolicyCounter};
use std::sync::{Arc, Barrier};
use std::thread;

#[test]
fn debug_formatters_can_reenter_accounting_without_holding_its_lock() {
    use std::fmt::{self, Debug, Write};
    use std::sync::mpsc::{self, Receiver, Sender};
    use std::time::Duration;

    struct ObservingWriter {
        request: Sender<()>,
        reply: Receiver<u64>,
    }

    impl Write for ObservingWriter {
        fn write_str(&mut self, _: &str) -> fmt::Result {
            self.request.send(()).unwrap();
            // Bound a regression's deadlock, rather than leave the suite hanging.
            match self.reply.recv_timeout(Duration::from_secs(5)) {
                Ok(7) => Ok(()),
                _ => Err(fmt::Error),
            }
        }
    }

    fn check(value: &impl Debug, domain: &SCAccountingDomain) {
        thread::scope(|scope| {
            let (request, requests) = mpsc::channel();
            let (reply, replies) = mpsc::channel();
            let observer = scope.spawn(move || {
                while requests.recv().is_ok() {
                    let total = domain.snapshot().total_declared_bytes;
                    if reply.send(total).is_err() {
                        break;
                    }
                }
            });
            let mut writer = ObservingWriter {
                request,
                reply: replies,
            };
            let result = write!(&mut writer, "{value:?}");
            // Release the request channel before joining, including on failure.
            drop(writer);
            observer.join().unwrap();
            assert!(
                result.is_ok(),
                "Debug held an accounting lock across the writer callback"
            );
        });
    }

    let source = SCAccountingDomain::new();
    let charge = source.charge(7, SCAllocationClass::Resident).unwrap();
    check(&source, &source);
    check(&charge, &source);
    let full = SCAccountingDomain::new();
    let _full_charge = full.charge(u64::MAX, SCAllocationClass::Resident).unwrap();
    let failure = charge
        .transfer(&full, SCAllocationClass::Retiring)
        .unwrap_err();
    check(&failure, &source);
}

#[test]
fn lifecycle_transitions_preserve_complete_capacity() {
    let domain = SCAccountingDomain::new();
    let mut charge = domain.charge(128, SCAllocationClass::Temporary).unwrap();
    for class in [
        SCAllocationClass::Resident,
        SCAllocationClass::RetainedCapacity,
        SCAllocationClass::Retiring,
        SCAllocationClass::Retiring,
    ] {
        charge.transition(class);
        let snapshot = domain.snapshot();
        assert_eq!(charge.class(), class);
        assert_eq!(charge.declared_bytes(), 128);
        assert_eq!(snapshot.total_declared_bytes, 128);
        assert_eq!(
            snapshot.temporary_bytes
                + snapshot.resident_bytes
                + snapshot.retiring_bytes
                + snapshot.retained_capacity_bytes,
            128,
        );
    }
    assert_eq!(domain.snapshot().retiring_bytes, 128);
    drop(charge);
    assert_eq!(domain.snapshot().total_declared_bytes, 0);
}

#[test]
fn domain_handles_do_not_control_charge_lifetime() {
    let domain = SCAccountingDomain::new();
    let observer = domain.clone();
    let charge = domain.charge(31, SCAllocationClass::Resident).unwrap();
    drop(domain);
    assert_eq!(observer.snapshot().resident_bytes, 31);
    let moved_charge = charge;
    assert_eq!(observer.snapshot().resident_bytes, 31);
    drop(moved_charge);
    assert_eq!(observer.snapshot().total_declared_bytes, 0);
}

#[test]
fn checked_admission_accepts_zero_and_rejects_total_overflow_unchanged() {
    let domain = SCAccountingDomain::new();
    let full = domain
        .charge(u64::MAX, SCAllocationClass::Temporary)
        .unwrap();
    let zero = domain.charge(0, SCAllocationClass::Retiring).unwrap();
    let before = domain.snapshot();
    assert_eq!(
        domain.charge(1, SCAllocationClass::Resident).unwrap_err(),
        SCAccountingError::Overflow,
    );
    assert_eq!(domain.snapshot(), before);
    drop(zero);
    assert_eq!(domain.snapshot(), before);
    drop(full);
    assert_eq!(domain.snapshot().total_declared_bytes, 0);
}

#[test]
fn transfers_move_one_charge_between_domains_or_classes() {
    let source = SCAccountingDomain::new();
    let destination = SCAccountingDomain::new();
    let charge = source.charge(23, SCAllocationClass::Temporary).unwrap();
    let charge = charge
        .transfer(&source.clone(), SCAllocationClass::Resident)
        .unwrap();
    assert_eq!(source.snapshot().resident_bytes, 23);
    let charge = charge
        .transfer(&destination, SCAllocationClass::RetainedCapacity)
        .unwrap();
    assert_eq!(source.snapshot().total_declared_bytes, 0);
    assert_eq!(destination.snapshot().retained_capacity_bytes, 23);
    drop(charge);
    assert_eq!(destination.snapshot().total_declared_bytes, 0);
}

#[test]
fn failed_transfer_preserves_original_charge_for_retry() {
    let source = SCAccountingDomain::new();
    let destination = SCAccountingDomain::new();
    let charge = source.charge(7, SCAllocationClass::Temporary).unwrap();
    let full = destination
        .charge(u64::MAX, SCAllocationClass::Resident)
        .unwrap();
    let source_before = source.snapshot();
    let destination_before = destination.snapshot();
    let failure = charge
        .transfer(&destination, SCAllocationClass::Retiring)
        .unwrap_err();
    assert_eq!(failure.error(), SCAccountingError::Overflow);
    assert_eq!(source.snapshot(), source_before);
    assert_eq!(destination.snapshot(), destination_before);
    let (error, charge) = failure.into_parts();
    assert_eq!(error, SCAccountingError::Overflow);
    assert_eq!(charge.class(), SCAllocationClass::Temporary);
    assert_eq!(charge.declared_bytes(), 7);
    drop(full);
    let charge = charge
        .transfer(&destination, SCAllocationClass::Retiring)
        .unwrap();
    assert_eq!(source.snapshot().total_declared_bytes, 0);
    assert_eq!(destination.snapshot().retiring_bytes, 7);
    drop(charge);
    assert_eq!(destination.snapshot().total_declared_bytes, 0);
}

#[test]
fn opposite_direction_transfers_and_shared_domain_charges_remain_coherent() {
    let left = SCAccountingDomain::new();
    let right = SCAccountingDomain::new();
    let left_charge = left.charge(11, SCAllocationClass::Resident).unwrap();
    let right_charge = right.charge(17, SCAllocationClass::Resident).unwrap();
    let barrier = Arc::new(Barrier::new(3));
    let handles: Vec<_> = [(left_charge, right.clone()), (right_charge, left.clone())]
        .into_iter()
        .map(|(charge, destination)| {
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                let charge = charge
                    .transfer(&destination, SCAllocationClass::Retiring)
                    .unwrap();
                let temporary = destination.charge(5, SCAllocationClass::Temporary).unwrap();
                barrier.wait();
                barrier.wait();
                drop(temporary);
                charge
            })
        })
        .collect();
    barrier.wait();
    barrier.wait();
    assert_eq!(left.snapshot().retiring_bytes, 17);
    assert_eq!(right.snapshot().retiring_bytes, 11);
    assert_eq!(left.snapshot().temporary_bytes, 5);
    assert_eq!(right.snapshot().temporary_bytes, 5);
    barrier.wait();
    let charges: Vec<_> = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect();
    assert_eq!(left.snapshot().total_declared_bytes, 17);
    assert_eq!(right.snapshot().total_declared_bytes, 11);
    drop(charges);
    assert_eq!(left.snapshot().total_declared_bytes, 0);
    assert_eq!(right.snapshot().total_declared_bytes, 0);
}

#[test]
fn policy_release_and_duplicate_read_counts_are_independent_of_backing() {
    let domain = SCAccountingDomain::new();
    let mut policy = SCPolicyCounter::new(40);
    let mut charge = domain.charge(40, SCAllocationClass::Resident).unwrap();
    policy.try_sub(40).unwrap();
    charge.transition(SCAllocationClass::Retiring);
    assert_eq!(policy.value(), 0);
    assert_eq!(domain.snapshot().retiring_bytes, 40);
    drop(charge);
    assert_eq!(domain.snapshot().total_declared_bytes, 0);

    let charge = domain.charge(40, SCAllocationClass::Resident).unwrap();
    policy.try_add(40).unwrap();
    policy.try_add(40).unwrap();
    assert_eq!(policy.value(), 80);
    assert_eq!(domain.snapshot().resident_bytes, 40);
    drop(charge);
    assert_eq!(domain.snapshot().total_declared_bytes, 0);
    assert_eq!(policy.value(), 80);
}
