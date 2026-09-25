//! The `allocator-api2` trait, for crates such as `hashbrown` that do not use
//! the standard `Allocator` trait yet.

use core::{alloc::Layout, ptr::NonNull};

use allocator_api2::alloc::{AllocError, Allocator};

use crate::{AllocResult, BumpAllocator, HybridAllocator, StackAllocator};

/// Converts a result of the standard trait into one of `allocator-api2`.
fn convert(result: AllocResult) -> Result<NonNull<[u8]>, AllocError> {
    result.map_err(|_| AllocError)
}

/// Implements the `allocator-api2` trait by forwarding to the standard one.
macro_rules! forward {
    ([$($generics:tt)*] $allocator:ty) => {
        // SAFETY: Every method forwards to the standard `Allocator`
        // implementation, which upholds the same contract.
        unsafe impl<$($generics)*> Allocator for $allocator {
            #[cfg_attr(all(feature = "no-panic", not(debug_assertions)), no_panic::no_panic)]
            fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
                convert(core::alloc::Allocator::allocate(self, layout))
            }

            #[cfg_attr(all(feature = "no-panic", not(debug_assertions)), no_panic::no_panic)]
            unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
                // SAFETY: The caller upholds the contract of `deallocate`.
                unsafe { core::alloc::Allocator::deallocate(self, ptr, layout) }
            }

            #[cfg_attr(all(feature = "no-panic", not(debug_assertions)), no_panic::no_panic)]
            unsafe fn grow(
                &self,
                ptr: NonNull<u8>,
                old: Layout,
                new: Layout,
            ) -> Result<NonNull<[u8]>, AllocError> {
                // SAFETY: The caller upholds the contract of `grow`.
                convert(unsafe { core::alloc::Allocator::grow(self, ptr, old, new) })
            }

            #[cfg_attr(all(feature = "no-panic", not(debug_assertions)), no_panic::no_panic)]
            unsafe fn shrink(
                &self,
                ptr: NonNull<u8>,
                old: Layout,
                new: Layout,
            ) -> Result<NonNull<[u8]>, AllocError> {
                // SAFETY: The caller upholds the contract of `shrink`.
                convert(unsafe { core::alloc::Allocator::shrink(self, ptr, old, new) })
            }
        }
    };
}

forward!([const N: usize] &StackAllocator<N>);
forward!([const N: usize, F: core::alloc::Allocator] &HybridAllocator<N, F>);
forward!([const N: usize, F: core::alloc::Allocator] &BumpAllocator<N, F>);
