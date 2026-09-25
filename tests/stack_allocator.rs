#![cfg_attr(feature = "nightly", feature(allocator_api))]
#![allow(unused)]

use std::sync::Arc;

use stack_allocator::StackAllocator;

const STACK_SIZE: usize = 8 * 1024;
const BIG_STACK_SIZE: usize = 256 * 1024;
const MAX_USIZE: usize = STACK_SIZE / std::mem::size_of::<usize>();

#[test]
#[cfg(not(feature = "nightly"))]
fn hash_brown_test() {
    let alloc = StackAllocator::<BIG_STACK_SIZE>::new();
    let mut map = hashbrown::HashMap::new_in(alloc);
    for i in 0..MAX_USIZE {
        map.insert(i, i);
    }
    assert_eq!(map.len(), MAX_USIZE);
    for i in 0..MAX_USIZE {
        assert_eq!(map.get(&i), Some(&i));
    }
}

#[test]
#[cfg(feature = "nightly")]
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
#[cfg(feature = "nightly")]
fn two_vec_test() {
    let alloc = Arc::new(StackAllocator::<STACK_SIZE>::new());
    let mut v1 = Vec::with_capacity_in(MAX_USIZE / 2, alloc.clone());
    let mut v2 = Vec::with_capacity_in(MAX_USIZE / 2, alloc.clone());
    for i in 0..(MAX_USIZE / 2) {
        v1.push(i);
        v2.push(i + (MAX_USIZE / 2));
    }
    assert_eq!(v1.len(), MAX_USIZE / 2);
    assert_eq!(v2.len(), MAX_USIZE / 2);
    for (i, &val) in v1.iter().enumerate() {
        assert_eq!(i, val);
    }
    for (i, &val) in v2.iter().enumerate() {
        assert_eq!(i + (MAX_USIZE / 2), val);
    }
}

#[test]
#[cfg(feature = "nightly")]
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
