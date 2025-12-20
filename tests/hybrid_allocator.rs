#![cfg_attr(nightly, feature(allocator_api))]
#![allow(unused)]

use stack_allocator::HybridAllocator;
use std::sync::Arc;

const STACK_SIZE: usize = 8 * 1024;
const MAX_USIZE: usize = STACK_SIZE / std::mem::size_of::<usize>();

#[test]
#[cfg(nightly)]
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
