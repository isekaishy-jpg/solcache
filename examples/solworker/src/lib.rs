//! Application code composing SC and SW directly. This is not a cache adapter.

use std::num::NonZeroUsize;
use std::sync::mpsc::{self, Sender};
use std::thread::{self, ThreadId};
use std::time::{Duration, Instant};

use solcache::{
    SCAccountingDomain, SCAdmission, SCAllocationClass, SCBacking, SCCleanupContext, SCCountLimit,
    SCDemand, SCDependencies, SCIdentity, SCLookup, SCProductionClaim, SCProductionMap,
    SCProductionStart, SCProviderDecision, SCPublication, SCPublicationAttempt,
    SCPublicationRejected, SCSubmissionOutcome, SCUrgencyUpdate,
};
use solworker::{
    SWCompletion, SWCost, SWDeliveryStatus, SWExecutionClass, SWGroup, SWLane, SWLimits,
    SWOwnedLimits, SWPhase, SWPriority, SWPumpBudget, SWRetained, SWRuntime, SWRuntimeConfig,
    SWSpawnError, SWSpawnOptions, SWStageOptions, SWTaskStatus, SWWorkerConfig,
};

const WAIT: Duration = Duration::from_secs(30);
const BACKGROUND: SWPriority = SWPriority::new(10);
const URGENT: SWPriority = SWPriority::new(0);

fn runtime(runnable: usize, records: usize) -> SWRuntime {
    let config = SWRuntimeConfig::new(3, [SWWorkerConfig::new(1); 3]).unwrap();
    SWRuntime::builder(config)
        .with_owned_limits(SWOwnedLimits::new(records, 16, [runnable; 3], [1; 3]).unwrap())
        .with_capacity_limits(
            SWLimits::new(SWCost::new(16, 16, 4, 64), SWCost::default(), 0, None).unwrap(),
        )
        .with_demand_limits(vec![URGENT, SWPriority::new(5), BACKGROUND], 8)
        .build()
        .unwrap()
}

fn wait(completion: &SWCompletion) {
    assert_eq!(
        completion.wait_timeout(WAIT).unwrap(),
        Some(SWTaskStatus::Succeeded)
    );
}

fn service_demand(runtime: &SWRuntime) {
    // The graph is finite. A service failure cannot become an unbounded retry.
    for _ in 0..32 {
        if !runtime.service_demand(8) {
            return;
        }
    }
    panic!("finite demand graph did not settle");
}

fn blocked_worker(lane: &SWLane) -> (SWGroup, Sender<()>) {
    let group = lane.group().unwrap();
    let (started_send, started_recv) = mpsc::channel();
    let (release_send, release_recv) = mpsc::channel();
    lane.try_spawn_in(&group, SWSpawnOptions::default(), move || {
        started_send.send(()).unwrap();
        let _ = release_recv.recv_timeout(WAIT);
    })
    .unwrap();
    started_recv.recv_timeout(WAIT).unwrap();
    (group, release_send)
}

struct Payload {
    bytes: Box<[u8; 4]>,
    dropped: Sender<ThreadId>,
}

impl Drop for Payload {
    fn drop(&mut self) {
        let _ = self.dropped.send(thread::current().id());
    }
}

struct OwnerState<'cleanup> {
    cleanup: &'cleanup SCCleanupContext<Payload>,
    accounting: SCAccountingDomain,
    dependencies: SCDependencies,
    publication: SCPublication<'cleanup, &'static str, Payload>,
    identity: SCIdentity<&'static str>,
    application_thread: Option<ThreadId>,
    output_address: Option<usize>,
    transfer_retry: Option<(SCPublicationAttempt<&'static str>, SWRetained<Payload>)>,
    publication_retry: Option<(
        SCPublicationAttempt<&'static str>,
        SCPublicationRejected<'cleanup, Payload, ()>,
    )>,
}

impl OwnerState<'_> {
    fn apply_owned(
        &mut self,
        ticket: SCPublicationAttempt<&'static str>,
        retained: SWRetained<Payload>,
    ) {
        // Acquire the destination charge before releasing SW's lease. This
        // explicit transfer has a brief accounting overlap and copies no bytes.
        let charge = match self.accounting.charge(4, SCAllocationClass::Resident) {
            Ok(charge) => charge,
            Err(_) => {
                // Preserve the exact result, SW lease and original authority.
                self.transfer_retry = Some((ticket, retained));
                return;
            }
        };
        let payload = retained.into_inner();
        self.output_address = Some(payload.bytes.as_ptr() as usize);
        let backing = self.cleanup.acquire_charged(payload, charge);
        if let Err(rejected) = self
            .publication
            .publish(&ticket, &self.dependencies, Ok(backing), 4)
        {
            self.publication_retry = Some((ticket, rejected));
            return;
        }
        self.application_thread = Some(thread::current().id());
    }
}

/// Receipts from actual bytes, owned accesses and thread-bound destruction.
#[derive(Debug)]
pub struct PipelineReport {
    pub bytes: [u8; 4],
    pub input_address_preserved: bool,
    pub output_address_preserved: bool,
    pub applied_on_owner: bool,
    pub destroyed_on_owner: bool,
    pub transfer_rejected: bool,
}

/// Runs one cold request, one genuine saturation/retry and one warm request.
/// Illustrative capacities and timeout bounds are not tuning recommendations.
pub fn run_pipeline() -> PipelineReport {
    pipeline(false)
}

/// Adds a synthetic declared-accounting overflow to the real byte pipeline,
/// preserving its owned SW output and original ticket for one owner-local retry.
/// The sentinel declaration is arithmetic test input, not allocated memory.
pub fn run_transfer_rejection() -> PipelineReport {
    pipeline(true)
}

fn pipeline(reject_transfer: bool) -> PipelineReport {
    let mut runtime = runtime(1, 16);
    let low = runtime.lane(SWExecutionClass::Low);
    let (pressure, release_pressure) = blocked_worker(&low);
    low.try_spawn_in(&pressure, SWSpawnOptions::default(), || ())
        .unwrap();
    pressure.seal();
    assert!(runtime.progress().scheduler.runnable_full[0]);

    let accounting = SCAccountingDomain::new();
    let allowance = SCAdmission::new(SCCountLimit::new(1));
    let dependencies = SCDependencies::new();
    let cleanup = SCCleanupContext::new();
    let mut publication = SCPublication::new();
    let identity = publication.ensure("prepared bytes");
    let mut productions = SCProductionMap::<_, SWRetained<Payload>, ()>::new(allowance.clone());
    let first_interest = productions.interest(identity.clone(), 0, 1);
    let second_interest = productions.interest(identity.clone(), 0, 10);
    let demand = productions.demand(&identity).unwrap();
    let input = SCBacking::try_new(
        Box::new([1_u8, 2, 3, 4]),
        &accounting,
        4,
        SCAllocationClass::Temporary,
    )
    .unwrap();
    let original_input_address = input.view().get().as_ptr() as usize;

    // This sequence is serialized on the coordinator. Only Started rotates a
    // publication round. A joined request must keep this original ticket.
    assert!(matches!(
        publication.lookup(&identity, &dependencies).unwrap(),
        SCLookup::Vacant
    ));
    let SCProductionStart::Started(attempt) = productions.begin_shared(&identity, input).unwrap()
    else {
        panic!("first cold request starts production");
    };
    let ticket = publication
        .begin_attempt(&identity, dependencies.snapshot(&[]).unwrap())
        .unwrap();
    let production = attempt.production;
    let producers = runtime.work_set(NonZeroUsize::new(1).unwrap()).unwrap();
    let discovery = producers.discovery().unwrap();
    let root_group = low.group().unwrap();

    // SC charges the input. SW owns the output's four declared bytes until
    // the owner explicitly transfers that output to SC. These are separate
    // domains; their snapshots are not a single physical-memory total.
    // The reservation predicts output bytes only. It does not reserve CPU job
    // admission, so ordinary runnable pressure still rejects the root stage.
    let pipeline = runtime.reserve_ordinary(SWCost::new(0, 0, 0, 4)).unwrap();
    let stage = pipeline.stage(SWCost::new(0, 0, 0, 4)).unwrap();
    let output_charge = stage.retain_bytes(4).unwrap();
    assert_eq!(stage.available(), SWCost::default());
    let (child_send, child_recv) = mpsc::channel();
    let (root_started_send, root_started_recv) = mpsc::channel();
    let (root_release_send, root_release_recv) = mpsc::channel();
    let (dropped_send, dropped_recv) = mpsc::channel();
    let (addresses_send, addresses_recv) = mpsc::channel();
    let mut pending = None;
    let submitted = attempt.submission.submit(|mut root| {
        let operation = move || {
            root_started_send.send(()).unwrap();
            if root_release_recv.recv_timeout(WAIT).is_err() {
                return;
            }
            let input = root.inputs().unwrap();
            let actual_input_address = input.view().get().as_ptr() as usize;
            let child = root.child(input.clone()).unwrap();
            let bytes = Box::new((**input.view().get()).map(|value| value * 2));
            let output_address = bytes.as_ptr() as usize;
            root.complete(Ok(SWRetained::from_lease(
                Payload {
                    bytes,
                    dropped: dropped_send,
                },
                output_charge,
            )))
            .unwrap();
            child_send.send(child).unwrap();
            addresses_send
                .send((actual_input_address, output_address))
                .unwrap();
        };
        let rejected = low
            .try_spawn_stage(
                SWStageOptions {
                    group: Some(&root_group),
                    work_set: Some(&producers),
                    cost: SWCost::new(1, 0, 0, 0),
                    priority: Some(BACKGROUND),
                    ..Default::default()
                },
                operation,
            )
            .err()
            .expect("genuine unreserved runnable saturation");
        assert_eq!(rejected.reason, SWSpawnError::Full);
        pending = Some(rejected);
        // Accepted here means the HOST owns responsibility for this exact
        // root and its retained retry. SW admission has NOT succeeded.
        SCProviderDecision::Accepted
    });
    assert!(matches!(submitted, SCSubmissionOutcome::Accepted));
    assert_eq!(production.snapshot().active_accesses, 1);
    assert_eq!(allowance.snapshot().active, 1);
    assert_eq!(accounting.snapshot().temporary_bytes, 4);
    let SCProductionStart::Joined { unused_input, .. } = productions
        .begin_shared(&identity, Box::new([99_u8; 4]))
        .unwrap()
    else {
        panic!("rejected SW admission still retains the original SC attempt");
    };
    assert_eq!(*unused_input, [99; 4]);

    release_pressure.send(()).unwrap();
    wait(&pressure.completion());
    // One deliberate retry after observed settlement, with untouched closure
    // and options. Drop that closure instead to take the abandonment route.
    let rejected = pending.take().unwrap();
    let (root_task, _root_control) = low
        .try_spawn_stage(rejected.options, rejected.operation)
        .unwrap();
    drop(pending);
    // Once the accepted stage settles, only the output's detached byte lease
    // keeps its four SW bytes charged. No parent reserve survives the transfer.
    drop((stage, pipeline));
    root_group.seal();
    producers.seal();
    root_started_recv.recv_timeout(WAIT).unwrap();

    // This coordinator owns all SC demand changes and forwarding. SC uses
    // larger urgency for higher priority; SW uses smaller ranks.
    let snapshot = demand.snapshot();
    assert!(demand.is_current(&snapshot));
    let forwarded = root_task
        .completion()
        .demand(rank(snapshot.urgency().unwrap()))
        .unwrap();
    first_interest.detach();
    assert_eq!(second_interest.update(1, 1), SCUrgencyUpdate::Applied);
    assert_eq!(second_interest.update(0, 10), SCUrgencyUpdate::Obsolete);
    assert!(!demand.is_current(&snapshot));
    let current = demand.snapshot();
    assert!(demand.is_current(&current));
    forwarded.refresh(rank(current.urgency().unwrap())).unwrap();
    service_demand(&runtime);
    root_release_send.send(()).unwrap();
    wait(&root_group.completion());
    drop(forwarded);
    service_demand(&runtime);
    let child = child_recv.recv_timeout(WAIT).unwrap();
    let (actual_input_address, output_address) = addresses_recv.recv_timeout(WAIT).unwrap();
    assert!(matches!(production.claim(), SCProductionClaim::Pending));

    // Actual accepted discovery after producer-set root closure. The SC child
    // keeps real input bytes alive even though the root CPU job has finished.
    let mid = runtime.lane(SWExecutionClass::Mid);
    let child_group = mid.group().unwrap();
    let (child_started_send, child_started_recv) = mpsc::channel();
    let (child_release_send, child_release_recv) = mpsc::channel();
    let owner_thread = thread::current().id();
    let phase = SWPhase(1);
    let mut owner = runtime
        .owner(
            OwnerState {
                cleanup: &cleanup,
                accounting: accounting.clone(),
                dependencies,
                publication,
                identity: identity.clone(),
                application_thread: None,
                output_address: None,
                transfer_retry: None,
                publication_retry: None,
            },
            NonZeroUsize::new(1).unwrap(),
        )
        .unwrap();
    owner.set_phase(phase).unwrap();
    let claim = production.clone();
    let (delivery, _delivery_control) = owner
        .on_ready(&child_group.completion(), phase, move |state, status| {
            assert_eq!(status, SWTaskStatus::Succeeded);
            let SCProductionClaim::Ready(Ok(retained)) = claim.claim() else {
                panic!("root output is claimable after child final use");
            };
            state.apply_owned(ticket, retained);
        })
        .unwrap();
    // Reserve the owner callback before accepting the child whose completion
    // triggers required application. Registration rejection returns this callback
    // intact; the root output stays unclaimed in its production until handled.
    mid.try_spawn_stage(
        SWStageOptions {
            group: Some(&child_group),
            discovery: Some(&discovery),
            ..Default::default()
        },
        move || {
            child_started_send.send(()).unwrap();
            if child_release_recv.recv_timeout(WAIT).is_err() {
                return;
            }
            assert_eq!(**child.inputs().unwrap().view().get(), [1, 2, 3, 4]);
            drop(child);
        },
    )
    .unwrap();
    child_group.seal();
    drop(discovery);
    child_started_recv.recv_timeout(WAIT).unwrap();
    assert!(!producers.is_drained());
    assert_eq!(allowance.snapshot().active, 1);
    assert_eq!(accounting.snapshot().temporary_bytes, 4);
    assert_eq!(runtime.capacity_usage().unwrap().ordinary.bytes, 4);
    assert_eq!(owner.pump(phase, SWPumpBudget::new(1)).unwrap().invoked, 0);
    child_release_send.send(()).unwrap();
    // Registering an owner forbids SW passive waits on this thread. Observe
    // this CPU-only group without blocking the owner context; delivery is
    // serviced separately below, after the optional transfer-failure fixture.
    let completion = child_group.completion();
    let deadline = Instant::now() + WAIT;
    while completion.status().is_none() {
        assert!(Instant::now() < deadline, "child group did not settle");
        thread::yield_now();
    }
    assert_eq!(completion.status(), Some(SWTaskStatus::Succeeded));
    assert_eq!(accounting.snapshot().temporary_bytes, 0);
    // This synthetic sentinel is installed only after input final use, so the
    // destination charge has no freed-input headroom that could mask overflow.
    let sentinel = reject_transfer.then(|| {
        accounting
            .charge(u64::MAX, SCAllocationClass::Temporary)
            .unwrap()
    });
    // Terminal group status can precede notification activation. Service the
    // owner route until this actual delivery settles; observing CPU status alone
    // does not certify that a subscriber has become eligible for pumping.
    let deadline = Instant::now() + WAIT;
    while !delivery.status().is_settled() {
        assert!(Instant::now() < deadline, "owner delivery did not settle");
        owner.pump(phase, SWPumpBudget::new(1)).unwrap();
        thread::yield_now();
    }
    assert_eq!(delivery.status(), SWDeliveryStatus::Published);
    if reject_transfer {
        let (ticket, retained) = owner
            .state_mut()
            .transfer_retry
            .take()
            .expect("owned rejected transfer");
        assert_eq!(*retained.bytes, [2, 4, 6, 8]);
        assert_eq!(retained.bytes.as_ptr() as usize, output_address);
        assert_eq!(runtime.capacity_usage().unwrap().ordinary.bytes, 4);
        assert!(matches!(
            owner
                .state()
                .publication
                .lookup(&identity, &owner.state().dependencies)
                .unwrap(),
            SCLookup::Vacant
        ));
        assert!(dropped_recv.try_recv().is_err());
        drop(sentinel);
        // One explicit retry in the required owner context. The original ticket
        // is reused; no new begin_attempt can retag the retained old bytes.
        owner.state_mut().apply_owned(ticket, retained);
    }
    assert!(owner.state().transfer_retry.is_none());
    assert!(owner.state().publication_retry.is_none());
    assert!(producers.is_drained());
    assert_eq!(allowance.snapshot().active, 0);
    assert_eq!(accounting.snapshot().temporary_bytes, 0);
    assert_eq!(runtime.capacity_usage().unwrap().ordinary.bytes, 0);

    // Warm valid lookup is the entire request path: no begin_shared, ticket,
    // worker submission or delivery is required for synchronous reuse.
    let SCLookup::Ready(reader) = owner
        .state()
        .publication
        .share(&owner.state().identity, &owner.state().dependencies)
        .unwrap()
    else {
        panic!("published warm result");
    };
    let bytes = *reader.view().get().bytes;
    assert_eq!(bytes, [2, 4, 6, 8]);
    assert!(matches!(production.claim(), SCProductionClaim::Taken));
    let applied_on_owner = owner.state().application_thread == Some(owner_thread);
    let output_address_preserved = owner.state().output_address == Some(output_address)
        && reader.view().get().bytes.as_ptr() as usize == output_address;
    assert_eq!(cleanup.pending(), 0);
    let state = owner.state_mut();
    drop(state.publication.clear(&state.identity));
    assert_eq!(accounting.snapshot().resident_bytes, 4);
    drop(reader);
    assert_eq!(cleanup.pending(), 1);
    assert_eq!(accounting.snapshot().retiring_bytes, 4);
    assert!(dropped_recv.try_recv().is_err());
    assert_eq!(cleanup.drain(), 1);
    let destroyed_on_owner = dropped_recv.recv_timeout(WAIT).unwrap() == owner_thread;
    assert_eq!(accounting.snapshot().total_declared_bytes, 0);
    owner.close();
    drop((owner, second_interest));
    service_demand(&runtime);
    runtime.shutdown().unwrap();
    PipelineReport {
        bytes,
        input_address_preserved: actual_input_address == original_input_address,
        output_address_preserved,
        applied_on_owner,
        destroyed_on_owner,
        transfer_rejected: reject_transfer,
    }
}

/// Drops a retained saturated operation instead of retrying it. The original
/// SC root ends, owned input is destroyed and its allowance is returned.
pub fn settle_saturated_attempt() {
    let mut runtime = runtime(1, 16);
    let low = runtime.lane(SWExecutionClass::Low);
    let (pressure, release) = blocked_worker(&low);
    low.try_spawn_in(&pressure, SWSpawnOptions::default(), || ())
        .unwrap();
    pressure.seal();
    let accounting = SCAccountingDomain::new();
    let allowance = SCAdmission::new(SCCountLimit::new(1));
    let mut publication = SCPublication::<_, u8>::new();
    let identity = publication.ensure("abandoned");
    let mut productions = SCProductionMap::<_, (), ()>::new(allowance.clone());
    let _interest = productions.interest(identity.clone(), 0, 1);
    let input = SCBacking::try_new(
        Box::new([7_u8; 4]),
        &accounting,
        4,
        SCAllocationClass::Temporary,
    )
    .unwrap();
    let SCProductionStart::Started(attempt) = productions.begin_shared(&identity, input).unwrap()
    else {
        panic!("new attempt");
    };
    let mut pending = None;
    attempt.submission.submit(|root| {
        // The closure keeps the root intact; it has never run.
        let rejected = low
            .try_spawn(SWSpawnOptions::default(), move || drop(root))
            .err()
            .expect("saturated runnable queue");
        assert_eq!(rejected.reason, SWSpawnError::Full);
        pending = Some(rejected);
        SCProviderDecision::Accepted
    });
    assert_eq!(allowance.snapshot().active, 1);
    assert_eq!(accounting.snapshot().temporary_bytes, 4);
    drop(pending);
    assert!(matches!(
        attempt.production.claim(),
        SCProductionClaim::Abandoned
    ));
    assert_eq!(allowance.snapshot().active, 0);
    assert_eq!(accounting.snapshot().total_declared_bytes, 0);
    assert_eq!(
        productions
            .demand(&identity)
            .unwrap()
            .snapshot()
            .consumers(),
        1
    );
    release.send(()).unwrap();
    wait(&pressure.completion());
    runtime.shutdown().unwrap();
}

fn rank(urgency: u32) -> SWPriority {
    // Host policy: SC urgency >= 10 maps to SW rank 0; lower urgency to 10.
    if urgency >= 10 { URGENT } else { BACKGROUND }
}

/// Proves forwarding changes real pending execution order. All SC updates and
/// snapshot-to-SW application occur on this serialized coordinator.
pub fn forward_ordered_demand() {
    // Raise, lower, and detach a more urgent independent consumer. In every
    // case both resources remain pending until forwarding has been serviced.
    for (initial, current_urgency, detach) in [(1, 10, false), (10, 1, false), (10, 1, true)] {
        let mut runtime = runtime(2, 16);
        let low = runtime.lane(SWExecutionClass::Low);
        let (pressure, release) = blocked_worker(&low);
        pressure.seal();
        let demand = SCDemand::new();
        let _background = demand.attach(0, 1);
        let consumer = demand.attach(0, initial);
        let (order_send, order_recv) = mpsc::channel();
        let shared_send = order_send.clone();
        let group = low.group().unwrap();
        let (shared, _) = low
            .try_spawn_stage(
                SWStageOptions {
                    group: Some(&group),
                    priority: Some(BACKGROUND),
                    ..Default::default()
                },
                move || shared_send.send("shared").unwrap(),
            )
            .unwrap();
        low.try_spawn_stage(
            SWStageOptions {
                group: Some(&group),
                priority: Some(SWPriority::new(5)),
                ..Default::default()
            },
            move || order_send.send("other").unwrap(),
        )
        .unwrap();
        group.seal();
        let old = demand.snapshot();
        assert!(demand.is_current(&old));
        let forwarded = shared
            .completion()
            .demand(rank(old.urgency().unwrap()))
            .unwrap();
        if detach {
            consumer.detach();
        } else {
            assert_eq!(
                consumer.update(1, current_urgency),
                SCUrgencyUpdate::Applied
            );
            assert_eq!(consumer.update(0, 10), SCUrgencyUpdate::Obsolete);
        }
        // Applying the old aggregate here would leave an obsolete high rank.
        assert!(!demand.is_current(&old));
        let snapshot = demand.snapshot();
        assert!(demand.is_current(&snapshot));
        assert_eq!(snapshot.urgency(), Some(current_urgency));
        forwarded
            .refresh(rank(snapshot.urgency().unwrap()))
            .unwrap();
        service_demand(&runtime);
        release.send(()).unwrap();
        let observed = [
            order_recv.recv_timeout(WAIT).unwrap(),
            order_recv.recv_timeout(WAIT).unwrap(),
        ];
        assert_eq!(
            observed,
            if current_urgency >= 10 {
                ["shared", "other"]
            } else {
                ["other", "shared"]
            }
        );
        wait(&pressure.completion());
        wait(&group.completion());
        drop(forwarded);
        service_demand(&runtime);
        runtime.shutdown().unwrap();
    }
}
