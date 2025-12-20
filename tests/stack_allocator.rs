#![cfg_attr(nightly, feature(allocator_api))]
#![allow(unused)]

use stack_allocator::StackAllocator;
use std::sync::Arc;

const STACK_SIZE: usize = 8 * 1024;
const MAX_USIZE: usize = STACK_SIZE / std::mem::size_of::<usize>();

#[test]
#[cfg(nightly)]
fn vec_test() {
    let alloc = StackAllocator::<STACK_SIZE>::new();
    let mut v = Vec::new_in(alloc);
    for i in 0..MAX_USIZE {
        v.push(i);
    }
    assert_eq!(v.len(), MAX_USIZE);
    for (i, &val) in v.iter().enumerate() {
        assert_eq!(i, val);
    }
}

#[test]
#[cfg(nightly)]
fn vec_shrink_test() {
    let alloc = Arc::new(StackAllocator::<STACK_SIZE>::new());
    let mut v = Vec::with_capacity_in(MAX_USIZE, alloc.clone());
    for i in 0..100 {
        v.push(i);
    }
    assert_eq!(v.len(), 100);
    for (i, &val) in v.iter().enumerate() {
        assert_eq!(i, val);
    }
    v.clear();
    assert_eq!(v.len(), 0);
    v.shrink_to_fit();
    assert_eq!(v.capacity(), 0);
}
