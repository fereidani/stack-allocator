//! Tests for `BumpAllocator`.

mod common;

use std::{
    alloc::{Allocator, Global},
    thread,
};

use common::{addr, check_resize, layout, layouts, Tracking, PREFIXES};
use stack_allocator::BumpAllocator;

#[test]
fn vec_spans_sections() {
    let tracking = Tracking::new();
    {
        let bump = BumpAllocator::<64, _>::new(&tracking);
        let mut v = Vec::new_in(&bump);
        for i in 0..1000u64 {
            v.push(i);
        }
        assert!(v.iter().copied().eq(0..1000));
        assert!(bump.sections() > 1);
        assert_eq!(tracking.counts(), (bump.sections(), 0));
    }
    // Dropping the allocator returns every section.
    assert_eq!(tracking.counts(), (0, 0));
}

#[test]
fn latest_block_is_reclaimed() {
    let bump = BumpAllocator::<64, Global>::new(Global);
    let b = &bump;
    let first = b.allocate(layout(8, 1)).unwrap();
    let latest = b.allocate(layout(8, 1)).unwrap();
    // SAFETY: `first` is live and `layout(8, 1)` fits it.
    unsafe { b.deallocate(first.cast(), layout(8, 1)) };
    assert_eq!(bump.current_offset(), 16);
    // SAFETY: `latest` is live and `layout(8, 1)` fits it.
    unsafe { b.deallocate(latest.cast(), layout(8, 1)) };
    assert_eq!(bump.current_offset(), 8);
}

#[test]
fn large_requests_get_their_own_section() {
    let bump = BumpAllocator::<64, Global>::new(Global);
    let big = (&bump).allocate(layout(1000, 4096)).unwrap();
    assert_eq!((big.len(), addr(big) % 4096, bump.sections()), (1000, 0, 1));
}

#[test]
fn reset_keeps_the_newest_section() {
    let tracking = Tracking::new();
    let mut bump = BumpAllocator::<64, _>::new(&tracking);
    for _ in 0..3 {
        let _ = (&bump).allocate(layout(64, 1)).unwrap();
    }
    assert_eq!((bump.sections(), tracking.counts()), (3, (3, 0)));
    bump.reset();
    assert_eq!((bump.sections(), bump.current_offset()), (1, 0));
    assert_eq!(tracking.counts(), (1, 0));
    let _ = (&bump).allocate(layout(64, 1)).unwrap();
    assert_eq!(bump.sections(), 1);
    drop(bump);
    assert_eq!(tracking.counts(), (0, 0));
}

#[test]
fn empty_blocks_use_no_memory() {
    let bump = BumpAllocator::<64, Global>::new(Global);
    let b = &bump;
    let empty = b.allocate(layout(0, 64)).unwrap();
    assert_eq!((empty.len(), addr(empty) % 64, bump.sections()), (0, 0, 0));
    // SAFETY: Each block is live and the old layout fits it.
    unsafe {
        let grown = b.grow(empty.cast(), layout(0, 64), layout(8, 1)).unwrap();
        assert_eq!((bump.sections(), bump.current_offset()), (1, 8));
        let shrunk = b.shrink(grown.cast(), layout(8, 1), layout(0, 1)).unwrap();
        assert_eq!(bump.current_offset(), 0);
        b.deallocate(shrunk.cast(), layout(0, 1));
    }
}

#[test]
fn resize_keeps_alignment_from_every_offset() {
    let tracking = Tracking::new();
    for prefix in 0..PREFIXES {
        for old in layouts() {
            for new in layouts() {
                check_resize(&BumpAllocator::<64, _>::new(&tracking), prefix, old, new);
            }
        }
    }
    assert_eq!(tracking.counts(), (0, 0));
}

#[test]
fn moves_to_another_thread() {
    let bump = BumpAllocator::<64, Global>::new(Global);
    let sum = thread::spawn(move || {
        let mut v = Vec::new_in(&bump);
        v.extend_from_slice(&[1u8, 2, 3]);
        v.iter().sum::<u8>()
    });
    assert_eq!(sum.join().unwrap(), 6);
}

#[test]
#[cfg(feature = "alloc")]
fn default_uses_global() {
    let bump = BumpAllocator::<64, Global>::default();
    let boxed = Box::new_in(5u32, &bump);
    assert_eq!(*boxed, 5);
}

#[test]
#[cfg(feature = "allocator-api2")]
fn allocator_api2_collections() {
    let bump = BumpAllocator::<64, Global>::new(Global);
    let mut map = hashbrown::HashMap::new_in(&bump);
    map.extend((0..100u32).map(|i| (i, i)));
    assert_eq!(map[&42], 42);
    let mut v = allocator_api2::vec::Vec::new_in(&bump);
    for i in 0..100u64 {
        v.push(i);
    }
    v.truncate(3);
    v.shrink_to_fit();
    assert_eq!(v.as_slice(), [0, 1, 2]);
}
