use std::collections::VecDeque;
use std::thread;

use solcache::{
    SCAccountingDomain, SCAdmission, SCAllocationClass, SCBacking, SCCountLimit, SCDependencies,
    SCLookup, SCProductionAttempt, SCProductionClaim, SCProductionMap, SCProductionPhase,
    SCProductionStart, SCProviderDecision, SCPublication, SCPublicationAttempt,
    SCSubmissionOutcome,
};

type Output = SCBacking<'static, Vec<u8>>;
type Work = (
    SCProductionAttempt<Box<[u8]>, Output, ()>,
    SCPublicationAttempt<&'static str>,
    usize,
);

// This is a concrete, caller-owned one-slot queue, not an SC executor contract.
fn enqueue(queue: &mut VecDeque<Work>, work: Work) -> Result<(), Work> {
    if queue.is_empty() {
        queue.push_back(work);
        Ok(())
    } else {
        Err(work)
    }
}

#[test]
fn manual_queue_retries_the_same_unsubmitted_attempt_and_publishes_on_its_owner() {
    let owner_thread = thread::current().id();
    let accounting = SCAccountingDomain::new();
    let allowance = SCAdmission::new(SCCountLimit::new(2));
    let dependencies = SCDependencies::new();
    let mut publication = SCPublication::<_, Vec<u8>>::new();
    let first = publication.ensure("first");
    let second = publication.ensure("second");
    let mut map = SCProductionMap::new(allowance.clone());
    let _first_interest = map.interest(first.clone(), 0, 1);
    let _second_interest = map.interest(second.clone(), 0, 1);
    let mut queue = VecDeque::new();
    let mut retry = None;
    for (identity, bytes) in [(&first, &b"abc"[..]), (&second, &b"xyz"[..])] {
        assert!(matches!(
            publication.lookup(identity, &dependencies).unwrap(),
            SCLookup::Vacant
        ));
        let input = Box::<[u8]>::from(bytes);
        let address = input.as_ptr() as usize;
        let SCProductionStart::Started(attempt) = map.begin_shared(identity, input).unwrap() else {
            panic!("new work");
        };
        let ticket = publication
            .begin_attempt(identity, dependencies.snapshot(&[]).unwrap())
            .unwrap();
        if let Err(work) = enqueue(&mut queue, (attempt, ticket, address)) {
            retry = Some(work);
        }
    }
    let rejected = retry.as_ref().expect("second enqueue is saturated");
    assert_eq!(
        rejected.0.production.snapshot().phase,
        SCProductionPhase::Prepared
    );
    assert!(matches!(
        rejected.0.production.claim(),
        SCProductionClaim::Pending
    ));
    assert_eq!(allowance.snapshot().active, 2);

    let mut invocations = 0;
    for _ in 0..2 {
        let (attempt, ticket, original_address) = queue.pop_front().unwrap();
        let decision = attempt.submission.submit(|mut root| {
            invocations += 1;
            assert_eq!(thread::current().id(), owner_thread);
            let input = root.inputs().unwrap();
            assert_eq!(
                input.as_ptr() as usize,
                original_address,
                "retry must retain the original owned input"
            );
            let bytes: Vec<u8> = input.iter().map(u8::to_ascii_uppercase).collect();
            let capacity = bytes.capacity() as u64;
            root.complete(Ok(SCBacking::try_new(
                bytes,
                &accounting,
                capacity,
                SCAllocationClass::Resident,
            )
            .unwrap()))
                .unwrap();
            SCProviderDecision::Accepted
        });
        assert!(matches!(decision, SCSubmissionOutcome::Accepted));
        let SCProductionClaim::Ready(outcome) = attempt.production.claim() else {
            panic!("inline input access settled");
        };
        let cost = outcome.as_ref().unwrap().declared_bytes();
        publication
            .publish(&ticket, &dependencies, outcome, cost)
            .unwrap();
        if let Some(work) = retry.take() {
            assert!(
                enqueue(&mut queue, work).is_ok(),
                "retry occurs only after progress"
            );
        }
    }
    assert_eq!(invocations, 2);
    assert_eq!(allowance.snapshot().active, 0);
    for (identity, expected) in [(&first, &b"ABC"[..]), (&second, &b"XYZ"[..])] {
        let SCLookup::Ready(reader) = publication.share(identity, &dependencies).unwrap() else {
            panic!("warm lookup must use existing result");
        };
        assert_eq!(reader.view().get(), expected);
    }
    assert!(queue.is_empty());
    assert_eq!(invocations, 2, "warm requests scheduled nothing");
    drop(publication);
    assert_eq!(accounting.snapshot().total_declared_bytes, 0);
}
