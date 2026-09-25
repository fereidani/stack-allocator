#![cfg_attr(not(feature = "std"), no_std)]
#![warn(missing_docs)]
#![doc = include_str!("../README.md")]
#![cfg_attr(nightly, feature(allocator_api))]
use core::cell::UnsafeCell;
use core::mem::MaybeUninit;
use core::ptr::NonNull;
use core::sync::atomic::AtomicUsize;
use core::sync::atomic::Ordering;

extern crate alloc;

#[cfg(not(nightly))]
use allocator_api2::alloc::{AllocError, Allocator, Layout};
#[cfg(nightly)]
use core::alloc::{AllocError, Allocator, Layout};

/// A simple bump‑allocator that lives on the stack (or in static memory).
///
/// It will tries to reuse freed memory only if it is the most recently allocated block.
///
/// `N` is the size of the backing buffer in bytes.
pub struct StackAllocator<const N: usize> {
    /// The buffer that backs all allocations.
    buf: UnsafeCell<MaybeUninit<[u8; N]>>,
    /// Offset of the next free byte inside `buf`.
    offset: AtomicUsize,
}

impl<const N: usize> Default for StackAllocator<N> {
    fn default() -> Self {
        Self::new()
    }
}

unsafe impl<const N: usize> Send for StackAllocator<N> {}
unsafe impl<const N: usize> Sync for StackAllocator<N> {}

impl<const N: usize> StackAllocator<N> {
    /// Create a fresh allocator.  The buffer starts empty.
    pub const fn new() -> Self {
        Self {
            buf: UnsafeCell::new(MaybeUninit::uninit()),
            offset: AtomicUsize::new(0),
        }
    }

    /// Reset the allocator, discarding all previously allocated memory.
    ///
    /// # Safety
    /// the caller must guarantee that no live allocation
    /// created by this allocator is still in use.
    pub unsafe fn reset(&mut self) {
        self.offset.store(0, Ordering::Relaxed);
    }

    /// Align `addr` upwards to `align`.  `align` must be a power of two.
    #[inline]
    const fn align_up(addr: usize, align: usize) -> usize {
        (addr + align - 1) & !(align - 1)
    }

    /// Get the current offset of the allocator.
    pub fn current_offset(&self) -> usize {
        self.offset.load(Ordering::Acquire)
    }
}

unsafe impl<const N: usize> Allocator for StackAllocator<N> {
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        //println!("StackAllocator allocate: layout={:?}", layout);
        let base = self.buf.get() as usize;
        let mut current = self.offset.load(Ordering::Acquire);
        let mut start;
        loop {
            // Compute the aligned pointer address, then get the offset.
            let current_ptr = base + current;
            let aligned_ptr = Self::align_up(current_ptr, layout.align());
            start = aligned_ptr - base;
            let end = start.checked_add(layout.size()).ok_or(AllocError)?;

            // Ensure we stay inside the buffer.
            if end > N {
                return Err(AllocError);
            }

            // Update the bump pointer.
            if self
                .offset
                .compare_exchange(current, end, Ordering::Release, Ordering::Relaxed)
                .is_ok()
            {
                break;
            }
            current = self.offset.load(Ordering::Acquire);
        }
        // SAFETY: `start..end` is inside `self.buf` and properly aligned.
        let ptr = unsafe { self.buf.get().cast::<u8>().add(start) };
        Ok(NonNull::slice_from_raw_parts(
            NonNull::new(ptr).unwrap(),
            layout.size(),
        ))
    }

    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        //println!("StackAllocator deallocate: layout={:?}", layout);
        // Compute the start offset of the allocation.
        let base = self.buf.get() as usize;
        let start = ptr.as_ptr() as usize - base;
        let end = start + layout.size();

        // If this block is the most recent allocation, move the bump pointer back.
        let _ = self
            .offset
            .compare_exchange(end, start, Ordering::Release, Ordering::Relaxed);
    }

    unsafe fn grow(
        &self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
    ) -> Result<NonNull<[u8]>, AllocError> {
        /*println!(
            "StackAllocator grow: old={:?}, new={:?}",
            old_layout, new_layout
        );*/
        // `grow` is only allowed when the block being grown is the most recent allocation.
        // Compute the start offset of the existing allocation.
        let base = self.buf.get() as usize;
        let old_start = ptr.as_ptr() as usize - base;

        // Verify that the allocator's current offset matches the end of this allocation.
        let expected_offset = old_start + old_layout.size();
        let current_offset = self.offset.load(Ordering::Acquire);
        if current_offset != expected_offset {
            return Err(AllocError);
        }

        // The new layout must be at least as large as the old one.
        if new_layout.size() < old_layout.size() {
            return Err(AllocError);
        }

        // Reject alignment increases -- the existing pointer may not satisfy them.
        if new_layout.align() > old_layout.align() {
            return Err(AllocError);
        }

        // Compute the new end of the allocation, checking for overflow and buffer limits.
        let new_end = old_start.checked_add(new_layout.size()).ok_or(AllocError)?;
        if new_end > N {
            return Err(AllocError);
        }

        // Attempt to bump the allocator's offset forward. We don't retry on failure, since that
        // would require copying the data to a new location.
        if self
            .offset
            .compare_exchange(
                expected_offset,
                new_end,
                Ordering::Release,
                Ordering::Relaxed,
            )
            .is_err()
        {
            // Failed to grow in place (concurrent modification). Report failure so the
            // caller can fall back to allocate + copy + deallocate.
            return Err(AllocError);
        }
        // Return the same pointer, now representing a slice of the larger size.
        Ok(NonNull::slice_from_raw_parts(ptr, new_layout.size()))
    }

    unsafe fn shrink(
        &self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
    ) -> Result<NonNull<[u8]>, AllocError> {
        // Compute the start offset of the existing allocation.
        let base = self.buf.get() as usize;
        let old_start = ptr.as_ptr() as usize - base;

        // Verify that the allocator's current offset matches the end of this allocation.
        let expected_offset = old_start + old_layout.size();
        let current_offset = self.offset.load(Ordering::Acquire);
        if current_offset != expected_offset {
            return Err(AllocError);
        }

        // The new layout must be no larger than the old one.
        if new_layout.size() > old_layout.size() {
            return Err(AllocError);
        }

        // Reject alignment increases -- the existing pointer may not satisfy them.
        if new_layout.align() > old_layout.align() {
            return Err(AllocError);
        }

        // Compute the new end of the allocation.
        let new_end = old_start + new_layout.size();

        // Attempt to move the bump pointer backwards.
        // We simply don't care if this fails, as it only means that some other
        // allocation happened in the meantime.
        // In that case, the memory will be reclaimed later when `reset` is called.
        _ = self.offset.compare_exchange(
            expected_offset,
            new_end,
            Ordering::Release,
            Ordering::Relaxed,
        );

        // Return the same pointer, now representing a slice of the smaller size.
        Ok(NonNull::slice_from_raw_parts(ptr, new_layout.size()))
    }
}

/// A hybrid allocator that first tries to allocate from a stack‑backed bump
/// allocator and falls back to a user‑provided allocator.
///
/// `N` - size of the stack buffer in bytes.
/// `F` - the fallback allocator type (e.g. `std::alloc::Global` or any custom
/// allocator that implements `Allocator`).
pub struct HybridAllocator<const N: usize, F: Allocator> {
    stack_alloc: StackAllocator<N>,
    fallback: F,
}

#[cfg(feature = "alloc")]
impl<const N: usize> Default for HybridAllocator<N, alloc::alloc::Global> {
    fn default() -> Self {
        Self::new(alloc::alloc::Global)
    }
}

impl<const N: usize, F: Allocator> HybridAllocator<N, F> {
    /// Create a new hybrid allocator.
    ///
    /// The caller supplies the fallback allocator that will be used when the
    /// stack buffer cannot satisfy a request.
    pub const fn new(fallback: F) -> Self {
        Self {
            stack_alloc: StackAllocator::new(),
            fallback,
        }
    }

    /// Reset the allocator, discarding all previously allocated memory.
    ///
    /// # Safety
    /// the caller must guarantee that no live allocation
    /// created by this allocator is still in use.
    pub unsafe fn reset(&mut self) {
        self.stack_alloc.reset();
    }

    /// Get the current offset of the stack allocator.
    pub fn current_offset(&self) -> usize {
        self.stack_alloc.current_offset()
    }

    /// Get a reference to the fallback allocator.
    pub fn fallback(&self) -> &F {
        &self.fallback
    }

    /// Check if the last allocation used the fallback allocator.
    /// if true, the stack buffer is exhausted and all further allocations
    /// will go to the fallback until reset is called.
    pub fn is_stack_exhausted(&self) -> bool {
        self.current_offset() >= N
    }
}

unsafe impl<const N: usize, F: Allocator> Allocator for HybridAllocator<N, F> {
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        // Try the stack allocator first; on failure delegate to the fallback.
        match self.stack_alloc.allocate(layout) {
            ok @ Ok(_) => ok,
            Err(_) => self.fallback.allocate(layout),
        }
    }

    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        // Determine whether the pointer belongs to the stack buffer.
        let base = self.stack_alloc.buf.get() as usize;
        let end = base + N;
        let addr = ptr.as_ptr() as usize;

        if (base..end).contains(&addr) {
            self.stack_alloc.deallocate(ptr, layout);
        } else {
            self.fallback.deallocate(ptr, layout);
        }
    }

    unsafe fn grow(
        &self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
    ) -> Result<NonNull<[u8]>, AllocError> {
        let base = self.stack_alloc.buf.get() as usize;
        let addr = ptr.as_ptr() as usize;

        if (base..base + N).contains(&addr) {
            // Attempt to grow in place using the stack allocator.
            if let Ok(res) = self.stack_alloc.grow(ptr, old_layout, new_layout) {
                return Ok(res);
            } else {
                // We need to alloc manually a new block and copy the data.
                let new_ptr = self.fallback.allocate(new_layout)?;
                core::ptr::copy_nonoverlapping(
                    ptr.as_ptr(),
                    new_ptr.as_ptr() as *mut u8,
                    old_layout.size(),
                );
                // Deallocate the old block.
                self.stack_alloc.deallocate(ptr, old_layout);
                return Ok(new_ptr);
            }
        }
        // Fallback allocator handles the grow request.
        self.fallback.grow(ptr, old_layout, new_layout)
    }

    unsafe fn shrink(
        &self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
    ) -> Result<NonNull<[u8]>, AllocError> {
        let base = self.stack_alloc.buf.get() as usize;
        let addr = ptr.as_ptr() as usize;

        if (base..base + N).contains(&addr) {
            // Attempt to shrink in place using the stack allocator.
            if let Ok(res) = self.stack_alloc.shrink(ptr, old_layout, new_layout) {
                // StackAllocator will always succeed in shrinking.
                return Ok(res);
            }
        }
        // Fallback allocator handles the shrink request.
        self.fallback.shrink(ptr, old_layout, new_layout)
    }
}
