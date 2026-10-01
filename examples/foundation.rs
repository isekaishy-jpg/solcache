use solcache::{SCAccountingDomain, SCAllocationClass, SCCleanupContext, SCPolicyCounter};

fn main() {
    let domain = SCAccountingDomain::new();
    let cleanup = SCCleanupContext::new();
    let bytes = vec![10_u8, 20, 30, 40];
    let declared_capacity = bytes.capacity() as u64;
    let owner = cleanup
        .try_acquire(
            bytes,
            &domain,
            declared_capacity,
            SCAllocationClass::Resident,
        )
        .expect("small declared capacity fits the empty domain");
    let mut retention = SCPolicyCounter::new(declared_capacity);

    let consumer = owner.clone();
    assert_eq!(consumer.view().map(|bytes| &bytes[1..3]).get(), &[20, 30]);
    drop(owner);
    retention.clear();
    // Removing a policy charge does not invalidate an existing consumer.
    assert_eq!(domain.snapshot().resident_bytes, declared_capacity);

    std::thread::scope(|scope| {
        scope.spawn(move || {
            assert_eq!(consumer.view().get()[0], 10);
            drop(consumer);
        });
    });
    // The final off-thread owner has retired; cleanup is still this thread's job.
    assert_eq!(domain.snapshot().retiring_bytes, declared_capacity);
    assert_eq!(cleanup.drain(), 1);
    assert_eq!(domain.snapshot().total_declared_bytes, 0);
    println!("Retained backing survived handoff and was drained on its owning thread.");
}
