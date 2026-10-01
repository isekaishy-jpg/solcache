use std::hash::{Hash, Hasher};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use super::SCPublication;
use crate::{SCDependencies, SCLookup};

struct Key {
    value: u8,
    panic_on_drop: bool,
}

impl Hash for Key {
    fn hash<H: Hasher>(&self, hasher: &mut H) {
        self.value.hash(hasher);
    }
}
impl PartialEq for Key {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value
    }
}
impl Eq for Key {}
impl Drop for Key {
    fn drop(&mut self) {
        assert!(!self.panic_on_drop, "stored key cleanup panic");
    }
}

#[test]
fn panicking_key_cleanup_does_not_retain_removed_publication_metadata() {
    let mut publication = SCPublication::<_, ()>::new();
    let identity = publication.ensure(Key {
        value: 1,
        panic_on_drop: true,
    });
    let authority = publication.authority(&identity).unwrap();
    // A weak observation measures actual authority retention without adding an owner.
    // Public APIs intentionally do not expose these bookkeeping allocations.
    let retained_authority = Arc::downgrade(&authority.token);
    drop(authority);
    let search = Key {
        value: 1,
        panic_on_drop: false,
    };
    let panic = catch_unwind(AssertUnwindSafe(|| publication.remove(&search)))
        .expect_err("stored key must panic");
    assert_eq!(
        panic.downcast_ref::<&str>(),
        Some(&"stored key cleanup panic")
    );
    let dependencies = SCDependencies::new();
    assert!(matches!(
        publication.lookup(&identity, &dependencies).unwrap(),
        SCLookup::Absent
    ));
    assert!(
        retained_authority.upgrade().is_none(),
        "removed authority leaked after key destructor panic"
    );
    let fresh = publication.ensure(Key {
        value: 1,
        panic_on_drop: false,
    });
    assert_ne!(identity, fresh);
    assert!(publication.authority(&fresh).is_some());
}
