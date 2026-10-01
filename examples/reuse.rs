use solcache::{
    SCAccountingDomain, SCAllocationClass, SCBacking, SCPool, SCRangeStore, SCSourceBuffers,
    SCSourceSnapshot,
};

fn main() {
    let allocations = SCAccountingDomain::new();
    let source_bytes = b"captured source bytes".to_vec();
    let capacity = source_bytes.capacity() as u64;
    let source = SCSourceSnapshot::new(
        SCBacking::try_new(
            source_bytes,
            &allocations,
            capacity,
            SCAllocationClass::Resident,
        )
        .unwrap(),
    );
    let mut ranges = SCRangeStore::reference(&allocations);
    ranges
        .insert(&source, 0, &source.context().get()[..8])
        .unwrap();

    let scratch = SCPool::new();
    let buffer = Vec::<u8>::with_capacity(64);
    let charge = allocations
        .charge(buffer.capacity() as u64, SCAllocationClass::Temporary)
        .unwrap();
    scratch.insert(1, buffer, charge).unwrap();
    let mut lease = scratch.checkout(1, capacity).unwrap().unwrap();
    lease.get_mut().resize(source.context().get().len(), 0);
    let read = ranges.copy_prefix(&source, 0, lease.get_mut()).unwrap();
    // A real provider would read the remainder through this same captured context.
    let start = read.remaining_offset() as usize;
    lease.get_mut()[read.copied()..]
        .copy_from_slice(&read.source().context().get()[start..start + read.remaining_length()]);
    assert_eq!(lease.get(), source.context().get());
    lease.reset_and_return(Vec::clear);

    let mut whole = SCSourceBuffers::new(0);
    let bytes = source.context().get().clone();
    let length = bytes.len() as u64;
    let charge = allocations
        .charge(bytes.capacity() as u64, SCAllocationClass::Resident)
        .unwrap();
    whole
        .insert_read(7, SCBacking::from_charged(bytes, charge), length)
        .unwrap();
    let taken = whole.take(7).unwrap();
    whole.close();
    assert_eq!(taken.view().get(), source.context().get());

    drop(taken);
    drop(read);
    ranges.close();
    scratch.close();
    drop(source);
    assert_eq!(allocations.snapshot().total_declared_bytes, 0);
}
