use solcache::{
    SCAccountingDomain, SCAllocationClass, SCBacking, SCCleanupContext, SCSourceBuffers,
    SCSourceInsert, SCSourceRejectionReason,
};
use std::panic::{AssertUnwindSafe, catch_unwind};

fn bytes(
    domain: &SCAccountingDomain,
    value: &[u8],
    capacity: usize,
) -> SCBacking<'static, Vec<u8>> {
    let mut buffer = Vec::with_capacity(capacity);
    buffer.extend_from_slice(value);
    let charge = domain
        .charge(buffer.capacity() as u64, SCAllocationClass::Resident)
        .unwrap();
    SCBacking::from_charged(buffer, charge)
}

#[test]
fn duplicates_charge_reads_while_take_transfers_first_backing_and_full_capacity() {
    let domain = SCAccountingDomain::new();
    let mut store = SCSourceBuffers::new(6);
    let first = bytes(&domain, b"abc", 32);
    let capacity = first.declared_bytes();
    store.insert_read(7, first, 3).unwrap();
    let duplicate = bytes(&domain, b"abc", 64);
    let duplicate_capacity = duplicate.declared_bytes();
    let SCSourceInsert::Duplicate(unused) = store.insert_read(7, duplicate, 3).unwrap() else {
        panic!("duplicate replaced original");
    };
    assert_eq!(store.snapshot().read_bytes, 6);
    assert!(store.may_read(), "equality must permit another read");
    assert_eq!(
        domain.snapshot().resident_bytes,
        capacity + duplicate_capacity
    );
    drop(unused);
    assert_eq!(domain.snapshot().resident_bytes, capacity);
    store.insert_read(8, bytes(&domain, b"d", 1), 1).unwrap();
    assert!(
        !store.may_read(),
        "crossing read is admitted before stopping"
    );
    assert_eq!(store.snapshot().retained_entries, 2);
    let taken = store.take(7).unwrap();
    assert_eq!(taken.view().get().as_slice(), b"abc");
    assert_eq!(taken.declared_bytes(), capacity);
    assert_eq!(
        store.snapshot().read_bytes,
        4,
        "duplicate read remains charged"
    );
    assert!(!store.may_read(), "take cannot restart a stopped builder");
    let rejected = store
        .insert_read(9, bytes(&domain, b"e", 1), 1)
        .unwrap_err();
    assert_eq!(rejected.reason(), SCSourceRejectionReason::SoftLimitReached);
    drop(rejected);
    store.close();
    assert_eq!(store.snapshot().read_bytes, 0);
    assert!(!store.snapshot().enabled);
    assert_eq!(domain.snapshot().resident_bytes, capacity);
    assert_eq!(taken.view().get().as_slice(), b"abc");
    drop(taken);
    assert_eq!(domain.snapshot().total_declared_bytes, 0);
}

#[test]
fn disabled_limit_and_rejection_preserve_owners_and_state() {
    let domain = SCAccountingDomain::new();
    let mut store = SCSourceBuffers::new(0);
    store.insert_read(1, bytes(&domain, b"1234", 8), 4).unwrap();
    assert!(store.may_read());
    for (size, class, reason) in [
        (
            0,
            SCAllocationClass::Resident,
            SCSourceRejectionReason::EmptyRead,
        ),
        (
            1,
            SCAllocationClass::Temporary,
            SCSourceRejectionReason::InvalidClass,
        ),
    ] {
        let mut input = bytes(&domain, b"a", 4);
        assert!(input.try_transition(class));
        let before = store.snapshot();
        let error = store.insert_read(2, input, size).unwrap_err();
        assert_eq!(error.reason(), reason);
        assert_eq!(store.snapshot(), before);
        let (input, _) = error.into_parts();
        assert_eq!(input.view().get().as_slice(), b"a");
        assert_eq!(input.allocation_class(), class);
    }
    store.close();
    let before = store.snapshot();
    let error = store
        .insert_read(1, bytes(&domain, b"b", 1), 1)
        .unwrap_err();
    assert_eq!(error.reason(), SCSourceRejectionReason::Closed);
    assert_eq!(store.snapshot(), before);
}

#[test]
fn policy_overflow_is_checked_independently_of_physical_storage() {
    // Synthetic policy-unit boundary, deliberately not a physical-memory test.
    let domain = SCAccountingDomain::new();
    let mut store = SCSourceBuffers::new(0);
    store
        .insert_read(1, bytes(&domain, b"a", 1), u64::MAX)
        .unwrap();
    let before = store.snapshot();
    let error = store
        .insert_read(1, bytes(&domain, b"a", 1), 1)
        .unwrap_err();
    assert_eq!(error.reason(), SCSourceRejectionReason::CounterOverflow);
    assert_eq!(store.snapshot(), before);
    drop(error);
    drop(store.take(1).unwrap());
    assert_eq!(store.snapshot().read_bytes, 0);
    assert_eq!(domain.snapshot().total_declared_bytes, 0);
}

#[test]
fn cleanup_state_commits_before_panicking_payload_and_deferred_cleanup_stays_charged() {
    struct Payload {
        domain: SCAccountingDomain,
        panic: bool,
    }
    impl Drop for Payload {
        fn drop(&mut self) {
            assert!(self.domain.snapshot().retiring_bytes >= 8);
            if self.panic {
                std::panic::panic_any("source cleanup probe");
            }
        }
    }
    let domain = SCAccountingDomain::new();
    let mut store = SCSourceBuffers::new(0);
    store
        .insert_read(
            1,
            SCBacking::try_new(
                Payload {
                    domain: domain.clone(),
                    panic: true,
                },
                &domain,
                8,
                SCAllocationClass::Resident,
            )
            .unwrap(),
            1,
        )
        .unwrap();
    let panic = catch_unwind(AssertUnwindSafe(|| store.close())).unwrap_err();
    assert_eq!(panic.downcast_ref::<&str>(), Some(&"source cleanup probe"));
    assert!(store.snapshot().closed);
    assert_eq!(store.snapshot().retained_entries, 0);
    assert_eq!(store.snapshot().read_bytes, 0);
    assert_eq!(domain.snapshot().total_declared_bytes, 0);

    let context = SCCleanupContext::new();
    let mut store = SCSourceBuffers::new(0);
    store
        .insert_read(
            1,
            context
                .try_acquire(
                    Payload {
                        domain: domain.clone(),
                        panic: false,
                    },
                    &domain,
                    8,
                    SCAllocationClass::Resident,
                )
                .unwrap(),
            1,
        )
        .unwrap();
    store.close();
    assert_eq!(domain.snapshot().retiring_bytes, 8);
    assert_eq!(context.drain(), 1);
    assert_eq!(domain.snapshot().total_declared_bytes, 0);
}
