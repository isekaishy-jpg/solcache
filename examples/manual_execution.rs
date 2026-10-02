//! A caller queues owned submissions and drives them itself, with no executor.

use solcache::{
    SCAccountingDomain, SCAdmission, SCAllocationClass, SCBacking, SCCountLimit, SCDependencies,
    SCLookup, SCProductionClaim, SCProductionMap, SCProductionStart, SCProviderDecision,
    SCPublication, SCSubmissionOutcome,
};

fn main() {
    let accounting = SCAccountingDomain::new();
    let dependencies = SCDependencies::new();
    let allowance = SCAdmission::new(SCCountLimit::new(1));
    let mut publication = SCPublication::<_, Vec<u8>>::new();
    let identity = publication.ensure("document");
    let mut work = SCProductionMap::new(allowance.clone());
    let _consumer = work.interest(identity.clone(), 0, 10);
    let mut queued = None;
    let mut provider_calls = 0;

    for _ in 0..2 {
        // This check precedes both production admission and input preparation.
        if let SCLookup::Ready(value) = publication.lookup(&identity, &dependencies).unwrap() {
            assert_eq!(value.get(), b"HELLO");
            continue;
        }
        match work.begin_shared(&identity, b"hello".to_vec()).unwrap() {
            SCProductionStart::Joined { unused_input, .. } => drop(unused_input),
            SCProductionStart::Started(attempt) => {
                let ticket = publication
                    .begin_attempt(&identity, dependencies.snapshot(&[]).unwrap())
                    .unwrap();
                // The caller retains this exact submission and ticket until its
                // chosen service point. No SC callback or task runs on observation.
                assert!(queued.is_none());
                queued = Some((attempt, ticket));
            }
        }
        if let Some((attempt, ticket)) = queued.take() {
            assert!(matches!(
                attempt.production.claim(),
                SCProductionClaim::Pending
            ));
            let decision = attempt.submission.submit(|mut root| {
                provider_calls += 1;
                let bytes: Vec<u8> = root
                    .inputs()
                    .unwrap()
                    .iter()
                    .map(u8::to_ascii_uppercase)
                    .collect();
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
                panic!("final access settled");
            };
            let cost = outcome.as_ref().unwrap().declared_bytes();
            publication
                .publish(&ticket, &dependencies, outcome, cost)
                .unwrap();
        }
    }
    assert_eq!(provider_calls, 1);
    assert_eq!(allowance.snapshot().active, 0);
    drop(publication);
    assert_eq!(accounting.snapshot().total_declared_bytes, 0);
    println!("One caller-driven production; the second request reused its publication.");
}
