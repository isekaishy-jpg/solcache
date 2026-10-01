use std::panic::{AssertUnwindSafe, catch_unwind};

use solcache::{
    SC_REFERENCE_PAYLOAD_RETENTION, SCAccountingDomain, SCAllocationClass, SCBacking, SCByteLimit,
    SCCache, SCCleanupContext, SCLookup,
};

fn backing(domain: &SCAccountingDomain, value: u8, declared_bytes: u64) -> SCBacking<'static, u8> {
    SCBacking::try_new(value, domain, declared_bytes, SCAllocationClass::Resident).unwrap()
}

#[test]
fn family_thresholds_preserve_equality_and_stop_at_the_selected_target() {
    let mib = 1024 * 1024;
    for (limit, cost, expected_evictions, expected_remaining) in [
        (SC_REFERENCE_PAYLOAD_RETENTION, 16 * mib, 1, 16 * mib),
        (SCByteLimit::Above(32 * mib), 16 * mib, 0, 32 * mib),
        (SCByteLimit::Above(16 * mib), 16 * mib, 1, 16 * mib),
        (SCByteLimit::Disabled, 16 * mib, 0, 32 * mib),
    ] {
        let domain = SCAccountingDomain::new();
        let mut cache: SCCache<u8, u8> = SCCache::new();
        cache.install(0, backing(&domain, 0, 1), cost).unwrap();
        cache.install(1, backing(&domain, 1, 1), cost).unwrap();
        let report = cache.maintain(limit, usize::MAX, false);
        assert_eq!(report.evicted_payloads, expected_evictions, "{limit:?}");
        assert_eq!(report.policy_bytes_after, expected_remaining, "{limit:?}");
        assert!(!report.pressure_after, "{limit:?}");
        assert_eq!(report.scanned_slots, expected_evictions, "{limit:?}");
    }
}

#[test]
fn shared_payload_revival_blocks_even_forced_collection_until_final_pin_release() {
    let domain = SCAccountingDomain::new();
    let mut cache: SCCache<u8, u8> = SCCache::new();
    let (identity, _) = cache.install(0, backing(&domain, 42, 5), 17).unwrap();
    assert!(matches!(cache.lookup(&0), SCLookup::Ready(_)));
    let SCLookup::Ready(pin) = cache.share(&0) else {
        panic!("installed payload must be shareable");
    };
    let blocked = cache.maintain(SCByteLimit::Above(0), 1, true);
    assert_eq!(blocked.pinned_encountered, 1);
    assert_eq!(blocked.evicted_payloads, 0);
    assert!(blocked.pressure_after);
    assert_eq!(*pin.view().get(), 42);
    drop(pin);
    let collected = cache.maintain(SCByteLimit::Above(0), 1, false);
    assert_eq!(collected.evicted_payloads, 1);
    assert_eq!(collected.removed_policy_bytes, 17);
    assert!(cache.contains_identity(&identity));
    assert!(matches!(cache.lookup_identity(&identity), SCLookup::Vacant));
    assert_eq!(domain.snapshot().total_declared_bytes, 0);
    let (revived, _) = cache.install(0, backing(&domain, 43, 6), 19).unwrap();
    assert_eq!(revived, identity);
    assert!(matches!(
        cache.lookup_identity(&identity),
        SCLookup::Ready(_)
    ));
}

#[test]
fn audio_policy_removal_precedes_deferred_physical_cleanup() {
    let domain = SCAccountingDomain::new();
    let context = SCCleanupContext::new();
    let mut cache: SCCache<u8, u8> = SCCache::new();
    let owner = context
        .try_acquire(7, &domain, 103, SCAllocationClass::Resident)
        .unwrap();
    let (identity, _) = cache.install(0, owner, 29).unwrap();
    let report = cache.maintain(SCByteLimit::Above(20), 1, false);
    assert_eq!(report.removed_policy_bytes, 29);
    assert_eq!(cache.policy_bytes(), 0);
    assert!(!report.pressure_after);
    assert!(matches!(cache.lookup_identity(&identity), SCLookup::Vacant));
    assert_eq!(context.pending(), 1);
    let retired = domain.snapshot();
    assert_eq!(retired.resident_bytes, 0);
    assert_eq!(retired.retiring_bytes, 103);
    assert_eq!(retired.total_declared_bytes, 103);
    assert_eq!(context.drain(), 1);
    assert_eq!(domain.snapshot().total_declared_bytes, 0);
}

#[test]
fn bounded_scans_count_holes_unavailable_and_pinned_slots_and_resume_without_starvation() {
    let domain = SCAccountingDomain::new();
    let mut cache: SCCache<u8, u8, &'static str> = SCCache::new();
    for key in 0..5 {
        cache.ensure(key);
    }
    drop(cache.remove(&0));
    cache.fail(2, "unavailable");
    cache.install(3, backing(&domain, 3, 1), 3).unwrap();
    cache.install(4, backing(&domain, 4, 1), 4).unwrap();
    let SCLookup::Ready(pin) = cache.share(&3) else {
        panic!("installed payload must be shareable");
    };
    let zero = cache.maintain(SCByteLimit::Above(0), 0, true);
    assert_eq!(zero.scanned_slots, 0);
    assert_eq!(zero.next_slot, 0);
    assert!(!zero.completed_cycle);
    for (index, entries, pinned, evicted) in [
        (0, 0, 0, 0),
        (1, 1, 0, 0),
        (2, 1, 0, 0),
        (3, 1, 1, 0),
        (4, 1, 0, 1),
    ] {
        let report = cache.maintain(SCByteLimit::Above(0), 1, true);
        assert_eq!(report.scanned_slots, 1);
        assert_eq!(report.visited_entries, entries);
        assert_eq!(report.pinned_encountered, pinned);
        assert_eq!(report.evicted_payloads, evicted);
        assert_eq!(report.next_slot, (index + 1) % 5);
        assert_eq!(report.slot_count, 5);
        assert!(!report.completed_cycle);
    }
    drop(pin);
    let cycle = cache.maintain(SCByteLimit::Disabled, usize::MAX, true);
    assert_eq!(cycle.scanned_slots, 5);
    assert_eq!(cycle.visited_entries, 4);
    assert_eq!(cycle.evicted_payloads, 1);
    assert_eq!(cycle.pinned_encountered, 0);
    assert!(cycle.completed_cycle);
    assert_eq!(cycle.policy_bytes_after, 0);
    let mut empty: SCCache<u8, u8> = SCCache::new();
    let empty_report = empty.maintain(SCByteLimit::AtOrAbove(0), 0, true);
    assert!(empty_report.completed_cycle);
    assert!(empty_report.pressure_after);
    assert_eq!(empty_report.scanned_slots, 0);
}

#[test]
fn aliased_backing_conservatively_pins_entries_with_independent_policy_contributions() {
    let domain = SCAccountingDomain::new();
    let mut cache: SCCache<u8, u8> = SCCache::new();
    let owner = backing(&domain, 8, 3);
    cache.install(0, owner.clone(), 7).unwrap();
    cache.install(1, owner, 11).unwrap();
    assert_eq!(cache.policy_bytes(), 18);
    assert_eq!(domain.snapshot().total_declared_bytes, 3);
    let report = cache.maintain(SCByteLimit::Above(0), 2, true);
    assert_eq!(report.pinned_encountered, 2);
    assert_eq!(report.evicted_payloads, 0);
    drop(cache.remove(&0));
    assert_eq!(cache.policy_bytes(), 11);
    assert_eq!(domain.snapshot().total_declared_bytes, 3);
    let report = cache.maintain(SCByteLimit::Above(0), 2, true);
    assert_eq!(report.removed_policy_bytes, 11);
    assert_eq!(report.evicted_payloads, 1);
    assert_eq!(domain.snapshot().total_declared_bytes, 0);
}

struct PanickingPayload(bool);

impl Drop for PanickingPayload {
    fn drop(&mut self) {
        assert!(!self.0, "deliberate payload destruction panic");
    }
}

#[test]
fn destructor_panic_preserves_committed_detachment_accounting_and_scan_progress() {
    let domain = SCAccountingDomain::new();
    let mut cache: SCCache<u8, PanickingPayload> = SCCache::new();
    for (key, panic, declared, policy) in [(0, true, 3, 5), (1, false, 7, 11)] {
        let owner = SCBacking::try_new(
            PanickingPayload(panic),
            &domain,
            declared,
            SCAllocationClass::Resident,
        )
        .unwrap();
        cache.install(key, owner, policy).unwrap();
    }
    let identity = cache.identity(&0).unwrap();
    assert!(
        catch_unwind(AssertUnwindSafe(|| {
            cache.maintain(SCByteLimit::Disabled, 1, true);
        }))
        .is_err()
    );
    assert!(matches!(cache.lookup_identity(&identity), SCLookup::Vacant));
    assert_eq!(cache.policy_bytes(), 11);
    assert_eq!(domain.snapshot().total_declared_bytes, 7);
    let report = cache.maintain(SCByteLimit::Disabled, 1, true);
    assert_eq!(report.evicted_payloads, 1);
    assert_eq!(report.removed_policy_bytes, 11);
    assert_eq!(report.next_slot, 0);
    assert_eq!(cache.policy_bytes(), 0);
    assert_eq!(domain.snapshot().total_declared_bytes, 0);
}
