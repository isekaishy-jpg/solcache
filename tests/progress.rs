use std::cell::RefCell;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;
use std::sync::Mutex;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

use solcache::{SCAccountingDomain, SCAllocationClass, SCCleanupContext, SCCleanupSnapshot};

struct DropAction(Box<dyn FnMut()>);

impl Drop for DropAction {
    fn drop(&mut self) {
        (self.0)();
    }
}

#[test]
fn bounded_cleanup_retains_unselected_records_and_charges() {
    let domain = SCAccountingDomain::new();
    let context = SCCleanupContext::new();
    for value in 0..3 {
        drop(
            context
                .try_acquire(value, &domain, 7, SCAllocationClass::Resident)
                .unwrap(),
        );
    }
    assert_eq!(context.drain_budget(0), 0);
    assert_eq!(
        context.snapshot(),
        SCCleanupSnapshot {
            pending_records: 3,
            in_flight_records: 0
        }
    );
    assert_eq!(domain.snapshot().retiring_bytes, 21);
    assert_eq!(context.drain_budget(2), 2);
    assert_eq!(
        context.snapshot(),
        SCCleanupSnapshot {
            pending_records: 1,
            in_flight_records: 0
        }
    );
    assert_eq!(domain.snapshot().retiring_bytes, 7);
    assert_eq!(context.drain_budget(usize::MAX), 1);
    assert_eq!(context.snapshot(), SCCleanupSnapshot::default());
    assert_eq!(domain.snapshot().total_declared_bytes, 0);
    drop(
        context
            .try_acquire(99, &domain, 0, SCAllocationClass::Resident)
            .unwrap(),
    );
    assert_eq!(domain.snapshot().total_declared_bytes, 0);
    assert_eq!(
        context.snapshot(),
        SCCleanupSnapshot {
            pending_records: 1,
            in_flight_records: 0
        }
    );
    assert_eq!(context.drain_budget(1), 1);
    assert_eq!(context.snapshot(), SCCleanupSnapshot::default());
}

#[test]
fn destructor_reentry_observes_extracted_records_and_nested_cleanup() {
    let domain = SCAccountingDomain::new();
    let context = Rc::new(SCCleanupContext::new());
    let observations = Rc::new(RefCell::new(Vec::new()));
    let weak = Rc::downgrade(&context);
    let observed = Rc::clone(&observations);
    let callback_domain = domain.clone();
    let action = DropAction(Box::new(move || {
        let context = weak.upgrade().unwrap();
        observed.borrow_mut().push(context.snapshot());
        assert_eq!(callback_domain.snapshot().retiring_bytes, 5);
        let nested_context = Rc::downgrade(&context);
        let nested_observations = Rc::clone(&observed);
        let nested_domain = callback_domain.clone();
        let nested = DropAction(Box::new(move || {
            nested_observations
                .borrow_mut()
                .push(nested_context.upgrade().unwrap().snapshot());
            assert_eq!(nested_domain.snapshot().retiring_bytes, 8);
        }));
        drop(
            context
                .try_acquire(nested, &callback_domain, 3, SCAllocationClass::Resident)
                .unwrap(),
        );
        observed.borrow_mut().push(context.snapshot());
        assert_eq!(callback_domain.snapshot().retiring_bytes, 8);
        assert_eq!(context.drain_budget(1), 1);
        assert_eq!(callback_domain.snapshot().retiring_bytes, 5);
        observed.borrow_mut().push(context.snapshot());
    }));
    drop(
        context
            .try_acquire(action, &domain, 5, SCAllocationClass::Resident)
            .unwrap(),
    );
    assert_eq!(context.drain_budget(1), 1);
    assert_eq!(
        *observations.borrow(),
        vec![
            SCCleanupSnapshot {
                pending_records: 0,
                in_flight_records: 1
            },
            SCCleanupSnapshot {
                pending_records: 1,
                in_flight_records: 1
            },
            SCCleanupSnapshot {
                pending_records: 0,
                in_flight_records: 2
            },
            SCCleanupSnapshot {
                pending_records: 0,
                in_flight_records: 1
            },
        ]
    );
    assert_eq!(context.snapshot(), SCCleanupSnapshot::default());
}

#[test]
fn bounded_cleanup_panic_settles_selected_records_and_preserves_unselected() {
    let domain = SCAccountingDomain::new();
    let context = SCCleanupContext::new();
    // The selected tail contains one panic and one successful destructor.
    for panic_on_drop in [false, true, false] {
        let action = DropAction(Box::new(move || assert!(!panic_on_drop, "payload panic")));
        drop(
            context
                .try_acquire(action, &domain, 11, SCAllocationClass::Resident)
                .unwrap(),
        );
    }
    assert!(catch_unwind(AssertUnwindSafe(|| context.drain_budget(2))).is_err());
    assert_eq!(
        context.snapshot(),
        SCCleanupSnapshot {
            pending_records: 1,
            in_flight_records: 0
        }
    );
    assert_eq!(domain.snapshot().retiring_bytes, 11);
    assert_eq!(context.drain(), 1);
    assert_eq!(domain.snapshot().total_declared_bytes, 0);
}

struct BlockingDrop(Option<(Sender<()>, Mutex<Receiver<()>>)>);

impl Drop for BlockingDrop {
    fn drop(&mut self) {
        if let Some((entered, resume)) = &self.0 {
            entered.send(()).unwrap();
            resume
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(5))
                .unwrap();
        }
    }
}

#[test]
fn bounded_batch_does_not_consume_concurrent_final_release() {
    let domain = SCAccountingDomain::new();
    let context = SCCleanupContext::new();
    let (entered_sender, entered_receiver) = mpsc::channel();
    let (resume_sender, resume_receiver) = mpsc::channel();
    drop(
        context
            .try_acquire(
                BlockingDrop(Some((entered_sender, Mutex::new(resume_receiver)))),
                &domain,
                5,
                SCAllocationClass::Resident,
            )
            .unwrap(),
    );
    let later = context
        .try_acquire(BlockingDrop(None), &domain, 13, SCAllocationClass::Resident)
        .unwrap();
    thread::scope(|scope| {
        scope.spawn(move || {
            entered_receiver
                .recv_timeout(Duration::from_secs(5))
                .unwrap();
            drop(later);
            resume_sender.send(()).unwrap();
        });
        assert_eq!(context.drain_budget(2), 1);
    });
    assert_eq!(
        context.snapshot(),
        SCCleanupSnapshot {
            pending_records: 1,
            in_flight_records: 0
        }
    );
    assert_eq!(domain.snapshot().retiring_bytes, 13);
    assert_eq!(context.drain(), 1);
}
