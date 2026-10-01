use std::cell::Cell;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;

use solcache::{
    SCAccountingDomain, SCAdmission, SCAllocationClass, SCBacking, SCCache, SCCountLimit,
    SCProduction, SCProductionClaim, SCProductionMap, SCProductionPhase, SCProductionStart,
    SCProviderDecision, SCStartRejectionReason, SCSubmissionOutcome,
};

#[test]
fn shared_rejection_preserves_demand_and_explicit_retry_returns_unused_inputs() {
    let mut cache = SCCache::<_, ()>::new();
    let identity = cache.ensure("key");
    let budget = SCAdmission::new(SCCountLimit::new(1));
    let mut map = SCProductionMap::<_, u8, ()>::new(budget.clone());
    let first = map.interest(identity.clone(), 0, 10);
    let second = map.interest(identity.clone(), 0, 20);
    let SCProductionStart::Started(attempt) = map.begin_shared(&identity, Box::new(4)).unwrap()
    else {
        panic!()
    };
    let SCProductionStart::Joined {
        production,
        unused_input,
    } = map.begin_shared(&identity, Box::new(5)).unwrap()
    else {
        panic!()
    };
    assert_eq!(*unused_input, 5);
    assert_eq!(budget.snapshot().active, 1);
    drop(first);
    let demand = map.demand(&identity).unwrap();
    assert_eq!(demand.snapshot().consumers(), 1);
    assert_eq!(demand.snapshot().urgency(), Some(20));
    let SCSubmissionOutcome::Rejected(rejection) =
        attempt.submission.submit(SCProviderDecision::Rejected)
    else {
        panic!()
    };
    assert_eq!(rejection.into_parts().0.map(|value| *value), Some(4));
    assert!(matches!(production.claim(), SCProductionClaim::Rejected));
    assert_eq!(budget.snapshot().active, 0);
    assert_eq!(demand.snapshot().consumers(), 1);
    let SCProductionStart::Started(retry) = map.begin_shared(&identity, ()).unwrap() else {
        panic!()
    };
    retry.submission.submit(|mut root| {
        root.complete(Ok(9)).unwrap();
        drop(root);
        SCProviderDecision::Accepted
    });
    // Completed but unclaimed results continue to coalesce without reserving.
    assert!(matches!(
        map.begin_shared(&identity, ()).unwrap(),
        SCProductionStart::Joined { .. }
    ));
    assert!(matches!(
        retry.production.claim(),
        SCProductionClaim::Ready(Ok(9))
    ));
    let SCProductionStart::Started(next) = map.begin_shared(&identity, ()).unwrap() else {
        panic!()
    };
    drop(next.submission);
    assert!(matches!(
        next.production.claim(),
        SCProductionClaim::Aborted
    ));
    drop(second);
    let error = match map.begin_shared(&identity, 123) {
        Err(error) => error,
        Ok(_) => panic!(),
    };
    assert_eq!(error.reason(), SCStartRejectionReason::NoDemand);
    assert_eq!(error.into_inputs(), 123);
}

#[test]
fn consumed_rejection_holds_production_allowance_but_not_provider_slot_or_demand() {
    let mut cache = SCCache::<_, ()>::new();
    let identity = cache.ensure("key");
    let budget = SCAdmission::new(SCCountLimit::new(1));
    let providers = SCAdmission::new(SCCountLimit::new(1));
    let accounting = SCAccountingDomain::new();
    let input =
        SCBacking::try_new(vec![0u8; 8], &accounting, 8, SCAllocationClass::Temporary).unwrap();
    let mut map = SCProductionMap::<_, (), ()>::new(budget.clone());
    let consumer = map.interest(identity.clone(), 0, 1);
    let SCProductionStart::Started(attempt) = map.begin_shared(&identity, input).unwrap() else {
        panic!()
    };
    let mut consumed = None;
    let slot = providers.try_acquire().unwrap();
    let SCSubmissionOutcome::Rejected(rejection) = attempt.submission.submit(|mut root| {
        consumed = root.consume_inputs().unwrap();
        drop(slot);
        SCProviderDecision::Rejected(root)
    }) else {
        panic!()
    };
    assert!(rejection.into_parts().0.is_none());
    assert_eq!(providers.snapshot().active, 0);
    assert_eq!(budget.snapshot().active, 1);
    assert_eq!(accounting.snapshot().temporary_bytes, 8);
    let error = match map.begin_shared(&identity, 41) {
        Err(error) => error,
        Ok(_) => panic!(),
    };
    assert_eq!(error.reason(), SCStartRejectionReason::AtCapacity);
    assert_eq!(error.into_inputs(), 41);
    assert_eq!(
        map.current(&identity).unwrap().snapshot().phase,
        SCProductionPhase::Rejected
    );
    drop(consumer);
    assert!(map.remove(&identity));
    drop(map);
    assert_eq!(budget.snapshot().active, 1);
    drop(consumed);
    assert_eq!(budget.snapshot().active, 0);
    assert_eq!(accounting.snapshot().temporary_bytes, 0);
}

#[test]
fn independent_candidates_and_recreated_identities_do_not_replace_shared_work() {
    let mut cache = SCCache::<_, ()>::new();
    let identity = cache.ensure("key");
    let budget = SCAdmission::new(SCCountLimit::new(3));
    let mut map = SCProductionMap::<_, u8, ()>::new(budget.clone());
    let old_interest = map.interest(identity.clone(), 0, 1);
    let SCProductionStart::Started(shared) = map.begin_shared(&identity, ()).unwrap() else {
        panic!()
    };
    let candidate = map.begin_candidate(&identity, ()).unwrap();
    candidate.submission.submit(|mut root| {
        root.complete(Ok(7)).unwrap();
        SCProviderDecision::Accepted
    });
    assert!(matches!(
        candidate.production.claim(),
        SCProductionClaim::Ready(Ok(7))
    ));
    assert_eq!(
        map.current(&identity).unwrap().snapshot().phase,
        SCProductionPhase::Prepared
    );
    cache.remove(&"key");
    let replacement = cache.ensure("key");
    assert!(map.current(&replacement).is_none());
    assert!(matches!(
        map.begin_candidate(&replacement, ())
            .err()
            .unwrap()
            .reason(),
        SCStartRejectionReason::NoDemand
    ));
    assert!(map.remove(&identity));
    let new_interest = map.interest(identity.clone(), 0, 5);
    drop(old_interest);
    assert_eq!(map.demand(&identity).unwrap().snapshot().consumers(), 1);
    // Old accepted discovery can still settle after association removal.
    let mut child = None;
    shared.submission.submit(|mut root| {
        child = Some(root.child(()).unwrap());
        root.complete(Ok(3)).unwrap();
        SCProviderDecision::Accepted
    });
    drop(new_interest);
    drop(map);
    assert!(matches!(
        shared.production.claim(),
        SCProductionClaim::Pending
    ));
    assert_eq!(budget.snapshot().active, 1);
    drop(child);
    assert_eq!(budget.snapshot().active, 0);
    assert!(matches!(
        shared.production.claim(),
        SCProductionClaim::Ready(Ok(3))
    ));
}

#[test]
fn abort_with_escaped_producer_drops_staged_output_outside_lock_and_retains_allowance() {
    struct Output {
        observer: SCProduction<Output, ()>,
        budget: SCAdmission,
        drops: Rc<Cell<usize>>,
    }
    impl Drop for Output {
        fn drop(&mut self) {
            assert_eq!(self.observer.snapshot().phase, SCProductionPhase::Aborted);
            assert_eq!(self.budget.snapshot().active, 1);
            self.drops.set(self.drops.get() + 1);
        }
    }
    let budget = SCAdmission::new(SCCountLimit::new(1));
    let (production, submission) =
        SCProduction::<Output, ()>::prepare_with_permit((), budget.try_acquire().unwrap());
    let mut escaped = None;
    let drops = Rc::new(Cell::new(0));
    let panic = catch_unwind(AssertUnwindSafe(|| {
        submission.submit(|mut root| {
            root.complete(Ok(Output {
                observer: production.clone(),
                budget: budget.clone(),
                drops: Rc::clone(&drops),
            }))
            .unwrap();
            escaped = Some(root);
            panic!("provider panic after escaped root");
        })
    }))
    .err()
    .expect("provider must panic");
    assert_eq!(
        panic.downcast_ref::<&str>(),
        Some(&"provider panic after escaped root")
    );
    assert_eq!(
        drops.get(),
        1,
        "aborting must actually destroy the staged output"
    );
    assert_eq!(budget.snapshot().active, 1);
    drop(escaped);
    assert_eq!(budget.snapshot().active, 0);
    assert!(matches!(production.claim(), SCProductionClaim::Aborted));
}

#[test]
fn abandoned_attempt_retries_without_reusing_old_observer() {
    let mut cache = SCCache::<_, ()>::new();
    let identity = cache.ensure(1);
    let budget = SCAdmission::new(SCCountLimit::new(1));
    let mut map = SCProductionMap::<_, (), ()>::new(budget.clone());
    let _interest = map.interest(identity.clone(), 0, 0);
    let SCProductionStart::Started(first) = map.begin_shared(&identity, ()).unwrap() else {
        panic!()
    };
    first.submission.submit(|_| SCProviderDecision::Accepted);
    assert_eq!(budget.snapshot().active, 0);
    let SCProductionStart::Started(second) = map.begin_shared(&identity, ()).unwrap() else {
        panic!()
    };
    assert!(matches!(
        first.production.claim(),
        SCProductionClaim::Abandoned
    ));
    assert!(matches!(
        second.production.claim(),
        SCProductionClaim::Pending
    ));
    drop(second.submission);
    assert_eq!(budget.snapshot().active, 0);
}

#[test]
fn unsubmitted_input_panic_aborts_and_releases_allowance_after_cleanup() {
    struct Input {
        budget: SCAdmission,
    }
    impl Drop for Input {
        fn drop(&mut self) {
            assert_eq!(self.budget.snapshot().active, 1);
            panic!("unsubmitted input cleanup");
        }
    }
    let budget = SCAdmission::new(SCCountLimit::new(1));
    let (production, submission) = SCProduction::<(), ()>::prepare_with_permit(
        Input {
            budget: budget.clone(),
        },
        budget.try_acquire().unwrap(),
    );
    let panic = catch_unwind(AssertUnwindSafe(|| drop(submission))).unwrap_err();
    assert_eq!(
        panic.downcast_ref::<&str>(),
        Some(&"unsubmitted input cleanup")
    );
    assert_eq!(budget.snapshot().active, 0);
    assert!(matches!(production.claim(), SCProductionClaim::Aborted));
}
