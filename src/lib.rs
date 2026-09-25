#![no_std]
#![doc = include_str!("../README.md")]

#[cfg(feature = "alloc")]
extern crate alloc;

#[cfg(feature = "allocator-api2")]
mod api2;
mod bump;

#[cfg(feature = "alloc")]
use alloc::alloc::Global;
use core::{
    alloc::{AllocError, Allocator, Layout},
    cell::UnsafeCell,
    mem::MaybeUninit,
    ptr::{self, NonNull},
    sync::atomic::{
        AtomicUsize,
        Ordering::{self, Acquire, Relaxed, Release},
    },
};

// Debug assertions turn the `no-panic` check off, so mark the crate as used.
#[cfg(all(feature = "no-panic", debug_assertions))]
use no_panic as _;

pub use crate::bump::BumpAllocator;

/// The result of an allocation request.
type AllocResult = Result<NonNull<[u8]>, AllocError>;

/// Maximum compare-and-swap attempts per allocation, to bound its run time.
const MAX_CAS_ATTEMPTS: usize = 1024;

/// A bump allocator over an inline buffer of `N` bytes.
///
/// The allocator can live on the stack or in a `static`. Only the latest
/// block can give memory back or grow in place; [`reset`] reclaims the rest.
///
/// Only `&StackAllocator<N>` implements [`Allocator`], because moving the
/// allocator would move its buffer. All references to the same allocator are
/// equivalent allocators.
///
/// [`reset`]: StackAllocator::reset
///
/// # Examples
///
/// ```
/// use stack_allocator::StackAllocator;
///
/// let stack = StackAllocator::<64>::new();
/// let mut v = Vec::new_in(&stack);
/// v.extend_from_slice(&[1u32, 2, 3]);
/// assert_eq!(v, [1, 2, 3]);
/// ```
///
/// A collection cannot own the allocator:
///
/// ```compile_fail,E0277
/// use stack_allocator::StackAllocator;
///
/// let v: Vec<u8, StackAllocator<64>> = Vec::new_in(StackAllocator::new());
/// ```
pub struct StackAllocator<const N: usize> {
    /// The buffer that backs all allocations.
    buf: UnsafeCell<MaybeUninit<[u8; N]>>,
    /// Offset of the first free byte. Taking memory uses `Acquire` and giving
    /// it back uses `Release`, so successive owners of a block do not race.
    offset: AtomicUsize,
}

impl<const N: usize> Default for StackAllocator<N> {
    #[cfg_attr(all(feature = "no-panic", not(debug_assertions)), no_panic::no_panic)]
    fn default() -> Self {
        Self::new()
    }
}

// SAFETY: Threads only touch the buffer through disjoint blocks, which the
// atomic `offset` hands out.
unsafe impl<const N: usize> Sync for StackAllocator<N> {}

impl<const N: usize> StackAllocator<N> {
    /// Creates an allocator with an empty buffer.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            buf: UnsafeCell::new(MaybeUninit::uninit()),
            offset: AtomicUsize::new(0),
        }
    }

    /// Frees every block, making the whole buffer available again.
    ///
    /// Collections borrow the allocator, so this cannot run while they live:
    ///
    /// ```compile_fail,E0502
    /// use stack_allocator::StackAllocator;
    ///
    /// let mut stack = StackAllocator::<64>::new();
    /// let v: Vec<u8, _> = Vec::new_in(&stack);
    /// stack.reset();
    /// drop(v);
    /// ```
    #[cfg_attr(all(feature = "no-panic", not(debug_assertions)), no_panic::no_panic)]
    pub fn reset(&mut self) {
        *self.offset.get_mut() = 0;
    }

    /// Returns the number of bytes in use, including padding and freed blocks
    /// that are not reclaimed yet.
    #[must_use]
    #[cfg_attr(all(feature = "no-panic", not(debug_assertions)), no_panic::no_panic)]
    pub fn current_offset(&self) -> usize {
        self.offset.load(Acquire)
    }

    /// Returns a pointer to the buffer. `UnsafeCell` makes it writable.
    fn base(&self) -> NonNull<u8> {
        NonNull::from(&self.buf).cast()
    }

    /// Returns `true` if `ptr` points into the buffer.
    fn owns(&self, ptr: NonNull<u8>) -> bool {
        self.span(ptr, Layout::new::<()>()).is_some()
    }

    /// Returns the `(start, end)` offsets of a block at `ptr` with the size of
    /// `layout`, if it starts and ends inside the buffer.
    fn span(&self, ptr: NonNull<u8>, layout: Layout) -> Option<(usize, usize)> {
        span(self.base(), N, ptr, layout.size())
    }

    /// Returns the `(start, end)` offsets of a new block for `layout` at the
    /// first aligned address at or after `offset`, if it fits.
    fn fit(&self, offset: usize, layout: Layout) -> Option<(usize, usize)> {
        fit(self.base(), N, offset, layout)
    }

    /// Sets the offset to `new` if it still equals `current`.
    fn set_offset(&self, current: usize, new: usize, order: Ordering) -> bool {
        self.offset
            .compare_exchange(current, new, order, Relaxed)
            .is_ok()
    }

    /// Resizes the block at `ptr` in place if its address suits `new`, and
    /// moves it otherwise.
    ///
    /// # Safety
    ///
    /// `ptr` must denote a live block of this allocator that `old` fits.
    unsafe fn resize(&self, ptr: NonNull<u8>, old: Layout, new: Layout) -> AllocResult {
        let span = self.span(ptr, old);
        debug_assert!(span.is_some(), "foreign block");
        let (_, old_end) = span.ok_or(AllocError)?;
        let aligned = is_aligned(ptr, new.align());
        if let Some((_, new_end)) = self.span(ptr, new).filter(|_| aligned) {
            let grows = new_end > old_end;
            let order = if grows { Acquire } else { Release };
            // Growing needs the latest block; shrinking works on any block.
            if self.set_offset(old_end, new_end, order) || !grows {
                return Ok(NonNull::slice_from_raw_parts(ptr, new.size()));
            }
        }
        // SAFETY: The caller upholds the contract of `move_to`.
        unsafe { self.move_to(&self, ptr, old, new) }
    }

    /// Moves the block at `ptr` into a new block from `target`, then frees the
    /// old block. On error the old block is untouched.
    ///
    /// # Safety
    ///
    /// `ptr` must denote a live block of this allocator that `old` fits.
    unsafe fn move_to<A: Allocator>(
        &self,
        target: &A,
        ptr: NonNull<u8>,
        old: Layout,
        new: Layout,
    ) -> AllocResult {
        debug_assert!(self.span(ptr, old).is_some(), "foreign block");
        let new_ptr = target.allocate(new)?;
        let len = old.size().min(new.size());
        // SAFETY: Both blocks hold `len` bytes, and they cannot overlap while
        // the old one is allocated.
        unsafe {
            ptr::copy_nonoverlapping(ptr.as_ptr(), new_ptr.cast().as_ptr(), len);
            self.deallocate(ptr, old);
        }
        Ok(new_ptr)
    }
}

// SAFETY: Blocks are disjoint parts of the buffer, and the buffer cannot move,
// reset, or drop while a `&StackAllocator` exists.
unsafe impl<const N: usize> Allocator for &StackAllocator<N> {
    #[cfg_attr(all(feature = "no-panic", not(debug_assertions)), no_panic::no_panic)]
    fn allocate(&self, layout: Layout) -> AllocResult {
        for _ in 0..MAX_CAS_ATTEMPTS {
            let current = self.offset.load(Relaxed);
            let (start, end) = self.fit(current, layout).ok_or(AllocError)?;
            if self.set_offset(current, end, Acquire) {
                // SAFETY: `fit` keeps `start` inside the buffer.
                let ptr = unsafe { self.base().add(start) };
                debug_assert!(is_aligned(ptr, layout.align()));
                return Ok(NonNull::slice_from_raw_parts(ptr, layout.size()));
            }
        }
        Err(AllocError)
    }

    #[cfg_attr(all(feature = "no-panic", not(debug_assertions)), no_panic::no_panic)]
    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        // allocator-api2's `Box` frees empty blocks it never allocated.
        if layout.size() == 0 {
            return;
        }
        let span = self.span(ptr, layout);
        debug_assert!(span.is_some(), "foreign block");
        if let Some((start, end)) = span {
            // Only the latest block gives its memory back.
            self.set_offset(end, start, Release);
        }
    }

    #[cfg_attr(all(feature = "no-panic", not(debug_assertions)), no_panic::no_panic)]
    unsafe fn grow(&self, ptr: NonNull<u8>, old: Layout, new: Layout) -> AllocResult {
        debug_assert!(new.size() >= old.size());
        // SAFETY: The caller upholds the contract of `grow`.
        unsafe { self.resize(ptr, old, new) }
    }

    #[cfg_attr(all(feature = "no-panic", not(debug_assertions)), no_panic::no_panic)]
    unsafe fn shrink(&self, ptr: NonNull<u8>, old: Layout, new: Layout) -> AllocResult {
        debug_assert!(new.size() <= old.size());
        // SAFETY: The caller upholds the contract of `shrink`.
        unsafe { self.resize(ptr, old, new) }
    }
}

/// An allocator that serves requests from an inline `N`-byte
/// [`StackAllocator`] and falls back to `F`, for example `Global`.
///
/// Stack blocks that outgrow the buffer move to `F`. Like [`StackAllocator`],
/// it implements [`Allocator`] for shared references only.
///
/// # Examples
///
/// ```
/// use std::alloc::Global;
///
/// use stack_allocator::HybridAllocator;
///
/// let hybrid = HybridAllocator::<64, Global>::new(Global);
/// let mut v = Vec::new_in(&hybrid);
/// // The first elements fit in the stack buffer, the rest spill to the heap.
/// for i in 0..100u32 {
///     v.push(i);
/// }
/// assert!(v.iter().copied().eq(0..100));
/// ```
pub struct HybridAllocator<const N: usize, F: Allocator> {
    stack_alloc: StackAllocator<N>,
    fallback: F,
}

#[cfg(feature = "alloc")]
impl<const N: usize> Default for HybridAllocator<N, Global> {
    #[cfg_attr(all(feature = "no-panic", not(debug_assertions)), no_panic::no_panic)]
    fn default() -> Self {
        Self::new(Global)
    }
}

impl<const N: usize, F: Allocator> HybridAllocator<N, F> {
    /// Creates an allocator with an empty stack buffer that falls back to
    /// `fallback`.
    #[must_use]
    pub const fn new(fallback: F) -> Self {
        Self {
            stack_alloc: StackAllocator::new(),
            fallback,
        }
    }

    /// Frees every stack block. Blocks of the fallback allocator stay.
    #[cfg_attr(all(feature = "no-panic", not(debug_assertions)), no_panic::no_panic)]
    pub fn reset(&mut self) {
        self.stack_alloc.reset();
    }

    /// Returns the [`StackAllocator::current_offset`] of the stack buffer.
    #[must_use]
    #[cfg_attr(all(feature = "no-panic", not(debug_assertions)), no_panic::no_panic)]
    pub fn current_offset(&self) -> usize {
        self.stack_alloc.current_offset()
    }

    /// Returns a reference to the fallback allocator.
    #[must_use]
    pub const fn fallback(&self) -> &F {
        &self.fallback
    }

    /// Returns `true` if the stack buffer is full.
    ///
    /// A request larger than the free space goes to the fallback earlier.
    #[must_use]
    #[cfg_attr(all(feature = "no-panic", not(debug_assertions)), no_panic::no_panic)]
    pub fn is_stack_exhausted(&self) -> bool {
        self.current_offset() >= N
    }
}

// SAFETY: The stack buffer cannot move while borrowed. Empty blocks own no
// memory and never reach either allocator, so `owns` only routes real blocks,
// which cannot overlap.
unsafe impl<const N: usize, F: Allocator> Allocator for &HybridAllocator<N, F> {
    #[cfg_attr(all(feature = "no-panic", not(debug_assertions)), no_panic::no_panic)]
    fn allocate(&self, layout: Layout) -> AllocResult {
        if layout.size() == 0 {
            return dangling(layout);
        }
        (&self.stack_alloc)
            .allocate(layout)
            .or_else(|_| self.fallback.allocate(layout))
    }

    #[cfg_attr(all(feature = "no-panic", not(debug_assertions)), no_panic::no_panic)]
    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        // allocator-api2's `Box` also frees empty blocks it never allocated.
        if layout.size() == 0 {
            return;
        }
        let stack = &self.stack_alloc;
        // SAFETY: `owns` picks the allocator that handed out `ptr`.
        unsafe {
            if stack.owns(ptr) {
                stack.deallocate(ptr, layout);
            } else {
                self.fallback.deallocate(ptr, layout);
            }
        }
    }

    #[cfg_attr(all(feature = "no-panic", not(debug_assertions)), no_panic::no_panic)]
    unsafe fn grow(&self, ptr: NonNull<u8>, old: Layout, new: Layout) -> AllocResult {
        if old.size() == 0 {
            return self.allocate(new);
        }
        let stack = &self.stack_alloc;
        // SAFETY: `owns` picks the allocator that handed out `ptr`, and a
        // failed `grow` leaves the block untouched.
        unsafe {
            if !stack.owns(ptr) {
                return self.fallback.grow(ptr, old, new);
            }
            stack
                .grow(ptr, old, new)
                .or_else(|_| stack.move_to(&self.fallback, ptr, old, new))
        }
    }

    #[cfg_attr(all(feature = "no-panic", not(debug_assertions)), no_panic::no_panic)]
    unsafe fn shrink(&self, ptr: NonNull<u8>, old: Layout, new: Layout) -> AllocResult {
        if new.size() == 0 {
            // SAFETY: The caller upholds the contract of `deallocate`.
            unsafe { self.deallocate(ptr, old) };
            return dangling(new);
        }
        let stack = &self.stack_alloc;
        // SAFETY: `owns` picks the allocator that handed out `ptr`, and a
        // failed `shrink` leaves the block untouched.
        unsafe {
            if !stack.owns(ptr) {
                return self.fallback.shrink(ptr, old, new);
            }
            stack
                .shrink(ptr, old, new)
                .or_else(|_| stack.move_to(&self.fallback, ptr, old, new))
        }
    }
}

/// Returns the `(start, end)` offsets of a block at `ptr` with `size` bytes, if
/// it starts and ends inside the `len` bytes at `base`.
fn span(base: NonNull<u8>, len: usize, ptr: NonNull<u8>, size: usize) -> Option<(usize, usize)> {
    let start = ptr.as_ptr().addr().wrapping_sub(base.as_ptr().addr());
    let end = start.checked_add(size)?;
    (start < len && end <= len).then_some((start, end))
}

/// Returns the `(start, end)` offsets of a new block for `layout` in the `len`
/// bytes at `base`, at the first aligned address at or after `offset`.
fn fit(base: NonNull<u8>, len: usize, offset: usize, layout: Layout) -> Option<(usize, usize)> {
    debug_assert!(offset <= len, "offset past the buffer");
    // Pad based on the absolute address.
    let addr = base.as_ptr().addr().wrapping_add(offset);
    let padding = addr.wrapping_neg() & (layout.align() - 1);
    let start = offset.checked_add(padding)?;
    let end = start.checked_add(layout.size())?;
    // Empty blocks also start inside the buffer, so `owns` recognizes them.
    (start < len && end <= len).then_some((start, end))
}

/// Returns an empty block for `layout` that owns no memory.
fn dangling(layout: Layout) -> AllocResult {
    let ptr = NonNull::new(ptr::without_provenance_mut(layout.align())).ok_or(AllocError)?;
    Ok(NonNull::slice_from_raw_parts(ptr, 0))
}

/// Returns `true` if `ptr` is a multiple of `align`, a power of two.
fn is_aligned(ptr: NonNull<u8>, align: usize) -> bool {
    debug_assert!(align.is_power_of_two());
    ptr.as_ptr().addr() & (align - 1) == 0
}
