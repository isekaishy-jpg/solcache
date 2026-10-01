use solcache::{
    SCAccountingDomain, SCAllocationClass, SCByteLimit, SCCache, SCCleanupContext, SCLookup,
};

fn main() {
    let domain = SCAccountingDomain::new();
    let cleanup = SCCleanupContext::new();
    let mut cache = SCCache::<&str, Vec<u8>>::new();
    let bytes = vec![10_u8, 20, 30, 40];
    let cost = bytes.capacity() as u64;
    let backing = cleanup
        .try_acquire(bytes, &domain, cost, SCAllocationClass::Resident)
        .unwrap();
    let (identity, _) = cache.install("source", backing, cost).unwrap();

    let SCLookup::Ready(consumer) = cache.share(&"source") else {
        panic!("installed source must be ready");
    };
    // This example explicitly chooses a tiny threshold to demonstrate pressure.
    let threshold = SCByteLimit::AtOrAbove(cost);
    let blocked = cache.maintain(threshold, 1, false);
    assert_eq!(blocked.pinned_encountered, 1);
    assert!(blocked.pressure_after);
    assert_eq!(consumer.view().map(|bytes| &bytes[1..3]).get(), &[20, 30]);

    drop(consumer);
    let collected = cache.maintain(threshold, 1, false);
    assert_eq!(collected.evicted_payloads, 1);
    assert_eq!(cache.policy_bytes(), 0);
    assert!(cache.contains_identity(&identity));
    assert!(matches!(cache.lookup_identity(&identity), SCLookup::Vacant));
    assert_eq!(domain.snapshot().retiring_bytes, cost);
    assert_eq!(cleanup.drain(), 1);
    assert_eq!(domain.snapshot().total_declared_bytes, 0);
    println!("Collected idle payload; logical identity survived deferred cleanup.");
}
