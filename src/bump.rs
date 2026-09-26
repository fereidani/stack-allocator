//! A bump allocator over sections taken from another allocator.

#[cfg(feature = "alloc")]
use alloc::alloc::Global;
use core::{
    alloc::{AllocError, Allocator, Layout},
    cell::Cell,
    ptr::{self, NonNull},
};

use crate::{dangling, fit, is_aligned, span, AllocResult};

/// The header at the start of every section, followed by its data.
struct Section {
    /// The section taken before this one.
    prev: Option<NonNull<Self>>,
    /// The layout of the whole section, header included.
    layout: Layout,
}

/// Size of the section header.
const HEADER: usize = size_of::<Section>();

/// A bump allocator over sections of `N` bytes that it takes from `F`, for
/// example `Global`.
///
/// It carves blocks from the newest section and takes a new one when a
/// request does not fit, so a request larger than `N` gets a section of its
/// own. Only the latest block gives memory back; [`reset`] and drop return
/// the sections to `F`.
///
/// Only `&BumpAllocator<N, F>` implements [`Allocator`], so collections
/// borrow the allocator. It is not `Sync`.
///
/// [`reset`]: BumpAllocator::reset
///
/// # Examples
///
/// ```
/// use std::alloc::Global;
///
/// use stack_allocator::BumpAllocator;
///
/// let bump = BumpAllocator::<256, Global>::new(Global);
/// let mut v = Vec::new_in(&bump);
/// v.extend(0..100u64);
/// assert!(v.iter().copied().eq(0..100));
/// assert_eq!(bump.sections(), 1);
/// ```
pub struct BumpAllocator<const N: usize, F: Allocator> {
    heap: F,
    /// The newest section, which serves new blocks.
    current: Cell<Option<NonNull<Section>>>,
    /// Offset of the first free byte in the data of the newest section.
    offset: Cell<usize>,
    /// Number of sections, which bounds the loops that free them.
    sections: Cell<usize>,
}

// SAFETY: The allocator owns its sections, and blocks are only handed out
// through `&self`, which stays on one thread because the type is not `Sync`.
unsafe impl<const N: usize, F: Allocator + Send> Send for BumpAllocator<N, F> {}

#[cfg(feature = "alloc")]
impl<const N: usize> Default for BumpAllocator<N, Global> {
    #[cfg_attr(all(no_panic, not(debug_assertions)), no_panic::no_panic)]
    fn default() -> Self {
        Self::new(Global)
    }
}

impl<const N: usize, F: Allocator> BumpAllocator<N, F> {
    /// Creates an allocator without sections that takes them from `heap`.
    #[must_use]
    pub const fn new(heap: F) -> Self {
        Self {
            heap,
            current: Cell::new(None),
            offset: Cell::new(0),
            sections: Cell::new(0),
        }
    }

    /// Frees every block at once. Keeps the newest section for reuse and
    /// returns the others to the heap allocator.
    #[cfg_attr(all(no_panic, not(debug_assertions)), no_panic::no_panic)]
    pub fn reset(&mut self) {
        let Some(current) = self.current.get() else {
            return;
        };
        // SAFETY: `current` is live, and `&mut self` means no block is used.
        let older = unsafe { (*current.as_ptr()).prev.take() };
        // SAFETY: The older sections are live and unreachable from now on.
        unsafe { self.free(older, self.sections.get().saturating_sub(1)) };
        self.sections.set(1);
        self.offset.set(0);
    }

    /// Returns the number of bytes in use at the start of the newest section.
    #[must_use]
    pub const fn current_offset(&self) -> usize {
        self.offset.get()
    }

    /// Returns the number of sections taken from the heap allocator.
    #[must_use]
    pub const fn sections(&self) -> usize {
        self.sections.get()
    }

    /// Returns a reference to the heap allocator.
    #[must_use]
    pub const fn heap(&self) -> &F {
        &self.heap
    }

    /// Returns the data of the newest section and its length.
    fn data(&self) -> Option<(NonNull<u8>, usize)> {
        let section = self.current.get()?;
        // SAFETY: `current` always points to a live section.
        let layout = unsafe { section.as_ref() }.layout;
        // SAFETY: The data follows the header in the same allocation.
        let data = unsafe { section.cast::<u8>().add(HEADER) };
        Some((data, layout.size().saturating_sub(HEADER)))
    }

    /// Carves a block for `layout` from the newest section, if it fits.
    fn bump(&self, layout: Layout) -> Option<NonNull<[u8]>> {
        let (data, len) = self.data()?;
        let (start, end) = fit(data, len, self.offset.get(), layout)?;
        self.offset.set(end);
        // SAFETY: `fit` keeps the block inside the data of the section.
        let ptr = unsafe { data.add(start) };
        debug_assert!(is_aligned(ptr, layout.align()));
        Some(NonNull::slice_from_raw_parts(ptr, layout.size()))
    }

    /// Takes a new section from the heap allocator with room for `layout`.
    fn add_section(&self, layout: Layout) -> Result<(), AllocError> {
        // The block plus its worst-case padding.
        let needed = layout.size().checked_add(layout.align() - 1);
        let size = needed.ok_or(AllocError)?.max(N).checked_add(HEADER);
        let section_layout =
            Layout::from_size_align(size.ok_or(AllocError)?, align_of::<Section>())
                .map_err(|_| AllocError)?;
        let section = self.heap.allocate(section_layout)?.cast::<Section>();
        let header = Section {
            prev: self.current.get(),
            layout: section_layout,
        };
        // SAFETY: The new allocation is large and aligned enough for a header.
        unsafe { section.write(header) };
        self.current.set(Some(section));
        self.offset.set(0);
        self.sections.set(self.sections.get().saturating_add(1));
        Ok(())
    }

    /// Returns the new end offset of the block at `ptr` resized from `old` to
    /// `new` bytes, if it is the latest block and the new size still fits.
    fn latest_end(&self, ptr: NonNull<u8>, old: usize, new: usize) -> Option<usize> {
        let (data, len) = self.data()?;
        let (_, old_end) = span(data, len, ptr, old)?;
        let (_, new_end) = span(data, len, ptr, new)?;
        (old_end == self.offset.get()).then_some(new_end)
    }

    /// Resizes the block at `ptr` in place if it can, and moves it otherwise.
    ///
    /// # Safety
    ///
    /// `ptr` must denote a live block of this allocator that `old` fits.
    unsafe fn resize(&self, ptr: NonNull<u8>, old: Layout, new: Layout) -> AllocResult {
        if old.size() == 0 {
            return self.allocate(new);
        }
        if new.size() == 0 {
            // SAFETY: The caller upholds the contract of `deallocate`.
            unsafe { self.deallocate(ptr, old) };
            return dangling(new);
        }
        if is_aligned(ptr, new.align()) {
            // The latest block grows or shrinks in place; others only shrink.
            if let Some(end) = self.latest_end(ptr, old.size(), new.size()) {
                self.offset.set(end);
                return Ok(NonNull::slice_from_raw_parts(ptr, new.size()));
            }
            if new.size() <= old.size() {
                return Ok(NonNull::slice_from_raw_parts(ptr, new.size()));
            }
        }
        let new_ptr = self.allocate(new)?;
        let len = old.size().min(new.size());
        // SAFETY: Both blocks hold `len` bytes, and they cannot overlap while
        // the old one is allocated.
        unsafe {
            ptr::copy_nonoverlapping(ptr.as_ptr(), new_ptr.cast().as_ptr(), len);
            self.deallocate(ptr, old);
        }
        Ok(new_ptr)
    }

    /// Returns up to `count` sections, starting at `section`, to the heap
    /// allocator.
    ///
    /// # Safety
    ///
    /// The sections must be live, and no block in them may be used again.
    unsafe fn free(&self, mut section: Option<NonNull<Section>>, count: usize) {
        for _ in 0..count {
            let Some(current) = section else {
                break;
            };
            // SAFETY: The caller guarantees that the section is live.
            let Section { prev, layout } = unsafe { current.read() };
            // SAFETY: The heap allocator handed out the section for `layout`.
            unsafe { self.heap.deallocate(current.cast(), layout) };
            section = prev;
        }
        debug_assert!(section.is_none(), "section count out of sync");
    }
}

impl<const N: usize, F: Allocator> Drop for BumpAllocator<N, F> {
    #[cfg_attr(all(no_panic, not(debug_assertions)), no_panic::no_panic)]
    fn drop(&mut self) {
        // SAFETY: Dropping takes ownership, so no block is used again.
        unsafe { self.free(self.current.get(), self.sections.get()) };
    }
}

// SAFETY: Blocks are disjoint parts of sections, which stay allocated until
// `reset` or drop. Both need `&mut self` or ownership, so no block outlives
// the borrow of the allocator.
unsafe impl<const N: usize, F: Allocator> Allocator for &BumpAllocator<N, F> {
    #[cfg_attr(all(no_panic, not(debug_assertions)), no_panic::no_panic)]
    fn allocate(&self, layout: Layout) -> AllocResult {
        if layout.size() == 0 {
            return dangling(layout);
        }
        if let Some(block) = self.bump(layout) {
            return Ok(block);
        }
        self.add_section(layout)?;
        self.bump(layout).ok_or(AllocError)
    }

    #[cfg_attr(all(no_panic, not(debug_assertions)), no_panic::no_panic)]
    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        // Only the latest block gives memory back; the rest waits for `reset`.
        if let Some(start) = self.latest_end(ptr, layout.size(), 0) {
            self.offset.set(start);
        }
    }

    #[cfg_attr(all(no_panic, not(debug_assertions)), no_panic::no_panic)]
    unsafe fn grow(&self, ptr: NonNull<u8>, old: Layout, new: Layout) -> AllocResult {
        debug_assert!(new.size() >= old.size());
        // SAFETY: The caller upholds the contract of `grow`.
        unsafe { self.resize(ptr, old, new) }
    }

    #[cfg_attr(all(no_panic, not(debug_assertions)), no_panic::no_panic)]
    unsafe fn shrink(&self, ptr: NonNull<u8>, old: Layout, new: Layout) -> AllocResult {
        debug_assert!(new.size() <= old.size());
        // SAFETY: The caller upholds the contract of `shrink`.
        unsafe { self.resize(ptr, old, new) }
    }
}
