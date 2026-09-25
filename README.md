<div align="center">

# Stack Allocator

**Fast `no_std` stack allocator and bump allocator for Rust.**
Allocate `Vec`, `Box`, and `HashMap` from a fixed-size buffer on the stack or in a `static` with zero heap allocations, or fall back to the heap when the buffer runs out.

[![Crates.io version][crates-badge]][crates-url]
[![docs.rs documentation][doc-badge]][doc-url]
[![MIT license][mit-badge]][mit-url]

[crates-badge]: https://img.shields.io/crates/v/stack-allocator.svg?style=for-the-badge
[crates-url]: https://crates.io/crates/stack-allocator
[doc-badge]: https://img.shields.io/docsrs/stack-allocator?style=for-the-badge
[doc-url]: https://docs.rs/stack-allocator
[mit-badge]: https://img.shields.io/badge/license-MIT-blue.svg?style=for-the-badge
[mit-url]: https://github.com/fereidani/stack-allocator/blob/main/LICENSE

</div>

`stack-allocator` provides two memory allocators for the Rust `Allocator` API:

- **`StackAllocator<N>`** - a bump allocator (arena) over an `N`-byte buffer stored on the stack or in static memory. Allocations are fast and require no system calls. Only the latest block can give memory back or grow in place; other blocks move when they grow.
- **`HybridAllocator<N, F>`** - a hybrid allocator that first tries to allocate from a `StackAllocator<N>` and, if the stack buffer is exhausted, falls back to a user-provided allocator `F` (e.g. `Global`). This gives the performance benefits of stack allocation while still supporting unbounded allocations via the fallback.

## Why stack-allocator?

- **Zero heap allocations**: memory comes from an inline buffer, so hot paths make no `malloc` or system calls.
- **Constant-time bump allocation**: an allocation pads to the requested alignment and bumps a pointer.
- **Heap fallback on demand**: `HybridAllocator` spills to `Global`, or any other allocator, once the stack buffer is full.
- **Works with the ecosystem**: `allocator_api2::vec::Vec`, `allocator_api2::boxed::Box`, and `hashbrown::HashMap` on stable Rust, and `std::vec::Vec` on nightly Rust.
- **`no_std` and embedded ready**: no global allocator required, a `const fn` constructor for `static` buffers, and a lock-free, thread-safe bump pointer.
- **Sound and verified**: the borrow checker keeps the buffer in place while collections use it, Miri checks the tests for undefined behavior, and `no-panic` proves that the allocators cannot panic.

## Installation

```bash
cargo add stack-allocator
```

Add `Default` for `HybridAllocator<N, Global>` with the `alloc` feature, or use the standard library's `Allocator` trait on nightly Rust with the `nightly` feature:

```bash
cargo add stack-allocator --features alloc
cargo add stack-allocator --features nightly
```

## Usage

### Vec on the stack

```rust
use allocator_api2::vec::Vec;
use stack_allocator::StackAllocator;

// A pure stack allocator with a 1 KiB buffer.
let stack = StackAllocator::<1024>::new();
let mut v = Vec::new_in(&stack);
for i in 0..10 {
    v.push(i);
}
assert_eq!(v.len(), 10);
for (i, &val) in v.iter().enumerate() {
    assert_eq!(i, val);
}
v.clear();
v.shrink_to_fit();
assert_eq!(v.capacity(), 0);
```

### Hybrid stack and heap allocator

```rust
use allocator_api2::{alloc::Global, vec::Vec};
use stack_allocator::HybridAllocator;

// A 1 KiB stack buffer that falls back to the global allocator (heap).
let hybrid = HybridAllocator::<1024, Global>::new(Global);
let mut v = Vec::new_in(&hybrid);
for i in 0..2048 {
    v.push(i);
}
assert_eq!(v.len(), 2048);
assert!(v.iter().copied().eq(0..2048));
```

### HashMap on the stack with hashbrown

```rust
use hashbrown::HashMap;
use stack_allocator::StackAllocator;

let stack = StackAllocator::<4096>::new();
let mut scores = HashMap::new_in(&stack);
scores.insert("alice", 10);
scores.insert("bob", 7);
assert_eq!(scores["alice"], 10);
```

### Static allocator for no_std and embedded

`StackAllocator::new` is a `const fn` and the allocator is `Sync`, so it can back a `static` without a global heap:

```rust
use allocator_api2::vec::Vec;
use stack_allocator::StackAllocator;

static BUFFER: StackAllocator<1024> = StackAllocator::new();

let mut v = Vec::new_in(&BUFFER);
v.extend_from_slice(b"no heap");
assert_eq!(v.as_slice(), b"no heap");
```

### Nightly allocator_api with std::vec::Vec

```toml
[dependencies]
stack-allocator = { version = "0.2", features = ["nightly"] }
```

```rust,ignore
#![feature(allocator_api)]

use stack_allocator::StackAllocator;

let stack = StackAllocator::<1024>::new();
let mut v = Vec::new_in(&stack);
v.push(1);
```

## Cargo features

| Feature | Description |
| --- | --- |
| `nightly` | Implements the unstable `core::alloc::Allocator` for `std` collections such as `Vec::new_in`. Requires nightly, and `hashbrown` then needs its own `nightly` feature. |
| `alloc` | Implements `Default` for `HybridAllocator<N, Global>`. |
| `std` | Enables `alloc` and the `std` support of `allocator-api2`. |
| `no-panic` | Proves at link time that the allocators cannot panic, using [`no-panic`](https://crates.io/crates/no-panic). It only checks release builds, such as `cargo test --release --features no-panic`. |

No feature is enabled by default. Without `nightly`, the allocators implement the [`allocator-api2`](https://crates.io/crates/allocator-api2) trait, which works on stable Rust with `allocator_api2::vec::Vec`, `hashbrown`, and other crates built on it.

The crate is `#![no_std]` and needs a global allocator only with the `alloc` feature.

## How it works

- `StackAllocator<N>` owns an `N`-byte buffer and an atomic offset. It pads each block to its alignment based on the real address, so any alignment works in any buffer.
- `Allocator` is implemented for `&StackAllocator<N>` and `&HybridAllocator<N, F>`. Collections borrow the allocator, so the buffer cannot move or reset while they use it.
- Freeing or shrinking the latest block returns its memory at once. Other freed memory comes back with `reset`, which takes `&mut self`, so it compiles only once every collection has dropped.
- A block that cannot grow in place moves to a new block of the buffer. `HybridAllocator` moves it to the fallback allocator once the buffer is full.

## Testing

Every change runs through CI with:

- the test suite with every feature, on stable and nightly Rust;
- [Miri](https://github.com/rust-lang/miri) on 64-bit and 32-bit targets, to catch undefined behavior;
- `no-panic` on release builds with overflow checks, to prove that allocation, deallocation, grow, and shrink cannot panic;
- exhaustive tests that resize blocks from every start offset below 64 bytes, for every alignment up to 256 bytes;
- `clippy::pedantic` and `clippy::nursery` with warnings denied.

## License

`stack-allocator` is licensed under the MIT license. See the [LICENSE](https://github.com/fereidani/stack-allocator/blob/main/LICENSE) file for more information.
