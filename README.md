# Velvet
Velvet is a Rust library for ergonomic, efficient **divide-and-conquer parallelism**. Built entirely in safe Rust, Velvet contains *no hidden* ```unsafe```, giving you parallel speedups while preserving the language's signature safety guarantees. Velvet's interface is **macro based**; simply annotate your recursive functions to automatically generate efficient parallel execution code.

*Note: Velvet is a research project under development.*

---

## Features
- **Declarative parallelism via `#[spawnable]` macro** : Annotate recursive functions with `#[spawnable]` to automatically generate parallel execution code, without manual thread management.
- **Dynamic load balancing via work stealing**: Tasks are dynamically distributed to idle workers via work-stealing. In divide-and-conquer workloads, large tasks are created, and stolen, first, while finer-grained tasks are created later and are less likely to be stolen. This makes work-stealing especially efficient and reduces load-balancing overhead.
- **Automatic synchronization and scheduling**: Spawned tasks are synchronized automatically; no need for manual synchronization or barriers.
- **Configurable worker pools with thread pinning**: Control the number of worker threads and optionally pin them to specific CPU cores for predictable performance.
- **Multiple work queue backends**: Three queue backends are supported: safe (default), Crossbeam, and unsafe. It is easy to experiment with diffent backends and add your own.
- **Lightweight, compile-time code generation**: Parallelized code is generated at compile time via standard Rust metaprogramming features, keeping runtime overhead minimal.
- **Shared‑memory only**: Designed for parallelism on multi-core systems; Velvet does not provide a distributed runtime (...yet!)  

---

## Setup

*Before you begin, ensure that [both Rust and Cargo are installed](https://rust-lang.org/tools/install/) on your system.*

### 1. Add Velvet to your Cargo manifest
Add Velvet as both **build dependency** and **dev dependency**:

```toml
[dependencies]
velvet = { git = "https://github.com/liokouras/Velvet.git"}

[build-dependencies]
velvet = { git = "https://github.com/liokouras/Velvet.git"}
```

(or, if you have a local copy of Velvet, add it via `path = path/to/velvet`)

### 2. Create or update your build script (`build.rs`)
Velvet relies on a build script to generate parallel code. Either create the file `build.rs` in your package root (the directory containing `Cargo.toml`) or modify your existing one:

```rust
fn main() {
    let paths = vec![
        "src/main.rs", // list every file containing spawnable functions
    ];

    velvet::generate(paths);

    // recommended: ensure Cargo rebuilds when these files change
    for p in &paths {
        println!("cargo:rerun-if-changed={}", p);
    }
}
```

### 3. Update your crate root

In your crate root (usually `src/main.rs` or `src/lib.rs`), bring the Velvet API into scope and include the build script-generated code:

```rust
use velvet::prelude::*;
include!(concat!(env!("OUT_DIR"), "/velvet_app.rs"));
```

---

## Writing Spawnable Functions

A *spawnable function* is a recursive function annotated with the `spawnable` macro:

```rust
#[spawnable]
fn fib(n: u64) -> u64 {
    if n < 2 {
        return n;
    }
    let a = fib(n - 1);
    let b = fib(n - 2);
    return a + b;
}
```

The function that invokes your spawnable function must also be annotated as follows:
```rust
#[velvet_main(fib)]
fn main() {
    fib(42);
}
```

Both `spawnable` and `velvet_main` macros are exposed through Velvet's prelude. If you define spawnable functions outside your crate root, be sure to bring the prelude into scope in those files as well.


### Notes, Rules & Constraints
#### 1. **`velvet_main` and the Thread Pool**
The function annotated with `velvet_main` serves as the entry point for parallel execution. This is where Velvet creates and manages the thread pool for spawned tasks.
As a result:
- Each Velvet application should have at most one `velvet_main` function, unless you explicitly want to create multiple independent thread pools.
- All top-level calls to spawnable functions must originate from the same `velvet_main` function. However, this is likely to change with the introduction of a `spawn!()` macro for more developer control (see [Notes for Future Features](#notes-for-future-features))

#### 2. **Arguments & Return Types**
To uphold thread-safety, all arguments and return values to/from spawnable functions must be [`Send + 'static`](https://doc.rust-lang.org/std/marker/trait.Send.html).
Non-`'static` references cannot be transferred between threads safely, so Velvet treats `&T` in function signatures as if they were [`Arc<T>`](https://doc.rust-lang.org/std/sync/struct.Arc.html) and will call `.clone()` when the value is passed to another thread. 
If your spawnable function passes values by reference, make sure the values are wrapped in an `Arc` (this does not require changes to the function signature, as Rust's *deref coercion* allows an `Arc<T>` to be used where a `&T` is expected.).
If the datatype of an argument or return value is externally defined, ensure it is brought into scope using its **full qualified path** rather than relying on wildcard imports. This allows Velvet to resolve and generate code for the type correctly.

#### 3. **Unique Names**
Spawnable function names must be unique across the entire crate, even if defined in different modules.

#### 4. **Loop Constraints**
Velvet supports spawning in a loop, but there are some constraints to ensure correct parallel execution.
In general, the use of return values from recursive calls must be uniform and not depend on variables that are only defined inside the loop body. This is because when Velvet synchronizes spawned calls, information about locally scoped variables is not preserved.
For example, the following is supported:
```rust
    for val in input_collection {
        output_collection.push(recursive_call(val));
    }
```
Here, the return value of `recursive_call()` is used in the same way (pushed to `output_collection`), independent of any loop-local variables.
However, this is not supported:
```rust
    for (i, val) in input_collection.enumerate() {
        output_collection[i] = recursive_call(val); // i is loop-local and changes each iteration
    }
```
Neither is this:
```rust
    for (i, val) in input_collection.enumerate() {
        let res = recursive_call(val);
        output_collection[i] = res; // both i and res are loop-local and change each iteration
    }
```
In these cases, the use return values from recursive calls depends on loop-local variables (`i` or `res`) whose values are not tracked by Velvet after spawning.

#### 5. **Explicit Returns**
For functions with return values, use explicit `return val;`
This is required for Velvet to insert synchronization points correctly.

#### 6. **Sequential Thresholds**
Most parallel applications use a threshold to decide when to stop spawning parallel tasks and execute them directly. In general, the recursive base-case will act as such a threshold. However, parallel divide-and-conquer often benefits from a more coarse-grained threshold, which is easy to implement:
```rust
#[spawnable]
fn fib(n: u64) -> u64 {
    if n < THRESHOLD {
        return fib_seq(n);
    }
    let a = fib(n - 1);
    let b = fib(n - 2);
    return a + b;
}

fn fib_seq(n: u64) -> u64 {
    // ...
}
```
Note that `fib_seq` is not marked as `spawnable` so will execute sequentially.

---

## Shared Mutable State

If a spawnable function writes to shared memory, declare it explicitly:

```rust
#[spawnable(shared = ["shared_var_name"])]
```

Velvet uses this to insert appropriate synchronization barriers.
TODO: example

---

## Runtime Configuration
By default, Velvet runs the parallel application on as many threads as are availble on the system (probed via `std::thread::available_parallelism()`). However, this can also be adjusted via the `VELVET_WORKERS` environment variable. This must be set *before* compiling the package.

TODO: differnet queue backends


---
## Examples

TODO !!

---

## License

Apache 2.0, see [LICENSE](https://github.com/liokouras/velvet/blob/main/LICENSE)

---

## Notes for Future Features

- Optional argument to `velvet_main` for runtime configuration.
- `spawn!()` and `sync!()` macros for more involved control flow and programmer control.
- Better error messages in code generation

---

Enjoy fast and ergonomic parallel recursion!  
For questions, help, and feedback, feel free to open an issue.
