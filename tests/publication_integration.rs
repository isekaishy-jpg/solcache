use std::sync::{Arc, Barrier, Mutex};

use solcache::{
    SCAccountingDomain, SCAdmission, SCAllocationClass, SCBacking, SCCountLimit, SCDependencies,
    SCDependencyError, SCDependencyScope, SCLookup, SCProduction, SCProductionClaim,
    SCProductionMap, SCProductionStart, SCProviderDecision, SCPublication,
    SCPublicationRejectionReason,
};

fn backing(value: u8, accounting: &SCAccountingDomain) -> SCBacking<'static, u8> {
    SCBacking::try_new(value, accounting, 1, SCAllocationClass::Resident).unwrap()
}

#[test]
fn shared_join_preserves_one_publication_round_through_final_access_and_warm_reuse() {
    let accounting = SCAccountingDomain::new();
    let dependencies = SCDependencies::new();
    let budget = SCAdmission::new(SCCountLimit::new(1));
    let mut publication = SCPublication::<_, u8>::new();
    let identity = publication.ensure("shared");
    let mut map = SCProductionMap::new(budget.clone());
    let _first = map.interest(identity.clone(), 0, 1);
    let _second = map.interest(identity.clone(), 0, 2);
    assert!(matches!(
        publication.lookup(&identity, &dependencies).unwrap(),
        SCLookup::Vacant
    ));
    let SCProductionStart::Started(start) = map.begin_shared(&identity, 7u8).unwrap() else {
        panic!("first request starts work");
    };
    let ticket = publication
        .begin_attempt(&identity, dependencies.snapshot(&[]).unwrap())
        .unwrap();
    let mut child = None;
    start.submission.submit(|mut root| {
        child = Some(root.child(()).unwrap());
        let value = *root.inputs().unwrap() + 1;
        root.complete(Ok(backing(value, &accounting))).unwrap();
        SCProviderDecision::Accepted
    });
    let SCProductionStart::Joined {
        production,
        unused_input,
    } = map.begin_shared(&identity, 99u8).unwrap()
    else {
        panic!("join must not consume another allowance");
    };
    assert_eq!(unused_input, 99);
    assert_eq!(budget.snapshot().active, 1);
    assert!(matches!(production.claim(), SCProductionClaim::Pending));
    drop(child);
    // Completed but unclaimed work still joins; neither join rotates the ticket.
    assert!(matches!(
        map.begin_shared(&identity, 100u8).unwrap(),
        SCProductionStart::Joined {
            unused_input: 100,
            ..
        }
    ));
    let SCProductionClaim::Ready(outcome) = production.claim() else {
        panic!("final access has ended");
    };
    publication
        .publish(&ticket, &dependencies, outcome, 1)
        .unwrap();
    assert!(matches!(start.production.claim(), SCProductionClaim::Taken));
    assert_eq!(budget.snapshot().active, 0);
    // A subsequent request checks the installed result before begin_shared.
    let SCLookup::Ready(reader) = publication.share(&identity, &dependencies).unwrap() else {
        panic!("warm request must reuse publication");
    };
    assert_eq!(*reader.view().get(), 8);
    assert!(map.current(&identity).unwrap().snapshot().outcome_claimed);
    assert_eq!(accounting.snapshot().resident_bytes, 1);
    drop(publication);
    assert_eq!(*reader.view().get(), 8);
    drop(reader);
    assert_eq!(accounting.snapshot().total_declared_bytes, 0);
}

#[test]
fn invalidation_and_new_publication_rounds_do_not_retag_shared_work() {
    for rotate_round in [false, true] {
        let accounting = SCAccountingDomain::new();
        let budget = SCAdmission::new(SCCountLimit::new(1));
        let mut dependencies = SCDependencies::new();
        let source = dependencies.register(SCDependencyScope::Source);
        let mut publication = SCPublication::<_, u8>::new();
        let identity = publication.ensure("changing source");
        let mut map = SCProductionMap::new(budget.clone());
        let old_interest = map.interest(identity.clone(), 0, 1);
        let SCProductionStart::Started(old) = map.begin_shared(&identity, 10u8).unwrap() else {
            panic!();
        };
        let old_ticket = publication
            .begin_attempt(
                &identity,
                dependencies
                    .snapshot(std::slice::from_ref(&source))
                    .unwrap(),
            )
            .unwrap();
        let mut producer = None;
        old.submission.submit(|root| {
            producer = Some(root);
            SCProviderDecision::Accepted
        });
        let current = dependencies.advance(std::slice::from_ref(&source)).unwrap();
        let mut new_ticket = rotate_round.then(|| {
            publication
                .begin_attempt(&identity, current.clone())
                .unwrap()
        });
        // The map compares identity only. Changed input or a new publication
        // round does not magically replace its still-running old production.
        let SCProductionStart::Joined {
            production,
            unused_input,
        } = map.begin_shared(&identity, 20u8).unwrap()
        else {
            panic!("old association is still pending");
        };
        assert_eq!(unused_input, 20);
        let mut producer = producer.take().unwrap();
        producer
            .complete(Ok(backing(*producer.inputs().unwrap(), &accounting)))
            .unwrap();
        drop(producer);
        let SCProductionClaim::Ready(outcome) = production.claim() else {
            panic!();
        };
        let rejected = publication
            .publish(&old_ticket, &dependencies, outcome, 1)
            .unwrap_err();
        assert_eq!(
            rejected.reason(),
            if rotate_round {
                SCPublicationRejectionReason::ObsoleteAttempt
            } else {
                SCPublicationRejectionReason::Dependency(SCDependencyError::Stale)
            }
        );
        let old_backing = rejected.into_parts().0.unwrap();
        assert_eq!(*old_backing.view().get(), 10);
        drop(old_backing);
        assert_eq!(accounting.snapshot().total_declared_bytes, 0);

        assert!(map.remove(&identity));
        let _new_interest = map.interest(identity.clone(), 0, 2);
        drop(old_interest);
        assert_eq!(map.demand(&identity).unwrap().snapshot().consumers(), 1);
        let SCProductionStart::Started(new) = map.begin_shared(&identity, unused_input).unwrap()
        else {
            panic!();
        };
        let ticket = new_ticket
            .take()
            .unwrap_or_else(|| publication.begin_attempt(&identity, current).unwrap());
        new.submission.submit(|mut root| {
            root.complete(Ok(backing(*root.inputs().unwrap(), &accounting)))
                .unwrap();
            SCProviderDecision::Accepted
        });
        let SCProductionClaim::Ready(outcome) = new.production.claim() else {
            panic!();
        };
        publication
            .publish(&ticket, &dependencies, outcome, 1)
            .unwrap();
        let SCLookup::Ready(value) = publication.lookup(&identity, &dependencies).unwrap() else {
            panic!();
        };
        assert_eq!(*value.get(), 20);
        assert_eq!(budget.snapshot().active, 0);
        drop(publication);
        assert_eq!(accounting.snapshot().total_declared_bytes, 0);
    }
}

#[test]
fn superseded_production_settles_final_access_before_owned_loser_cleanup() {
    let dependencies = SCDependencies::new();
    let accounting = SCAccountingDomain::new();
    let budget = SCAdmission::new(SCCountLimit::new(1));
    let mut publication = SCPublication::<_, u8>::new();
    let identity = publication.ensure("resource");
    let old = publication
        .begin_attempt(&identity, dependencies.snapshot(&[]).unwrap())
        .unwrap();
    let input =
        SCBacking::try_new(vec![1u8; 8], &accounting, 8, SCAllocationClass::Temporary).unwrap();
    let (production, submission) = SCProduction::<SCBacking<'static, u8>, ()>::prepare_with_permit(
        input,
        budget.try_acquire().unwrap(),
    );
    let mut consumed = None;
    submission.submit(|mut root| {
        consumed = root.consume_inputs().unwrap();
        root.complete(Ok(backing(10, &accounting))).unwrap();
        SCProviderDecision::Accepted
    });
    assert!(matches!(production.claim(), SCProductionClaim::Pending));
    let replacement = publication
        .begin_attempt(&identity, dependencies.snapshot(&[]).unwrap())
        .unwrap();
    publication
        .publish(&replacement, &dependencies, Ok(backing(20, &accounting)), 1)
        .unwrap();
    assert_eq!(accounting.snapshot().temporary_bytes, 8);
    assert_eq!(budget.snapshot().active, 1);
    drop(consumed);
    assert_eq!(accounting.snapshot().temporary_bytes, 0);
    assert_eq!(budget.snapshot().active, 0);
    let SCProductionClaim::Ready(candidate) = production.claim() else {
        panic!("final access must settle")
    };
    let rejected = publication
        .publish(&old, &dependencies, candidate, 1)
        .unwrap_err();
    assert_eq!(
        rejected.reason(),
        SCPublicationRejectionReason::ObsoleteAttempt
    );
    let (candidate, cost) = rejected.into_parts();
    let candidate = candidate.unwrap();
    assert_eq!(*candidate.view().get(), 10);
    assert_eq!(cost, 1);
    assert_eq!(accounting.snapshot().resident_bytes, 2);
    drop(candidate);
    assert_eq!(accounting.snapshot().resident_bytes, 1);
    let SCLookup::Ready(value) = publication.lookup(&identity, &dependencies).unwrap() else {
        panic!()
    };
    assert_eq!(*value.get(), 20);
}

#[test]
fn device_change_blocks_gpu_reuse_but_preserves_cpu_source_and_old_reader() {
    let mut dependencies = SCDependencies::new();
    let source = dependencies.register(SCDependencyScope::Source);
    let device = dependencies.register(SCDependencyScope::Device);
    let accounting = SCAccountingDomain::new();
    let mut publication = SCPublication::<_, u8>::new();
    let cpu = publication.ensure("cpu");
    let gpu = publication.ensure("gpu");
    let cpu_attempt = publication
        .begin_attempt(
            &cpu,
            dependencies
                .snapshot(std::slice::from_ref(&source))
                .unwrap(),
        )
        .unwrap();
    let gpu_attempt = publication
        .begin_attempt(
            &gpu,
            dependencies
                .snapshot(&[source.clone(), device.clone()])
                .unwrap(),
        )
        .unwrap();
    publication
        .publish(&cpu_attempt, &dependencies, Ok(backing(1, &accounting)), 1)
        .unwrap();
    publication
        .publish(&gpu_attempt, &dependencies, Ok(backing(2, &accounting)), 1)
        .unwrap();
    let SCLookup::Ready(old_reader) = publication.share(&gpu, &dependencies).unwrap() else {
        panic!()
    };
    dependencies.advance(std::slice::from_ref(&device)).unwrap();
    assert!(matches!(
        publication.lookup(&gpu, &dependencies),
        Err(SCDependencyError::Stale)
    ));
    assert!(matches!(
        publication.share(&gpu, &dependencies),
        Err(SCDependencyError::Stale)
    ));
    let SCLookup::Ready(value) = publication.lookup(&cpu, &dependencies).unwrap() else {
        panic!()
    };
    assert_eq!(*value.get(), 1);
    assert_eq!(*old_reader.view().get(), 2);
    let replacement = publication
        .begin_attempt(&gpu, dependencies.snapshot(&[source, device]).unwrap())
        .unwrap();
    drop(
        publication
            .publish(&replacement, &dependencies, Ok(backing(3, &accounting)), 1)
            .unwrap(),
    );
    assert_eq!(accounting.snapshot().resident_bytes, 3);
    assert_eq!(*old_reader.view().get(), 2);
    drop(old_reader);
    assert_eq!(accounting.snapshot().resident_bytes, 2);
    let SCLookup::Ready(value) = publication.lookup(&gpu, &dependencies).unwrap() else {
        panic!()
    };
    assert_eq!(*value.get(), 3);
}

#[test]
fn concurrent_candidates_select_one_winner_and_return_the_owned_loser() {
    let dependencies = SCDependencies::new();
    let accounting = SCAccountingDomain::new();
    let mut publication = SCPublication::<_, u8>::new();
    let identity = publication.ensure(1);
    let attempt = publication
        .begin_attempt(&identity, dependencies.snapshot(&[]).unwrap())
        .unwrap();
    let publication = Mutex::new(publication);
    let barrier = Arc::new(Barrier::new(2));
    let outcomes = std::thread::scope(|scope| {
        let mut workers = Vec::new();
        for value in [7, 9] {
            let candidate = backing(value, &accounting);
            let attempt = attempt.clone();
            let barrier = barrier.clone();
            let publication = &publication;
            let dependencies = &dependencies;
            workers.push(scope.spawn(move || {
                barrier.wait();
                publication
                    .lock()
                    .unwrap()
                    .publish(&attempt, dependencies, Ok(candidate), 1)
            }));
        }
        workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert_eq!(outcomes.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(accounting.snapshot().resident_bytes, 2);
    let loser = outcomes
        .into_iter()
        .find_map(Result::err)
        .expect("one rejected candidate");
    assert_eq!(
        loser.reason(),
        SCPublicationRejectionReason::ObsoleteAttempt
    );
    let loser = loser.into_parts().0.unwrap();
    let winner = {
        let publication = publication.lock().unwrap();
        assert_eq!(publication.policy_bytes(), 1);
        let SCLookup::Ready(winner) = publication.share(&identity, &dependencies).unwrap() else {
            panic!()
        };
        winner
    };
    assert_ne!(winner.view().get(), loser.view().get());
    drop(loser);
    assert_eq!(accounting.snapshot().resident_bytes, 1);
}

#[test]
fn collection_discards_stale_payload_without_revoking_a_fresh_pending_round() {
    let accounting = SCAccountingDomain::new();
    let mut dependencies = SCDependencies::new();
    let source = dependencies.register(SCDependencyScope::Source);
    let mut publication = SCPublication::<_, u8>::new();
    let identity = publication.ensure(1);
    let old = publication
        .begin_attempt(
            &identity,
            dependencies
                .snapshot(std::slice::from_ref(&source))
                .unwrap(),
        )
        .unwrap();
    publication
        .publish(&old, &dependencies, Ok(backing(1, &accounting)), 1)
        .unwrap();
    let SCLookup::Ready(reader) = publication.share(&identity, &dependencies).unwrap() else {
        panic!()
    };
    let current = dependencies.advance(std::slice::from_ref(&source)).unwrap();
    let attempt = publication.begin_attempt(&identity, current).unwrap();
    let pinned = publication.maintain(solcache::SCByteLimit::AtOrAbove(1), 1, true);
    assert_eq!(pinned.evicted_payloads, 0);
    assert_eq!(pinned.pinned_encountered, 1);
    drop(reader);
    let collected = publication.maintain(solcache::SCByteLimit::AtOrAbove(1), 1, true);
    assert_eq!(collected.evicted_payloads, 1);
    assert_eq!(publication.policy_bytes(), 0);
    assert_eq!(accounting.snapshot().resident_bytes, 0);
    // Obsolete payload validity must not turn an evicted entry into a stale hit.
    assert!(matches!(
        publication.lookup(&identity, &dependencies).unwrap(),
        SCLookup::Vacant
    ));
    publication
        .publish(&attempt, &dependencies, Ok(backing(2, &accounting)), 1)
        .unwrap();
    let SCLookup::Ready(value) = publication.lookup(&identity, &dependencies).unwrap() else {
        panic!()
    };
    assert_eq!(*value.get(), 2);
}
