//! Helpers shared by the integration tests.

#![allow(dead_code)]

use std::{
    alloc::{AllocError, Allocator, Global, Layout},
    mem::MaybeUninit,
    ptr::NonNull,
    slice,
    sync::atomic::{AtomicUsize, Ordering::Relaxed},
};

/// Number of start offsets that `check_resize` runs from.
pub const PREFIXES: usize = if cfg!(miri) { 9 } else { 64 };

pub fn layout(size: usize, align: usize) -> Layout {
    Layout::from_size_align(size, align).unwrap()
}

pub fn addr(block: NonNull<[u8]>) -> usize {
    block.cast::<u8>().as_ptr().addr()
}

/// Returns layouts that pair small sizes with alignments up to 256.
pub fn layouts() -> impl Iterator<Item = Layout> {
    let aligns: &[usize] = if cfg!(miri) {
        &[1, 8, 64]
    } else {
        &[1, 2, 4, 8, 16, 32, 64, 128, 256]
    };
    aligns
        .iter()
        .flat_map(|&align| [0, 1, 7, 33].map(|size| layout(size, align)))
}

fn pattern(seed: u8) -> impl Iterator<Item = u8> {
    (0..=u8::MAX).cycle().map(move |i| seed ^ i)
}

/// Writes a pattern derived from `seed` into `block`.
pub fn fill(block: NonNull<[u8]>, seed: u8) {
    let ptr = block.cast::<MaybeUninit<u8>>().as_ptr();
    // SAFETY: `block` is live, and `MaybeUninit` bytes need no initialization.
    let bytes = unsafe { slice::from_raw_parts_mut(ptr, block.len()) };
    for (byte, value) in bytes.iter_mut().zip(pattern(seed)) {
        byte.write(value);
    }
}

/// Returns `true` if the first `len` bytes of `block` hold the `fill` pattern.
pub fn holds(block: NonNull<[u8]>, len: usize, seed: u8) -> bool {
    // SAFETY: `fill` initialized at least `len` bytes of the live `block`.
    let bytes = unsafe { slice::from_raw_parts(block.cast::<u8>().as_ptr(), len) };
    bytes.iter().copied().eq(pattern(seed).take(len))
}

/// Allocates a `prefix`-byte block and an `old` block after it, resizes the
/// second one to `new`, and checks alignment and contents on the way.
pub fn check_resize(alloc: impl Allocator + Copy, prefix: usize, old: Layout, new: Layout) {
    let head = alloc.allocate(layout(prefix, 1)).unwrap();
    let block = alloc.allocate(old).unwrap();
    assert_eq!(addr(block) % old.align(), 0, "{prefix} {old:?}");
    fill(head, 1);
    fill(block, 2);
    // SAFETY: `block` is live and `old` fits it.
    let resized = unsafe {
        if new.size() >= old.size() {
            alloc.grow(block.cast(), old, new)
        } else {
            alloc.shrink(block.cast(), old, new)
        }
    };
    let resized = resized.unwrap();
    assert_eq!(addr(resized) % new.align(), 0, "{prefix} {old:?} {new:?}");
    assert!(holds(resized, old.size().min(new.size()), 2));
    assert!(holds(head, prefix, 1));
    // SAFETY: Both blocks are live and their layouts fit them.
    unsafe {
        alloc.deallocate(resized.cast(), new);
        alloc.deallocate(head.cast(), layout(prefix, 1));
    }
}

/// A `Global` wrapper that records its blocks and counts foreign pointers.
/// It never panics, so it works with the `no-panic` feature.
pub struct Tracking {
    blocks: [AtomicUsize; 64],
    foreign: AtomicUsize,
}

impl Tracking {
    pub const fn new() -> Self {
        Self {
            blocks: [const { AtomicUsize::new(0) }; 64],
            foreign: AtomicUsize::new(0),
        }
    }

    /// Returns the number of live blocks and of foreign pointers.
    pub fn counts(&self) -> (usize, usize) {
        let live = self.blocks.iter().filter(|b| b.load(Relaxed) != 0).count();
        (live, self.foreign.load(Relaxed))
    }

    /// Replaces the first slot that holds `from` with `to`.
    fn swap(&self, from: usize, to: usize) -> bool {
        self.blocks
            .iter()
            .any(|b| b.compare_exchange(from, to, Relaxed, Relaxed).is_ok())
    }
}

// SAFETY: `Global` allocates every block, and only its blocks go back to it.
unsafe impl Allocator for Tracking {
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        let block = Global.allocate(layout)?;
        if self.swap(0, block.cast::<u8>().as_ptr().addr()) {
            return Ok(block);
        }
        // SAFETY: `Global` just allocated `block` for `layout`.
        unsafe { Global.deallocate(block.cast(), layout) };
        Err(AllocError)
    }

    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        if self.swap(ptr.as_ptr().addr(), 0) {
            // SAFETY: `Global` allocated `ptr`, and `layout` fits it.
            unsafe { Global.deallocate(ptr, layout) };
        } else {
            self.foreign.fetch_add(1, Relaxed);
        }
    }
}
