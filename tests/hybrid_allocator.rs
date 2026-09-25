#![cfg_attr(feature = "nightly", feature(allocator_api))]
#![allow(unused)]

use std::sync::Arc;

use stack_allocator::HybridAllocator;

const STACK_SIZE: usize = 8 * 1024;
const MAX_USIZE: usize = STACK_SIZE / std::mem::size_of::<usize>();

#[test]
#[cfg(feature = "nightly")]
fn vec_hybrid_test() {
    use std::alloc::Global;
    let hybrid_alloc: HybridAllocator<1024, Global> = HybridAllocator::<1024, _>::new(Global);
    let alloc = Arc::new(hybrid_alloc);
    let mut v = Vec::with_capacity_in(MAX_USIZE * 2, alloc.clone());
    for i in 0..(MAX_USIZE * 2) {
        v.push(i);
    }
    assert_eq!(v.len(), MAX_USIZE * 2);
    for (i, &val) in v.iter().enumerate() {
        assert_eq!(i, val);
    }
}

#[test]
#[cfg(not(feature = "nightly"))]
fn hash_brown_test() {
    use allocator_api2::alloc::Global;
    let alloc: HybridAllocator<1024, Global> = HybridAllocator::<1024, _>::new(Global);
    let mut map = hashbrown::HashMap::new_in(alloc);
    for i in 0..MAX_USIZE {
        map.insert(i, i);
    }
    assert_eq!(map.len(), MAX_USIZE);
    for i in 0..MAX_USIZE {
        assert_eq!(map.get(&i), Some(&i));
    }
    // move it to heap now
    let mut heap_map = hashbrown::HashMap::with_capacity(map.capacity());
    heap_map.extend(map.into_iter());
}
