use solcache::{
    SC_REFERENCE_PRODUCTION_LIMIT, SC_REFERENCE_PROVIDER_SLOTS, SCAdmission, SCAdmissionError,
    SCCountLimit,
};

#[test]
fn production_and_provider_slots_are_independent_and_lower_limits_preserve_owners() {
    let production = SCAdmission::new(SC_REFERENCE_PRODUCTION_LIMIT);
    let provider = SCAdmission::new(SC_REFERENCE_PROVIDER_SLOTS);
    let production_permits: Vec<_> = (0..16).map(|_| production.try_acquire().unwrap()).collect();
    let provider_permits: Vec<_> = (0..8).map(|_| provider.try_acquire().unwrap()).collect();
    assert_eq!(
        production.try_acquire().unwrap_err(),
        SCAdmissionError::AtCapacity
    );
    assert_eq!(
        provider.try_acquire().unwrap_err(),
        SCAdmissionError::AtCapacity
    );
    drop(provider_permits);
    assert_eq!(provider.snapshot().active, 0);
    assert_eq!(production.snapshot().active, 16);
    production.set_limit(SCCountLimit::new(0));
    assert_eq!(production.snapshot().active, 16);
    drop(production_permits);
    assert_eq!(production.snapshot().active, 0);
    assert!(production.try_acquire().is_err());
    production.set_limit(SCCountLimit::new(1));
    let permit = production.clone().try_acquire().unwrap();
    drop(production);
    drop(permit);
}

#[test]
fn concurrent_admission_never_exceeds_the_domain_limit() {
    use std::sync::{Arc, Barrier};
    use std::thread;

    let admission = SCAdmission::new(SCCountLimit::new(1));
    let barrier = Arc::new(Barrier::new(3));
    thread::scope(|scope| {
        let mut workers = Vec::new();
        for _ in 0..2 {
            let admission = admission.clone();
            let barrier = Arc::clone(&barrier);
            workers.push(scope.spawn(move || {
                barrier.wait();
                let permit = admission.try_acquire();
                barrier.wait();
                barrier.wait();
                let accepted = permit.is_ok();
                drop(permit);
                accepted
            }));
        }
        barrier.wait();
        barrier.wait();
        let active = admission.snapshot().active;
        // Release workers before asserting so a failure cannot deadlock scope exit.
        barrier.wait();
        let accepted = workers
            .into_iter()
            .map(|worker| usize::from(worker.join().unwrap()))
            .sum::<usize>();
        assert_eq!(active, 1);
        assert_eq!(accepted, 1);
    });
    assert_eq!(admission.snapshot().active, 0);
}
