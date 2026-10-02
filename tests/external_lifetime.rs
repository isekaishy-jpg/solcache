use std::sync::mpsc;
use std::thread::{self, ThreadId};

use solcache::{
    SCAccountingDomain, SCAccountingError, SCAdmission, SCAllocationClass, SCBacking,
    SCCleanupContext, SCCountLimit, SCProduction, SCProductionAccess, SCProductionClaim,
    SCProviderDecision, SCSubmissionOutcome,
};

struct DropObservation {
    thread: ThreadId,
    total_bytes: u64,
    retiring_bytes: u64,
}

struct ObservedBytes {
    bytes: Box<[u8]>,
    domain: SCAccountingDomain,
    dropped: mpsc::Sender<DropObservation>,
}

impl Drop for ObservedBytes {
    fn drop(&mut self) {
        let snapshot = self.domain.snapshot();
        let _ = self.dropped.send(DropObservation {
            thread: thread::current().id(),
            total_bytes: snapshot.total_declared_bytes,
            retiring_bytes: snapshot.retiring_bytes,
        });
    }
}

#[test]
fn completed_job_leaves_read_only_external_backing_and_charge_owned_until_retirement() {
    let temporary = SCAccountingDomain::new();
    let external = SCAccountingDomain::new();
    let cleanup = SCCleanupContext::new();
    let production_allowance = SCAdmission::new(SCCountLimit::new(1));
    let device_slots = SCAdmission::new(SCCountLimit::new(1));
    let (dropped, drop_observations) = mpsc::channel();
    let bytes = vec![2u8, 4, 6, 8].into_boxed_slice();
    let address = bytes.as_ptr();
    let byte_count = bytes.len() as u64;
    let charge = temporary
        .charge(byte_count, SCAllocationClass::Temporary)
        .unwrap();

    // This sentinel is a synthetic arithmetic boundary, not a memory allocation.
    // The real buffer remains owned beside the returned, unchanged charge.
    let sentinel = external
        .charge(u64::MAX, SCAllocationClass::Resident)
        .unwrap();
    let failure = charge
        .transfer(&external, SCAllocationClass::Resident)
        .unwrap_err();
    let (reason, charge) = failure.into_parts();
    assert_eq!(reason, SCAccountingError::Overflow);
    assert_eq!(charge.class(), SCAllocationClass::Temporary);
    assert_eq!(charge.declared_bytes(), byte_count);
    assert_eq!(bytes.as_ptr(), address);
    assert_eq!(temporary.snapshot().temporary_bytes, byte_count);
    assert_eq!(external.snapshot().resident_bytes, u64::MAX);
    drop(sentinel);
    let charge = charge
        .transfer(&external, SCAllocationClass::Resident)
        .unwrap();
    let owner = cleanup.acquire_charged(
        ObservedBytes {
            bytes,
            domain: external.clone(),
            dropped,
        },
        charge,
    );
    assert_eq!(temporary.snapshot().total_declared_bytes, 0);
    assert_eq!(owner.view().get().bytes.as_ptr(), address);
    assert_eq!(external.snapshot().resident_bytes, byte_count);

    // A simulated read-only device retains backing independently of SC access.
    // The result is immutable and already ready; publishing it need not wait for
    // this reader, but destroying its actual storage must wait.
    let device_owner = owner.clone();
    let device_slot = device_slots.try_acquire().unwrap();
    let (production, submission) = SCProduction::<_, ()>::prepare_with_permit(
        owner,
        production_allowance.try_acquire().unwrap(),
    );
    thread::scope(|scope| {
        // Unwinding disconnects this gate before scoped threads are joined.
        let (retire, retirement) = mpsc::channel();
        let device = scope.spawn(move || {
            if retirement.recv().is_ok() {
                assert_eq!(device_owner.view().get().bytes.as_ref(), &[2, 4, 6, 8]);
            }
            drop(device_owner);
            drop(device_slot);
        });
        let job = scope.spawn(move || {
            submission.submit(|mut producer| {
                let output = producer.inputs().unwrap().clone();
                producer.complete(Ok(output)).unwrap();
                drop(producer);
                SCProviderDecision::Accepted
            })
        });
        assert!(matches!(job.join().unwrap(), SCSubmissionOutcome::Accepted));
        assert_eq!(production.snapshot().active_accesses, 0);
        assert_eq!(production_allowance.snapshot().active, 0);
        let SCProductionClaim::Ready(Ok(output)) = production.claim() else {
            panic!("job outcome is ready while the read-only device still owns bytes");
        };
        assert_eq!(output.view().get().bytes.as_ptr(), address);
        drop(output);
        assert_eq!(device_slots.snapshot().active, 1);
        assert_eq!(external.snapshot().resident_bytes, byte_count);
        assert_eq!(cleanup.drain(), 0);
        assert!(drop_observations.try_recv().is_err());

        retire.send(()).unwrap();
        device.join().unwrap();
    });
    assert_eq!(device_slots.snapshot().active, 0);
    assert_eq!(cleanup.pending(), 1);
    assert_eq!(external.snapshot().retiring_bytes, byte_count);
    assert!(drop_observations.try_recv().is_err());
    assert_eq!(cleanup.drain(), 1);
    let observation = drop_observations.recv().unwrap();
    assert_eq!(observation.thread, thread::current().id());
    assert_eq!(observation.total_bytes, byte_count);
    assert_eq!(observation.retiring_bytes, byte_count);
    assert_eq!(external.snapshot().total_declared_bytes, 0);
}

#[test]
fn accepted_discovery_after_callback_return_gates_application_through_external_final_access() {
    let accounting = SCAccountingDomain::new();
    let cleanup = SCCleanupContext::new();
    let production_allowance = SCAdmission::new(SCCountLimit::new(1));
    let provider_slots = SCAdmission::new(SCCountLimit::new(1));
    let (dropped, drop_observations) = mpsc::channel();
    let owner = cleanup
        .try_acquire(
            ObservedBytes {
                bytes: vec![3u8, 5, 7].into_boxed_slice(),
                domain: accounting.clone(),
                dropped,
            },
            &accounting,
            3,
            SCAllocationClass::Temporary,
        )
        .unwrap();
    let (production, submission) = SCProduction::<u8, ()>::prepare_with_permit(
        owner,
        production_allowance.try_acquire().unwrap(),
    );
    let mut continuation = None;
    let decision = submission.submit(|mut producer| {
        let sum = producer.inputs().unwrap().view().get().bytes.iter().sum();
        continuation = producer.consume_inputs().unwrap();
        producer.complete(Ok(sum)).unwrap();
        drop(producer);
        SCProviderDecision::Accepted
    });
    assert!(matches!(decision, SCSubmissionOutcome::Accepted));
    assert!(production.snapshot().completion_recorded);
    assert!(matches!(production.claim(), SCProductionClaim::Pending));

    let continuation = continuation.unwrap();
    let provider_slot = provider_slots.try_acquire().unwrap();
    let (handoff, device_input) =
        mpsc::channel::<SCProductionAccess<SCBacking<'_, ObservedBytes>, u8, ()>>();
    thread::scope(|scope| {
        let (finish, finished) = mpsc::channel();
        let device = scope.spawn(move || {
            let Ok(child) = device_input.recv() else {
                return;
            };
            if finished.recv().is_ok() {
                assert_eq!(
                    child.inputs().unwrap().view().get().bytes.as_ref(),
                    &[3, 5, 7]
                );
            }
            drop(child);
        });
        let provider = scope.spawn(move || {
            // Discovery occurs after accepted root completion and callback return.
            // Bind real backing before transferring final-access responsibility.
            let child = continuation
                .child(continuation.inputs().unwrap().clone())
                .unwrap();
            assert!(handoff.send(child).is_ok());
            drop(continuation);
            drop(provider_slot);
        });
        provider.join().unwrap();
        assert_eq!(provider_slots.snapshot().active, 0);
        assert_eq!(production.snapshot().active_accesses, 1);
        assert_eq!(production_allowance.snapshot().active, 1);
        assert_eq!(accounting.snapshot().temporary_bytes, 3);
        assert!(matches!(production.claim(), SCProductionClaim::Pending));
        assert_eq!(cleanup.drain(), 0);
        assert!(drop_observations.try_recv().is_err());

        finish.send(()).unwrap();
        device.join().unwrap();
    });
    assert_eq!(production.snapshot().active_accesses, 0);
    assert_eq!(production_allowance.snapshot().active, 0);
    assert!(matches!(
        production.claim(),
        SCProductionClaim::Ready(Ok(15))
    ));
    // Final external access ends readiness gating, while deferred physical
    // destruction and its charge still require service on the owner thread.
    assert_eq!(cleanup.pending(), 1);
    assert_eq!(accounting.snapshot().retiring_bytes, 3);
    assert!(drop_observations.try_recv().is_err());
    assert_eq!(cleanup.drain(), 1);
    let observation = drop_observations.recv().unwrap();
    assert_eq!(observation.thread, thread::current().id());
    assert_eq!(observation.total_bytes, 3);
    assert_eq!(observation.retiring_bytes, 3);
    assert_eq!(accounting.snapshot().total_declared_bytes, 0);
}
