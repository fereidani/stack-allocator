//! Tests for `HybridAllocator`.

#![cfg_attr(feature = "nightly", feature(allocator_api))]

mod common;

#[cfg(feature = "nightly")]
use std::{
    alloc::{Allocator, Global},
    vec::Vec,
};

#[cfg(not(feature = "nightly"))]
use allocator_api2::{
    alloc::{Allocator, Global},
    vec::Vec,
};
use common::{check_resize, layout, layouts, Tracking, PREFIXES};
use stack_allocator::HybridAllocator;

const STACK_SIZE: usize = 8 * 1024;
const MAX_USIZE: usize = STACK_SIZE / size_of::<usize>();

#[test]
fn resize_keeps_alignment_from_every_offset() {
    for prefix in 0..PREFIXES {
        for old in layouts() {
            for new in layouts() {
                let alloc = HybridAllocator::<64, Tracking>::new(Tracking::new());
                check_resize(&alloc, prefix, old, new);
                assert_eq!(alloc.fallback().counts(), (0, 0));
            }
        }
    }
}

#[test]
fn vec_hybrid_test() {
    let hybrid_alloc: HybridAllocator<1024, Global> = HybridAllocator::<1024, _>::new(Global);
    let alloc = &hybrid_alloc;
    let mut v = Vec::with_capacity_in(MAX_USIZE * 2, alloc);
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
    let alloc: HybridAllocator<1024, Global> = HybridAllocator::<1024, _>::new(Global);
    let mut map = hashbrown::HashMap::new_in(&alloc);
    for i in 0..MAX_USIZE {
        map.insert(i, i);
    }
    assert_eq!(map.len(), MAX_USIZE);
    for i in 0..MAX_USIZE {
        assert_eq!(map.get(&i), Some(&i));
    }
    // move it to heap now
    let mut heap_map = hashbrown::HashMap::with_capacity(map.capacity());
    heap_map.extend(map);
    assert_eq!(heap_map.len(), MAX_USIZE);
}

#[test]
#[cfg(feature = "alloc")]
fn default_falls_back_to_global() {
    let alloc = HybridAllocator::<16, Global>::default();
    let mut v = Vec::new_in(&alloc);
    v.extend(0..100u32);
    assert!(v.iter().copied().eq(0..100));
}

#[test]
fn shrinking_older_stack_block_stays_in_stack() {
    let alloc = HybridAllocator::<1024, Tracking>::new(Tracking::new());
    let mut older = Vec::with_capacity_in(64, &alloc);
    let newer: Vec<u8, _> = Vec::with_capacity_in(64, &alloc);
    older.push(1u8);
    older.shrink_to_fit();
    assert_eq!(older.as_slice(), [1]);
    drop((older, newer));
    assert_eq!(alloc.fallback().counts(), (0, 0));
}

#[test]
fn empty_block_on_full_stack_comes_from_fallback() {
    let alloc = HybridAllocator::<16, Tracking>::new(Tracking::new());
    let hybrid = &alloc;
    let full = hybrid.allocate(layout(16, 1)).unwrap();
    let empty = hybrid.allocate(layout(0, 1)).unwrap();
    assert_eq!(alloc.fallback().counts(), (1, 0));
    // SAFETY: Both blocks are live and their layouts fit them.
    unsafe {
        hybrid.deallocate(empty.cast(), layout(0, 1));
        hybrid.deallocate(full.cast(), layout(16, 1));
    }
    assert_eq!(alloc.fallback().counts(), (0, 0));
    assert_eq!(alloc.current_offset(), 0);
}

#[test]
fn zero_capacity_stack_uses_fallback_only() {
    let alloc = HybridAllocator::<0, Tracking>::new(Tracking::new());
    let hybrid = &alloc;
    for size in [0, 1, 64] {
        let block = hybrid.allocate(layout(size, 1)).unwrap();
        assert_eq!(alloc.fallback().counts(), (1, 0));
        // SAFETY: `block` is live and `layout(size, 1)` fits it.
        unsafe { hybrid.deallocate(block.cast(), layout(size, 1)) };
    }
    assert_eq!(alloc.fallback().counts(), (0, 0));
}

#[test]
fn outgrowing_the_stack_moves_block_to_fallback() {
    let alloc = HybridAllocator::<64, Tracking>::new(Tracking::new());
    let mut v = Vec::new_in(&alloc);
    for i in 0..64u64 {
        v.push(i);
    }
    assert!(v.iter().copied().eq(0..64));
    assert_eq!(alloc.fallback().counts(), (1, 0));
    // The vector was the latest stack block, so moving it emptied the stack.
    assert_eq!(alloc.current_offset(), 0);
    drop(v);
    assert_eq!(alloc.fallback().counts(), (0, 0));
}

#[test]
fn fallback_blocks_stay_in_fallback() {
    let alloc = HybridAllocator::<16, Tracking>::new(Tracking::new());
    let hybrid = &alloc;
    let block = hybrid.allocate(layout(32, 8)).unwrap();
    // SAFETY: Each block is live and the old layout fits it.
    unsafe {
        let grown = hybrid
            .grow(block.cast(), layout(32, 8), layout(64, 8))
            .unwrap();
        let shrunk = hybrid
            .shrink(grown.cast(), layout(64, 8), layout(8, 8))
            .unwrap();
        assert_eq!(alloc.fallback().counts(), (1, 0));
        hybrid.deallocate(shrunk.cast(), layout(8, 8));
    }
    assert_eq!(alloc.current_offset(), 0);
    assert_eq!(alloc.fallback().counts(), (0, 0));
}

#[test]
fn mixed_workload_routes_every_block_correctly() {
    let alloc = HybridAllocator::<256, Tracking>::new(Tracking::new());
    {
        let mut vecs = [
            Vec::new_in(&alloc),
            Vec::new_in(&alloc),
            Vec::new_in(&alloc),
        ];
        for (value, slot) in (0..200u32).zip((0..vecs.len()).cycle()) {
            vecs[slot].push(value);
            if value % 16 == 0 {
                vecs[slot].shrink_to_fit();
            }
        }
        for (slot, v) in vecs.iter().enumerate() {
            assert!(v.iter().copied().eq((0..200u32).skip(slot).step_by(3)));
        }
    }
    assert_eq!(alloc.fallback().counts(), (0, 0));
}

#[test]
fn reset_and_exhaustion() {
    let mut alloc = HybridAllocator::<16, Tracking>::new(Tracking::new());
    let hybrid = &alloc;
    assert!(!alloc.is_stack_exhausted());
    let _full = hybrid.allocate(layout(16, 1)).unwrap();
    assert!(alloc.is_stack_exhausted());
    let heap = hybrid.allocate(layout(8, 1)).unwrap();
    // SAFETY: `heap` is live and `layout(8, 1)` fits it.
    unsafe { hybrid.deallocate(heap.cast(), layout(8, 1)) };
    alloc.reset();
    assert!(!alloc.is_stack_exhausted());
    assert_eq!(alloc.fallback().counts(), (0, 0));
}
