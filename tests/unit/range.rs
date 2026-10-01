use super::{SCRangeError, SCRangeInsert, SCRangeStore, SCSourceSnapshot};
use crate::{SCAccountingDomain, SCAllocationClass, SCBacking, SCCountLimit};

#[test]
fn exhausted_sequence_and_entry_scores_reject_before_copy_or_replacement() {
    let domain = SCAccountingDomain::new();
    let source = SCSourceSnapshot::new(
        SCBacking::try_new((), &domain, 0, SCAllocationClass::Resident).unwrap(),
    );
    let mut store = SCRangeStore::new(SCCountLimit::new(1), &domain);
    store.insert(&source, 0, b"abcd").unwrap();
    let before = store.snapshot();
    store.sequence = u64::MAX;
    let mut destination = [b'?'; 4];
    assert_eq!(
        store.copy_prefix(&source, 0, &mut destination).err(),
        Some(SCRangeError::ScoreExhausted),
    );
    assert_eq!(&destination, b"????");
    assert_eq!(
        store.insert(&source, 1, b"x"),
        Err(SCRangeError::ScoreExhausted)
    );
    assert_eq!(store.snapshot(), before);
    assert_eq!(
        store
            .copy_prefix(&source, 4, &mut destination)
            .unwrap()
            .copied(),
        0
    );
    // Identical refresh needs only the local score, not shared sequence room.
    assert_eq!(
        store.insert(&source, 0, b"abcd"),
        Ok(SCRangeInsert::Refreshed)
    );
    store.sequence = 3;
    store.entries[0].as_mut().unwrap().score = u64::MAX;
    assert_eq!(
        store.insert(&source, 0, b"abcd"),
        Err(SCRangeError::ScoreExhausted)
    );
    assert_eq!(store.snapshot(), before);
    store.copy_prefix(&source, 0, &mut destination).unwrap();
    assert_eq!(&destination, b"abcd");
}
