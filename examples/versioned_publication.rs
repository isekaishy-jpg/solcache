use solcache::{
    SCAccountingDomain, SCAllocationClass, SCBacking, SCDependencies, SCDependencyError,
    SCDependencyScope, SCLookup, SCProduction, SCProductionClaim, SCProviderDecision,
    SCPublication,
};

fn main() {
    let accounting = SCAccountingDomain::new();
    let mut dependencies = SCDependencies::new();
    let source = dependencies.register(SCDependencyScope::Source);
    let mut publication = SCPublication::<_, Vec<u8>, &'static str>::new();
    let identity = publication.ensure("document");
    let attempt = publication
        .begin_attempt(
            &identity,
            dependencies
                .snapshot(std::slice::from_ref(&source))
                .unwrap(),
        )
        .unwrap();
    let (production, submission) = SCProduction::prepare(vec![1, 2, 3]);
    submission.submit(|mut producer| {
        let bytes = producer
            .inputs()
            .unwrap()
            .iter()
            .map(|byte| byte * 2)
            .collect::<Vec<_>>();
        let backing =
            SCBacking::try_new(bytes, &accounting, 3, SCAllocationClass::Resident).unwrap();
        producer.complete(Ok(backing)).unwrap();
        SCProviderDecision::Accepted
    });
    let SCProductionClaim::Ready(outcome) = production.claim() else {
        panic!("inline provider has settled all access");
    };
    publication
        .publish(&attempt, &dependencies, outcome, 3)
        .unwrap();
    let SCLookup::Ready(reader) = publication.share(&identity, &dependencies).unwrap() else {
        panic!("published resource");
    };
    dependencies.advance(std::slice::from_ref(&source)).unwrap();
    assert!(matches!(
        publication.lookup(&identity, &dependencies),
        Err(SCDependencyError::Stale)
    ));
    assert_eq!(reader.view().get(), &[2, 4, 6]);
    // Old readers retain their allocation; new readers need a current publication.
    drop(publication.clear(&identity));
    assert_eq!(accounting.snapshot().resident_bytes, 3);
    drop(reader);
    assert_eq!(accounting.snapshot().total_declared_bytes, 0);
    println!("Source invalidation blocks new reuse while old readers retain backing.");
}
