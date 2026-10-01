use solcache::{
    SC_REFERENCE_PAYLOAD_RETENTION, SC_REFERENCE_PRODUCTION_LIMIT, SC_REFERENCE_PROVIDER_SLOTS,
    SC_REFERENCE_RANGE_ENTRIES, SCByteLimit, SCCountLimit, SCPolicyCounter, SCPolicyCounterError,
};

#[test]
fn count_admission_includes_existing_work_and_preserves_over_limit_state() {
    let limit = SC_REFERENCE_PRODUCTION_LIMIT;
    assert_eq!(limit.remaining(5), 11);
    assert!(limit.allows(15));
    assert!(!limit.allows(16));
    assert!(!limit.allows(17));
    assert_eq!(limit.remaining(usize::MAX), 0);
    assert!(!SCCountLimit::new(0).allows(0));
    assert_eq!(SCCountLimit::new(0).remaining(0), 0);
    assert_eq!(SC_REFERENCE_PROVIDER_SLOTS.limit(), 8);
    assert_eq!(SC_REFERENCE_RANGE_ENTRIES.limit(), 16);
}

#[test]
fn byte_thresholds_keep_equality_and_zero_semantics_distinct() {
    for (limit, usage, reached) in [
        (SCByteLimit::Disabled, u64::MAX, false),
        (SCByteLimit::AtOrAbove(0), 0, true),
        (SCByteLimit::Above(0), 0, false),
        (SCByteLimit::Above(0), 1, true),
        (SCByteLimit::AtOrAbove(10), 9, false),
        (SCByteLimit::AtOrAbove(10), 10, true),
        (SCByteLimit::Above(10), 10, false),
        (SCByteLimit::Above(10), 11, true),
    ] {
        assert_eq!(limit.reached(usage), reached, "{limit:?}, {usage}");
    }
    let reference_bytes = 32 * 1024 * 1024;
    assert!(!SC_REFERENCE_PAYLOAD_RETENTION.reached(reference_bytes - 1));
    assert!(SC_REFERENCE_PAYLOAD_RETENTION.reached(reference_bytes));
}

#[test]
fn policy_arithmetic_is_checked_and_reset_does_not_imply_cleanup() {
    let mut counter = SCPolicyCounter::new(7);
    assert_eq!(counter.try_sub(8), Err(SCPolicyCounterError::Underflow));
    assert_eq!(counter.value(), 7);
    assert_eq!(
        counter.try_add(u64::MAX),
        Err(SCPolicyCounterError::Overflow)
    );
    assert_eq!(counter.value(), 7);
    counter.try_add(u64::MAX - 7).unwrap();
    counter.try_add(0).unwrap();
    assert_eq!(counter.value(), u64::MAX);
    assert_eq!(counter.clear(), u64::MAX);
    assert_eq!(counter.value(), 0);
    counter.try_sub(0).unwrap();
}

#[test]
fn soft_limit_observation_can_follow_an_insertion_that_crosses_it() {
    let stop = SCByteLimit::Above(10);
    let mut admitted_bytes = SCPolicyCounter::new(9);
    admitted_bytes.try_add(4).unwrap();
    assert!(stop.reached(admitted_bytes.value()));
    assert_eq!(admitted_bytes.value(), 13);
    assert!(!SCByteLimit::Disabled.reached(admitted_bytes.value()));
}
