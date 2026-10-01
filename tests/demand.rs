use std::sync::{Arc, Barrier};
use std::thread;

use solcache::{SCDemand, SCUrgencyUpdate};

#[test]
fn independent_consumers_detach_without_erasing_remaining_interest() {
    let demand = SCDemand::new();
    let observer = demand.clone();
    assert_eq!(observer.snapshot().consumers(), 0);
    assert_eq!(observer.snapshot().urgency(), None);
    let low = demand.attach(0, 0);
    let high = observer.attach(9, 12);
    assert_eq!(demand.snapshot().consumers(), 2);
    assert_eq!(demand.snapshot().urgency(), Some(12));
    drop(observer);
    assert_eq!(demand.snapshot().consumers(), 2);
    high.detach();
    assert_eq!(demand.snapshot().consumers(), 1);
    assert_eq!(demand.snapshot().urgency(), Some(0));
    drop(low);
    assert_eq!(demand.snapshot().consumers(), 0);
    assert_eq!(demand.snapshot().urgency(), None);
}

#[test]
fn only_strictly_newer_revisions_apply_including_decreases_and_revision_exhaustion() {
    let demand = SCDemand::new();
    let handle = demand.attach(7, 20);
    for (revision, urgency, expected, aggregate) in [
        (6, 30, SCUrgencyUpdate::Obsolete, 20),
        (7, 40, SCUrgencyUpdate::Obsolete, 20),
        (8, 3, SCUrgencyUpdate::Applied, 3),
        (7, 90, SCUrgencyUpdate::Obsolete, 3),
        (u64::MAX, 5, SCUrgencyUpdate::Applied, 5),
        (u64::MAX, 99, SCUrgencyUpdate::Obsolete, 5),
        (0, 100, SCUrgencyUpdate::Obsolete, 5),
    ] {
        assert_eq!(handle.update(revision, urgency), expected);
        assert_eq!(demand.snapshot().urgency(), Some(aggregate));
    }
    let exhausted = demand.attach(u64::MAX, 0);
    assert_eq!(exhausted.update(u64::MAX, 42), SCUrgencyUpdate::Obsolete);
}

#[test]
fn retained_identities_do_not_keep_interest_or_acquire_foreign_or_recreated_authority() {
    let demand = SCDemand::new();
    let foreign = SCDemand::new();
    let handle = demand.attach(0, 4);
    let identity = handle.identity();
    assert_eq!(foreign.update(&identity, 1, 90), SCUrgencyUpdate::Detached);
    assert_eq!(foreign.snapshot().consumers(), 0);
    drop(handle);
    assert_eq!(demand.snapshot().consumers(), 0);
    let fresh = demand.attach(0, 2);
    assert_ne!(identity, fresh.identity());
    assert_eq!(
        demand.update(&identity, u64::MAX, 90),
        SCUrgencyUpdate::Detached
    );
    assert_eq!(demand.snapshot().urgency(), Some(2));
}

#[test]
fn snapshot_tokens_detect_changes_even_when_aggregate_urgency_is_unchanged() {
    let demand = SCDemand::new();
    let foreign = SCDemand::new();
    let empty = demand.snapshot();
    assert!(demand.is_current(&empty));
    assert!(!foreign.is_current(&empty));
    let first = demand.attach(0, 8);
    assert!(!demand.is_current(&empty));
    let attached = demand.snapshot();
    let copy = attached.clone();
    assert!(demand.is_current(&copy));
    assert_eq!(first.update(0, 99), SCUrgencyUpdate::Obsolete);
    assert!(demand.is_current(&attached));
    assert_eq!(first.update(1, 8), SCUrgencyUpdate::Applied);
    assert!(!demand.is_current(&attached));
    let updated = demand.snapshot();
    let second = demand.attach(0, 8);
    let stale_identity = second.identity();
    assert!(!demand.is_current(&updated));
    let both = demand.snapshot();
    second.detach();
    assert!(!demand.is_current(&both));
    assert_eq!(demand.snapshot().urgency(), Some(8));
    let after_detach = demand.snapshot();
    assert_eq!(
        demand.update(&stale_identity, 1, 100),
        SCUrgencyUpdate::Detached
    );
    assert!(demand.is_current(&after_detach));
}

#[test]
fn delayed_concurrent_commands_cannot_restore_obsolete_urgency() {
    let demand = SCDemand::new();
    let handle = demand.attach(0, 50);
    let identity = handle.identity();
    let newer_applied = Arc::new(Barrier::new(2));
    let old_group = demand.clone();
    let old_identity = identity.clone();
    let old_barrier = Arc::clone(&newer_applied);
    let old = thread::spawn(move || {
        old_barrier.wait();
        old_group.update(&old_identity, 1, 100)
    });
    let new_group = demand.clone();
    let newer = thread::spawn(move || {
        let outcome = new_group.update(&identity, 2, 3);
        newer_applied.wait();
        outcome
    });
    assert_eq!(newer.join().unwrap(), SCUrgencyUpdate::Applied);
    assert_eq!(old.join().unwrap(), SCUrgencyUpdate::Obsolete);
    assert_eq!(demand.snapshot().urgency(), Some(3));
    assert_eq!(demand.snapshot().consumers(), 1);
}
