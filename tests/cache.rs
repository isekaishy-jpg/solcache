use std::hash::{Hash, Hasher};

use solcache::{
    SCAccountingDomain, SCAllocationClass, SCBacking, SCCache, SCInstallErrorReason, SCLookup,
    SCStoredPayload,
};

#[derive(Eq, PartialEq)]
struct Collision(u8);
impl Hash for Collision {
    fn hash<H: Hasher>(&self, state: &mut H) {
        0u8.hash(state);
    }
}

fn backing(value: u8, domain: &SCAccountingDomain) -> SCBacking<'static, u8> {
    SCBacking::try_new(value, domain, 1, SCAllocationClass::Resident).unwrap()
}

#[test]
fn rehash_after_removal_does_not_invoke_stored_key_hash_or_lose_membership() {
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct Key {
        value: u16,
        reject_stored_hash: Arc<AtomicBool>,
    }
    impl Hash for Key {
        fn hash<H: Hasher>(&self, state: &mut H) {
            assert!(
                self.value >= 224 || !self.reject_stored_hash.load(Ordering::Relaxed),
                "stored-key hash invoked during index maintenance"
            );
            0_u8.hash(state);
        }
    }
    impl PartialEq for Key {
        fn eq(&self, other: &Self) -> bool {
            self.value == other.value
        }
    }
    impl Eq for Key {}

    let armed = Arc::new(AtomicBool::new(false));
    let key = |value| Key {
        value,
        reject_stored_hash: Arc::clone(&armed),
    };
    let domain = SCAccountingDomain::new();
    let mut cache = SCCache::<Key, u8>::new();
    let mut surviving = Vec::new();
    for value in 0..224 {
        let (identity, _) = cache.install(key(value), backing(1, &domain), 1).unwrap();
        if !(1..114).contains(&value) {
            surviving.push((value, identity));
        }
    }
    for value in 1..114 {
        drop(cache.remove(&key(value)));
    }
    assert_eq!(cache.len(), 111);
    armed.store(true, Ordering::Relaxed);
    let result = catch_unwind(AssertUnwindSafe(|| cache.ensure(key(225))));
    armed.store(false, Ordering::Relaxed);
    assert_eq!(cache.policy_bytes(), 111);
    assert_eq!(domain.snapshot().resident_bytes, 111);
    assert!(
        result.is_ok(),
        "index maintenance called stored Hash; {} indexed entries remain",
        cache.len()
    );
    assert_eq!(cache.len(), 112);
    for (value, identity) in surviving {
        assert!(cache.contains_identity(&identity));
        assert!(matches!(
            cache.lookup_identity(&identity),
            SCLookup::Ready(_)
        ));
        assert!(matches!(cache.lookup(&key(value)), SCLookup::Ready(_)));
    }
    assert!(matches!(cache.lookup(&key(0)), SCLookup::Ready(_)));
}

#[test]
fn colliding_keys_survive_removal_in_any_order_and_slot_reuse() {
    for order in [[1, 2, 3], [3, 1, 2], [2, 3, 1]] {
        let domain = SCAccountingDomain::new();
        let mut cache = SCCache::<Collision, u8>::new();
        let mut identities = Vec::new();
        for key in 1..=3 {
            let (identity, _) = cache
                .install(Collision(key), backing(key, &domain), 1)
                .unwrap();
            identities.push(identity);
        }
        let mut removed = Vec::new();
        for key in order {
            drop(cache.remove(&Collision(key)));
            removed.push(key);
            assert!(!cache.contains_identity(&identities[usize::from(key - 1)]));
            assert!(matches!(cache.lookup(&Collision(key)), SCLookup::Absent));
            for survivor in (1..=3).filter(|value| !removed.contains(value)) {
                let SCLookup::Ready(view) = cache.lookup(&Collision(survivor)) else {
                    panic!()
                };
                assert_eq!(*view.get(), survivor);
            }
            // Reuse the freed slot without reviving any removed identity.
            let (replacement, _) = cache
                .install(Collision(key + 10), backing(key + 10, &domain), 1)
                .unwrap();
            assert_ne!(replacement, identities[usize::from(key - 1)]);
            assert_eq!(cache.len(), 3);
            assert_eq!(cache.policy_bytes(), 3);
        }
        for key in 11..=13 {
            let SCLookup::Ready(view) = cache.lookup(&Collision(key)) else {
                panic!()
            };
            assert_eq!(*view.get(), key);
        }
        for key in 11..=13 {
            drop(cache.remove(&Collision(key)));
        }
        assert!(cache.is_empty());
        assert_eq!(cache.policy_bytes(), 0);
        assert_eq!(domain.snapshot().total_declared_bytes, 0);
        cache
            .install(Collision(20), backing(20, &domain), 1)
            .unwrap();
        assert!(matches!(cache.lookup(&Collision(20)), SCLookup::Ready(_)));
    }
}

#[test]
fn semantic_equality_and_explicit_readiness_preserve_membership() {
    let domain = SCAccountingDomain::new();
    let mut cache = SCCache::<Collision, u8, &str>::new();
    assert!(matches!(cache.lookup(&Collision(1)), SCLookup::Absent));
    let id = cache.ensure(Collision(1));
    assert!(matches!(cache.lookup(&Collision(1)), SCLookup::Vacant));
    let (failed_id, old) = cache.fail(Collision(1), "failed");
    assert_eq!(id, failed_id);
    assert!(matches!(old, SCStoredPayload::Vacant));
    assert!(matches!(
        cache.share(&Collision(1)),
        SCLookup::Failed(&"failed")
    ));
    cache.install(Collision(2), backing(2, &domain), 7).unwrap();
    assert!(matches!(
        cache.lookup(&Collision(1)),
        SCLookup::Failed(&"failed")
    ));
    let SCLookup::Ready(view) = cache.lookup(&Collision(2)) else {
        panic!()
    };
    assert_eq!(*view.get(), 2);
    let (recovered, old) = cache.install(Collision(1), backing(1, &domain), 3).unwrap();
    assert_eq!(recovered, id);
    assert!(matches!(old, SCStoredPayload::Failed("failed")));
    assert_eq!(cache.policy_bytes(), 10);
    let (failed_again, displaced) = cache.fail(Collision(1), "replacement failed");
    assert_eq!(failed_again, id);
    assert!(matches!(displaced, SCStoredPayload::Ready(_)));
    assert_eq!(cache.policy_bytes(), 7);
    assert!(matches!(
        cache.take(&Collision(1)),
        SCLookup::Failed(&"replacement failed")
    ));
    let cleared = cache.clear_payload(&Collision(2)).unwrap();
    assert!(matches!(cleared, SCStoredPayload::Ready(_)));
    assert_eq!(cache.policy_bytes(), 0);
    assert!(matches!(cache.lookup(&Collision(2)), SCLookup::Vacant));
}

#[test]
fn replacement_take_and_removal_preserve_escaped_backing_and_reject_stale_identity() {
    let domain = SCAccountingDomain::new();
    let mut cache = SCCache::<u8, u8>::new();
    let (id, _) = cache.install(1, backing(10, &domain), 4).unwrap();
    let escaped = match cache.share(&1) {
        SCLookup::Ready(owner) => owner,
        _ => panic!(),
    };
    let (same, displaced) = cache.install(1, backing(20, &domain), 8).unwrap();
    assert_eq!(id, same);
    drop(displaced);
    assert_eq!(*escaped.view().get(), 10);
    let taken = match cache.take(&1) {
        SCLookup::Ready(owner) => owner,
        _ => panic!(),
    };
    assert_eq!(*taken.view().get(), 20);
    assert_eq!(cache.policy_bytes(), 0);
    assert!(matches!(cache.lookup_identity(&id), SCLookup::Vacant));
    cache.remove(&1).unwrap();
    let fresh = cache.ensure(1);
    assert_ne!(id, fresh);
    assert!(!cache.contains_identity(&id));
    assert!(matches!(cache.lookup_identity(&id), SCLookup::Absent));
    let mut other = SCCache::<u8, u8>::new();
    let foreign = other.ensure(1);
    assert!(!cache.contains_identity(&foreign));
    drop(cache);
    assert_eq!(domain.snapshot().total_declared_bytes, 2);
    drop(taken);
    drop(escaped);
    assert_eq!(domain.snapshot().total_declared_bytes, 0);
}

#[test]
fn rejected_installation_preserves_inputs_and_old_payload() {
    let domain = SCAccountingDomain::new();
    let mut cache = SCCache::<u8, u8>::new();
    let (id, _) = cache.install(1, backing(1, &domain), u64::MAX).unwrap();
    let rejected = cache.install(2, backing(2, &domain), 1).unwrap_err();
    assert_eq!(rejected.reason(), SCInstallErrorReason::PolicyOverflow);
    let (key, owner, cost) = rejected.into_parts();
    assert_eq!((key, cost, *owner.view().get()), (2, 1, 2));
    assert!(matches!(cache.lookup(&2), SCLookup::Absent));
    assert_eq!(cache.policy_bytes(), u64::MAX);
    let temporary = SCBacking::try_new(3, &domain, 1, SCAllocationClass::Temporary).unwrap();
    let rejected = cache.install(1, temporary, 0).unwrap_err();
    assert_eq!(rejected.reason(), SCInstallErrorReason::InvalidClass);
    assert!(cache.contains_identity(&id));
    let SCLookup::Ready(view) = cache.lookup(&1) else {
        panic!()
    };
    assert_eq!(*view.get(), 1);
    // Replacement removes the old cost before overflow checking.
    cache.install(1, owner, 1).unwrap();
    assert_eq!(cache.policy_bytes(), 1);
}

#[test]
fn panicking_key_callbacks_leave_membership_and_policy_coherent() {
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct Key {
        value: u8,
        panic_hash: Arc<AtomicBool>,
        panic_eq: Arc<AtomicBool>,
        panic_drop: bool,
    }
    impl Hash for Key {
        fn hash<H: Hasher>(&self, state: &mut H) {
            assert!(!self.panic_hash.load(Ordering::Relaxed), "hash panic");
            0u8.hash(state);
        }
    }
    impl PartialEq for Key {
        fn eq(&self, other: &Self) -> bool {
            assert!(!self.panic_eq.load(Ordering::Relaxed), "eq panic");
            self.value == other.value
        }
    }
    impl Eq for Key {}
    impl Drop for Key {
        fn drop(&mut self) {
            assert!(!self.panic_drop, "key drop panic");
        }
    }
    let hash = Arc::new(AtomicBool::new(false));
    let eq = Arc::new(AtomicBool::new(false));
    let make = |panic_drop| Key {
        value: 1,
        panic_hash: hash.clone(),
        panic_eq: eq.clone(),
        panic_drop,
    };
    let domain = SCAccountingDomain::new();
    let mut cache = SCCache::<Key, u8>::new();
    let (identity, _) = cache.install(make(false), backing(1, &domain), 5).unwrap();
    for kind in 0..3 {
        hash.store(kind == 0, Ordering::Relaxed);
        eq.store(kind == 1, Ordering::Relaxed);
        assert!(
            catch_unwind(AssertUnwindSafe(|| {
                let _ = cache.install(make(kind == 2), backing(2, &domain), 7);
            }))
            .is_err()
        );
        hash.store(false, Ordering::Relaxed);
        eq.store(false, Ordering::Relaxed);
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.policy_bytes(), 5);
        let SCLookup::Ready(view) = cache.lookup_identity(&identity) else {
            panic!()
        };
        assert_eq!(*view.get(), 1);
        assert_eq!(domain.snapshot().total_declared_bytes, 1);
    }
    let mut removal = SCCache::<Key, u8>::new();
    let (removed_identity, _) = removal.install(make(true), backing(3, &domain), 9).unwrap();
    assert!(catch_unwind(AssertUnwindSafe(|| removal.remove(&make(false)))).is_err());
    assert!(removal.is_empty());
    assert_eq!(removal.policy_bytes(), 0);
    assert!(!removal.contains_identity(&removed_identity));
    assert_eq!(domain.snapshot().total_declared_bytes, 1);
    let fresh = removal.ensure(make(false));
    assert_ne!(fresh, removed_identity);
}
