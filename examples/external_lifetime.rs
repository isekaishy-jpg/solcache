use std::sync::mpsc;
use std::thread::{self, ThreadId};

use solcache::{
    SCAccountingDomain, SCAllocationClass, SCCleanupContext, SCProduction, SCProductionClaim,
    SCProviderDecision, SCSubmissionOutcome,
};

struct Bytes {
    data: Box<[u8]>,
    cleaned_on: mpsc::Sender<ThreadId>,
}

impl Drop for Bytes {
    fn drop(&mut self) {
        let _ = self.cleaned_on.send(thread::current().id());
    }
}

fn main() {
    let accounting = SCAccountingDomain::new();
    let cleanup = SCCleanupContext::new();
    let (cleaned_on, cleanup_thread) = mpsc::channel();
    let owner = cleanup
        .try_acquire(
            Bytes {
                data: vec![2u8, 4, 6].into_boxed_slice(),
                cleaned_on,
            },
            &accounting,
            3,
            SCAllocationClass::Resident,
        )
        .expect("declared allocation charge");
    // This simulated device reads immutable, already-ready bytes. Its separate
    // owner permits result application before device retirement. Hold an SC
    // production access guard instead when external work must gate readiness.
    let device_owner = owner.clone();
    let (production, submission) = SCProduction::<_, ()>::prepare(owner);
    thread::scope(|scope| {
        // On caller failure, disconnect before the scope joins its device thread.
        let (retire, retirement) = mpsc::channel();
        let device = scope.spawn(move || {
            if retirement.recv().is_ok() {
                assert_eq!(device_owner.view().get().data.as_ref(), &[2, 4, 6]);
            }
            drop(device_owner);
        });
        let decision = submission.submit(|mut producer| {
            producer
                .complete(Ok(producer.inputs().unwrap().clone()))
                .expect("first completion");
            drop(producer);
            SCProviderDecision::Accepted
        });
        assert!(matches!(decision, SCSubmissionOutcome::Accepted));
        let SCProductionClaim::Ready(Ok(result)) = production.claim() else {
            panic!("callback completed and ended its SC access");
        };
        assert_eq!(result.view().get().data.as_ref(), &[2, 4, 6]);
        drop(result);
        assert_eq!(accounting.snapshot().resident_bytes, 3);
        assert_eq!(cleanup.drain(), 0);

        retire.send(()).expect("release the simulated device");
        device
            .join()
            .expect("external final read and owner release");
    });
    assert_eq!(accounting.snapshot().retiring_bytes, 3);
    assert_eq!(cleanup.drain(), 1);
    assert_eq!(cleanup_thread.recv().unwrap(), thread::current().id());
    assert_eq!(accounting.snapshot().total_declared_bytes, 0);
    println!("Ready result, external final use, and owner-thread cleanup settled separately.");
}
