//! Tests for `StackAllocator`.

mod common;

use std::{alloc::Allocator, ptr, thread};

use common::{addr, check_resize, fill, holds, layout, layouts, PREFIXES};
use stack_allocator::StackAllocator;

const STACK_SIZE: usize = 8 * 1024;
#[cfg(feature = "allocator-api2")]
const BIG_STACK_SIZE: usize = 256 * 1024;
const MAX_USIZE: usize = STACK_SIZE / size_of::<usize>();

#[test]
#[cfg(feature = "allocator-api2")]
fn hash_brown_test() {
    let alloc = StackAllocator::<BIG_STACK_SIZE>::new();
    let mut map = hashbrown::HashMap::new_in(&alloc);
    for i in 0..MAX_USIZE {
        map.insert(i, i);
    }
    assert_eq!(map.len(), MAX_USIZE);
    for i in 0..MAX_USIZE {
        assert_eq!(map.get(&i), Some(&i));
    }
}

#[test]
#[cfg(feature = "allocator-api2")]
fn allocator_api2_collections() {
    let alloc = StackAllocator::<STACK_SIZE>::new();
    let mut v = allocator_api2::vec::Vec::new_in(&alloc);
    for i in 0..100u64 {
        v.push(i);
    }
    v.truncate(3);
    v.shrink_to_fit();
    assert_eq!(v.as_slice(), [0, 1, 2]);
    drop(v);
    drop(allocator_api2::boxed::Box::new_in((), &alloc));
    assert_eq!(alloc.current_offset(), 0);
}

#[test]
fn vec_test() {
    let alloc = StackAllocator::<STACK_SIZE>::new();
    let mut v = Vec::new_in(&alloc);
    for i in 0..MAX_USIZE {
        v.push(i);
    }
    assert_eq!(v.len(), MAX_USIZE);
    for (i, &val) in v.iter().enumerate() {
        assert_eq!(i, val);
    }
}

#[test]
fn two_vec_test() {
    let alloc = StackAllocator::<STACK_SIZE>::new();
    let mut v1 = Vec::with_capacity_in(MAX_USIZE / 2, &alloc);
    let mut v2 = Vec::with_capacity_in(MAX_USIZE / 2, &alloc);
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
fn vec_shrink_test() {
    let alloc = StackAllocator::<STACK_SIZE>::new();
    let mut v = Vec::with_capacity_in(MAX_USIZE, &alloc);
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
    assert_eq!(alloc.current_offset(), 0);
}

#[test]
fn interleaved_vecs_can_grow() {
    let alloc = StackAllocator::<STACK_SIZE>::new();
    let mut v1 = Vec::with_capacity_in(8, &alloc);
    let mut v2 = Vec::with_capacity_in(8, &alloc);
    // `v1` is not the latest block, so it moves to grow.
    for i in 0..64u64 {
        v1.push(i);
        v2.push(i * 2);
    }
    assert!(v1.iter().copied().eq(0..64));
    assert!(v2.iter().copied().eq((0..64).map(|i| i * 2)));
}

#[test]
fn boxes_and_statics() {
    static STATIC: StackAllocator<64> = StackAllocator::new();
    let alloc = StackAllocator::<64>::default();
    let a = Box::new_in(1u64, &alloc);
    let b = Box::new_in([2u8; 3], &STATIC);
    let unit = Box::new_in((), &alloc);
    assert_eq!((*a, *b, *unit), (1, [2; 3], ()));
}

#[test]
fn allocations_respect_alignment() {
    // Room for the worst-case padding of every alignment below.
    let alloc = StackAllocator::<{ 2 * STACK_SIZE }>::new();
    let stack = &alloc;
    for shift in 0..=12 {
        let align = 1 << shift;
        // A one-byte block first, so the next block needs padding.
        let byte = stack.allocate(layout(1, 1)).unwrap();
        let block = stack.allocate(layout(3, align)).unwrap();
        assert_eq!(addr(block) % align, 0, "align {align}");
        assert!(addr(block) > addr(byte));
    }
}

#[test]
fn resize_keeps_alignment_from_every_offset() {
    for prefix in 0..PREFIXES {
        for old in layouts() {
            for new in layouts() {
                check_resize(&StackAllocator::<1024>::new(), prefix, old, new);
            }
        }
    }
}

#[test]
fn over_aligned_values() {
    #[repr(align(64))]
    #[derive(Clone, Copy, Debug, PartialEq)]
    struct Line([u8; 64]);

    let alloc = StackAllocator::<{ 2 * STACK_SIZE }>::new();
    let byte = Box::new_in(1u8, &alloc);
    let wide = Box::new_in(u128::MAX, &alloc);
    let mut lines = Vec::new_in(&alloc);
    lines.extend((0..4).map(|i| Line([i; 64])));
    // A newer block makes the next growth move `lines`.
    let blocker = Box::new_in(0u8, &alloc);
    lines.extend((4..32).map(|i| Line([i; 64])));
    assert!(lines.iter().copied().eq((0..32).map(|i| Line([i; 64]))));
    assert_eq!(lines.as_ptr().addr() % 64, 0);
    assert_eq!(ptr::from_ref(&*wide).addr() % align_of::<u128>(), 0);
    assert_eq!((*byte, *wide, *blocker), (1, u128::MAX, 0));
}

#[test]
fn deallocate_reclaims_only_the_latest_block() {
    let mut alloc = StackAllocator::<64>::new();
    let stack = &alloc;
    let a = stack.allocate(layout(8, 1)).unwrap();
    let b = stack.allocate(layout(8, 1)).unwrap();
    // SAFETY: `a` is live and `layout(8, 1)` fits it.
    unsafe { stack.deallocate(a.cast(), layout(8, 1)) };
    assert_eq!(alloc.current_offset(), 16);
    // SAFETY: `b` is live and `layout(8, 1)` fits it.
    unsafe { stack.deallocate(b.cast(), layout(8, 1)) };
    assert_eq!(alloc.current_offset(), 8);
    alloc.reset();
    assert_eq!(alloc.current_offset(), 0);
}

#[test]
fn rejects_requests_that_do_not_fit() {
    let empty = StackAllocator::<0>::new();
    assert!((&empty).allocate(layout(0, 1)).is_err());
    let alloc = StackAllocator::<16>::new();
    let stack = &alloc;
    assert!(stack.allocate(layout(0, 1 << (usize::BITS - 2))).is_err());
    assert!(stack
        .allocate(layout(isize::MAX.unsigned_abs(), 1))
        .is_err());
    assert!(stack.allocate(layout(17, 1)).is_err());
    stack.allocate(layout(16, 1)).unwrap();
    // Even an empty block must start inside the buffer.
    assert!(stack.allocate(layout(0, 1)).is_err());
}

#[test]
fn grow_in_place_or_by_moving() {
    let alloc = StackAllocator::<64>::new();
    let stack = &alloc;
    let block = stack.allocate(layout(8, 8)).unwrap();
    fill(block, 1);
    // SAFETY: `block` is live and `layout(8, 8)` fits it.
    let grown = unsafe { stack.grow(block.cast(), layout(8, 8), layout(16, 8)) }.unwrap();
    assert_eq!(addr(grown), addr(block));
    let _latest = stack.allocate(layout(8, 8)).unwrap();
    // SAFETY: `grown` is live and `layout(16, 8)` fits it.
    let moved = unsafe { stack.grow(grown.cast(), layout(16, 8), layout(24, 8)) }.unwrap();
    assert_ne!(addr(moved), addr(grown));
    assert!(holds(moved, 8, 1));
}

#[test]
fn grow_to_stricter_alignment() {
    let alloc = StackAllocator::<256>::new();
    let stack = &alloc;
    let _pad = stack.allocate(layout(1, 1)).unwrap();
    let block = stack.allocate(layout(3, 1)).unwrap();
    assert_eq!(addr(block) % 2, 1);
    fill(block, 3);
    // SAFETY: `block` is live and `layout(3, 1)` fits it.
    let grown = unsafe { stack.grow(block.cast(), layout(3, 1), layout(32, 16)) }.unwrap();
    assert_eq!(addr(grown) % 16, 0);
    assert!(holds(grown, 3, 3));
}

#[test]
fn grow_fails_when_full() {
    let alloc = StackAllocator::<32>::new();
    let stack = &alloc;
    let block = stack.allocate(layout(16, 1)).unwrap();
    fill(block, 4);
    // SAFETY: `block` is live and `layout(16, 1)` fits it.
    assert!(unsafe { stack.grow(block.cast(), layout(16, 1), layout(33, 1)) }.is_err());
    assert!(holds(block, 16, 4));
    assert_eq!(alloc.current_offset(), 16);
}

#[test]
fn shrink_in_place() {
    let alloc = StackAllocator::<64>::new();
    let stack = &alloc;
    let older = stack.allocate(layout(16, 1)).unwrap();
    let latest = stack.allocate(layout(16, 1)).unwrap();
    fill(older, 5);
    // SAFETY: `older` is live and `layout(16, 1)` fits it.
    let shrunk = unsafe { stack.shrink(older.cast(), layout(16, 1), layout(8, 1)) }.unwrap();
    assert_eq!(addr(shrunk), addr(older));
    assert!(holds(shrunk, 8, 5));
    assert_eq!(alloc.current_offset(), 32);
    // SAFETY: `latest` is live and `layout(16, 1)` fits it.
    unsafe { stack.shrink(latest.cast(), layout(16, 1), layout(4, 1)) }.unwrap();
    assert_eq!(alloc.current_offset(), 20);
}

#[test]
fn shrink_to_stricter_alignment() {
    let alloc = StackAllocator::<256>::new();
    let stack = &alloc;
    let _pad = stack.allocate(layout(1, 1)).unwrap();
    let block = stack.allocate(layout(32, 1)).unwrap();
    assert_eq!(addr(block) % 2, 1);
    fill(block, 7);
    // SAFETY: `block` is live and `layout(32, 1)` fits it.
    let shrunk = unsafe { stack.shrink(block.cast(), layout(32, 1), layout(16, 16)) }.unwrap();
    assert_eq!(addr(shrunk) % 16, 0);
    assert!(holds(shrunk, 16, 7));
}

#[test]
fn concurrent_allocations_do_not_overlap() {
    let alloc = StackAllocator::<STACK_SIZE>::new();
    let stack = &alloc;
    thread::scope(|scope| {
        for seed in 0..4 {
            scope.spawn(move || {
                for round in 0..32 {
                    let size = 1 + round % 7 * 8;
                    let block = stack.allocate(layout(size, 8)).unwrap();
                    fill(block, seed);
                    thread::yield_now();
                    assert!(holds(block, size, seed));
                    // SAFETY: `block` is live and `layout(size, 8)` fits it.
                    unsafe { stack.deallocate(block.cast(), layout(size, 8)) };
                }
            });
        }
    });
}
