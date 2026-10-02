use std::num::NonZeroUsize;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use solcache::{
    SCAccountingDomain, SCAdmission, SCAllocationCharge, SCAllocationClass, SCBacking,
    SCCleanupContext, SCCountLimit, SCProduction, SCProductionClaim, SCProviderDecision,
};
use solworker::{
    SWDeliveryStatus, SWExecutionClass, SWOwnedLimits, SWPhase, SWPumpBudget, SWRuntime,
    SWRuntimeConfig, SWSpawnOptions, SWTaskStatus, SWWorkerConfig,
};

#[test]
fn shutdown_keeps_owner_delivery_alive_and_join_does_not_retire_escaped_output() {
    let mut runtime =
        SWRuntime::builder(SWRuntimeConfig::new(3, [SWWorkerConfig::new(1); 3]).unwrap())
            .with_owned_limits(SWOwnedLimits::new(16, 16, [4; 3], [1; 3]).unwrap())
            .build()
            .unwrap();
    let roots = SCAdmission::new(SCCountLimit::new(1));
    let accounting = SCAccountingDomain::new();
    let cleanup = SCCleanupContext::<Vec<u8>>::new();
    let input = SCBacking::try_new(
        Box::new([1_u8, 2, 3, 4]),
        &accounting,
        4,
        SCAllocationClass::Temporary,
    )
    .unwrap();
    let (production, submission) =
        SCProduction::<(Vec<u8>, SCAllocationCharge), ()>::prepare_with_permit(
            input,
            roots.try_acquire().unwrap(),
        );
    let lane = runtime.lane(SWExecutionClass::Low);
    let group = lane.group().unwrap();
    let (release, gate) = mpsc::channel();
    let (prepared, output_capacity) = mpsc::channel();
    let output_domain = accounting.clone();
    submission.submit(|mut root| {
        lane.try_spawn_in(&group, SWSpawnOptions::default(), move || {
            if gate.recv_timeout(Duration::from_secs(30)).is_err() {
                return;
            }
            // This accepted capability remains valid after both root gates close.
            let child = root.child(root.inputs().unwrap().clone()).unwrap();
            let output: Vec<u8> = child
                .inputs()
                .unwrap()
                .view()
                .get()
                .iter()
                .map(|byte| byte * 2)
                .collect();
            let capacity = output.capacity() as u64;
            let charge = output_domain
                .charge(capacity, SCAllocationClass::Temporary)
                .unwrap();
            root.complete(Ok((output, charge))).unwrap();
            prepared.send(capacity).unwrap();
            drop(child);
        })
        .unwrap();
        SCProviderDecision::Accepted
    });
    group.seal();
    let phase = SWPhase(1);
    let owner_thread = thread::current().id();
    let (applied, application) = mpsc::channel();
    let claim = production.clone();
    let context = &cleanup;
    // The callback itself must be static; borrow the cleanup context via state.
    let mut owner = runtime
        .owner(
            (None::<SCBacking<'_, Vec<u8>>>, context),
            NonZeroUsize::new(1).unwrap(),
        )
        .unwrap();
    owner.set_phase(phase).unwrap();
    let (delivery, _control) = owner
        .on_ready(&group.completion(), phase, move |state, status| {
            assert_eq!(status, SWTaskStatus::Succeeded);
            let SCProductionClaim::Ready(Ok((bytes, mut charge))) = claim.claim() else {
                panic!("final access ended");
            };
            // Move the existing charge through application without reacquisition.
            charge.transition(SCAllocationClass::Resident);
            state.0 = Some(state.1.acquire_charged(bytes, charge));
            applied.send(thread::current().id()).unwrap();
        })
        .unwrap();

    roots.close();
    runtime.begin_shutdown();
    assert!(roots.try_acquire().is_err());
    assert!(lane.try_spawn(SWSpawnOptions::default(), || ()).is_err());
    assert!(!runtime.try_shutdown().unwrap());
    assert!(matches!(production.claim(), SCProductionClaim::Pending));
    assert_eq!(roots.snapshot().active, 1);
    release.send(()).unwrap();
    // CPU settlement does not consume the result or service owner delivery.
    // A live owner forbids passive SW waits, so observe this independent group.
    let deadline = Instant::now() + Duration::from_secs(30);
    while group.completion().status().is_none() {
        assert!(Instant::now() < deadline, "production did not settle");
        thread::yield_now();
    }
    assert_eq!(group.completion().status(), Some(SWTaskStatus::Succeeded));
    let capacity = output_capacity
        .recv_timeout(Duration::from_secs(30))
        .unwrap();
    assert!(capacity >= 4);
    assert_eq!(accounting.snapshot().temporary_bytes, capacity);
    assert_eq!(roots.snapshot().active, 0);
    assert!(owner.state().0.is_none());
    assert_eq!(cleanup.drain_budget(1), 0);
    let deadline = Instant::now() + Duration::from_secs(30);
    while !delivery.status().is_settled() {
        assert!(
            Instant::now() < deadline,
            "shutdown delivery did not settle"
        );
        owner.pump(phase, SWPumpBudget::new(1)).unwrap();
        thread::yield_now();
    }
    assert_eq!(delivery.status(), SWDeliveryStatus::Published);
    assert_eq!(application.recv().unwrap(), owner_thread);
    let escaped = owner.state_mut().0.take().unwrap();
    assert_eq!(escaped.view().get(), &[2, 4, 6, 8]);
    assert_eq!(escaped.declared_bytes(), capacity);
    assert_eq!(accounting.snapshot().temporary_bytes, 0);
    assert_eq!(roots.snapshot().active, 0);
    owner.close();
    drop(owner);
    let deadline = Instant::now() + Duration::from_secs(30);
    while !runtime.try_shutdown().unwrap() {
        assert!(Instant::now() < deadline, "runtime did not settle");
        thread::yield_now();
    }
    assert_eq!(
        accounting.snapshot().resident_bytes,
        escaped.declared_bytes()
    );
    assert_eq!(cleanup.drain_budget(1), 0);
    drop(escaped);
    assert_eq!(cleanup.snapshot().pending_records, 1);
    assert_eq!(cleanup.drain_budget(1), 1);
    assert_eq!(accounting.snapshot().total_declared_bytes, 0);
}
