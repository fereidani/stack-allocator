# Stack Allocator

[![Crates.io](https://img.shields.io/crates/v/stack-allocator.svg)](https://crates.io/crates/stack-allocator)
[![Documentation](https://docs.rs/stack-allocator/badge.svg)](https://docs.rs/stack-allocator)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

This crate provides two allocator types:

- **`StackAllocator<N>`** - a bump‑allocator that uses a fixed‑size buffer stored on the stack (or in static memory). Allocations are fast and require no system calls. Individual blocks can be freed/shrinked/growed but only if they are the latest allocation.

- **`HybridAllocator<N, F>`** - a hybrid allocator that first tries to allocate from a `StackAllocator<N>` and, if the stack buffer is exhausted, falls back to a user‑provided allocator `F` (e.g. `std::alloc::Global`). This gives the performance benefits of stack allocation while still supporting unbounded allocations via the fallback.

## Features

- **`allocator_api`** - uses the nightly `allocator_api` feature to implement `std::alloc::Allocator`.
- **`allocator-api2`** – A stable fallback that mirrors the core allocation API, allowing this crate to be used on stable Rust with libraries like `hashbrown` that depend on `allocator-api2`.
  **Note:** This crate is `#![no_std]` compatible.

## Usage

```rust:ignore
#![feature(allocator_api)]
use stack_allocator::{StackAllocator, HybridAllocator};
use std::alloc::Global;

// A pure stack allocator with a 1 KiB buffer.
let mut stack = StackAllocator::<1024>::new();
let mut v = Vec::new_in(stack);
for i in 0..10 {
    v.push(i);
}
assert_eq!(v.len(), 10);
for (i, &val) in v.iter().enumerate() {
    assert_eq!(i, val);
}
v.clear();
assert_eq!(v.len(), 0);
v.shrink_to_fit();
assert_eq!(v.capacity(), 0);

// A hybrid allocator that falls back to the global allocator(heap).
let hybrid = HybridAllocator::<1024, Global>::new(Global);
let mut v = Vec::new_in(hybrid);
for i in 0..2048 {
    v.push(i);
}
assert_eq!(v.len(), 2048);
for (i, &val) in v.iter().enumerate() {
    assert_eq!(i, val);
}
v.clear();
assert_eq!(v.len(), 0);
v.shrink_to_fit();
assert_eq!(v.capacity(), 0);
```

The crate is `#![no_std]` compatible.
It is usable in stable rust using `allocator-api2`.
