AggCore → Rust AggregationCore: Minimal Bridging Plan

Goal
- Wire the C++ AggCore to a new Rust `reducer::aggregation_core::AggregationCore` that consumes the existing ElementQueues and prints basic info about messages (size, timestamp, rpc_id, source queue index).
- Keep everything compilable and runnable with the current reducer binary, using `cxx::bridge` for C++↔Rust calls.

Scope of this milestone
- Do not re‑implement aggregation logic yet.
- Only plumb ElementQueues into Rust and run a simple loop similar to `Core::handle_rpc` that reads, parses, and logs minimal info.
- Keep internal stats flushing and other infra intact on the C++ side where possible.

Current architecture (relevant pieces)
- C++ reducer Core handles: timers via libuv, ElementQueue plumbing, and VirtualClock updates (`reducer/core.cc`).
- AggCore extends CoreBase and currently uses `Core::run()` which drives `handle_rpc` via timers (`reducer/aggregation/agg_core.{h,cc}`).
- ElementQueue memory layout is C‑compatible (`util/element_queue.h`). Rust crate `crates/element-queue` matches that layout and can build a queue from a contiguous pointer.
- Rust crates already present: `timeslot::VirtualClock`, `element-queue`, `render_parser`, and `render/ebpf_net/aggregation` which exposes `all_message_metadata()`.
- Build system already supports `cxx::bridge` (`cmake/rust_cxxbridge.cmake`) and is used for reducer entrypoint and OTLP exporter.

Design overview
1) Rust side (in `crates/reducer`)
   - New module: `reducer::aggregation_core` providing struct `AggregationCore` with:
     - Fields: `Vec<ElementQueue>`, `VirtualClock`, and a `render_parser::Parser<()>` keyed by the render‑generated perfect hash for aggregation RPCs.
     - Perfect hash: use the render‑generated functions and constants in `crates/render/ebpf_net/aggregation/src/hash.rs` together with message metadata from `crates/render/ebpf_net/aggregation/src/wire_messages.rs` (e.g., `all_message_metadata()`).
     - `fn run(&mut self, ...)`: Round‑robin over queues; peek `u64` timestamp, update `VirtualClock`; if current, read and parse the message, then print: queue_index, rpc_id, timestamp, message.len(). Also call `clock.advance()` and print timeslot advances.
   - FFI shim file: `crates/reducer/src/ffi.rs` exposing extern "Rust" fns for C++ and an opaque Rust type:
     - Constructor/destructor: `aggregation_core_new(...) -> UniquePtr<AggregationCore>` and drop (drop will also stop if still running).
     - Methods: `aggregation_core_run(Pin<&mut AggregationCore>)` and `aggregation_core_stop(Pin<&mut AggregationCore>)`.
     - Queue view type: `EqView { data: *mut u8, n_elems: u32, buf_len: u32 }` so the base pointer is explicitly a byte pointer.
   - Implementation builds `ElementQueue` from `EqView` via `unsafe ElementQueue::new_from_contiguous(n_elems, buf_len, data)`, registers aggregation message metadata into the `Parser` using the render‑generated perfect hash, and loops similarly to `Core::handle_rpc` (no libuv timers for this milestone). Use a per‑queue batch cap of `kMaxRpcBatchPerQueue = 10_000` and optionally a soft time budget like `RPC_HANDLE_TIME` (~20ms) later.

2) C++ side (AggCore)
   - Do not call `add_rpc_clients` in `AggCore::AggCore`. Instead, construct the `EqView` vector directly from `matching_to_aggregation_queues.make_readers(shard_num)` and pass it into Rust to build the queues.
   - Hold an opaque pointer to the Rust core inside C++ `AggCore` (e.g., `rust::UniquePtr<AggregationCore>` via cxx). Store it as a member.
   - `AggCore::run()`:
     - Set the thread name (as today for debuggability).
     - Call `reducer_agg::aggregation_core_run(*rust_core_)`, which blocks until stop.
   - `AggCore::stop_async()`:
     - Call `reducer_agg::aggregation_core_stop(*rust_core_)` to stop the Rust loop.
     - Optionally call `Core::stop_async()` to keep shutdown semantics consistent.
   - Keep `write_internal_stats()` and `on_timeslot_complete()` as‑is for now. Future work can reintroduce periodic stats from Rust or C++.

3) Build system (CMake)
   - Add a second `cxx::bridge` for `crates/reducer/src/ffi.rs`, e.g. target name `reducer_aggregation_cxxbridge`.
   - Link `reducerlib` against `reducer_aggregation_cxxbridge` (similar to `reducer_entrypoint_cxxbridge` already in `reducer/CMakeLists.txt`).
   - Include the generated header from `AggCore` (e.g., `#include "reducer_aggregation_cxxbridge.h"`).

Rust implementation details
- `AggregationCore::new`
  - Build queues: `unsafe ElementQueue::new_from_contiguous(n_elems, buf_len, data_ptr)` for each `EqView`.
  - `clock.add_inputs(queues.len())`.
  - Build `Parser` using the render‑generated perfect hash (functions/constants from `aggregation/src/hash.rs`) and register `all_message_metadata()` entries.

- `AggregationCore::run`
  - Round‑robin over queues using an index (like `Core::next_rpc_client_`):
    - Skip if `!clock.can_update(i)`.
    - `let mut rb = queue.start_read();` (RAII guard).
    - For up to `kMaxRpcBatchPerQueue` and while `rb.peek_len().is_ok()` and `clock.can_update(i)`:
      - `let ts: u64 = rb.peek_value()?;` on error: log, `let _ = rb.read();` to drain, continue.
      - `clock.update(i, ts)`; on `PastTimeslot`: log and continue; on `NotPermitted`: continue.
      - If `clock.is_current(i)`: `let msg = rb.read()?;`
        - Feed to `parser.handle(msg_with_ts)` where `msg_with_ts` is a buffer including the 8‑byte timestamp prefix if required by parser; otherwise extract fields directly and print `[agg shard, eq=i, ts, rpc_id, size=msg.len()]`. On error, log and continue.
    - `rb.finish();` to publish heads.
  - If `clock.advance()` returns true, print a timeslot advance marker.
  - Cancellation: each instance holds a `tokio_util::sync::CancellationToken`. `run()` can spawn a small Tokio task that `select!`s on the cancellation token and drives the loop; `stop()` triggers the token. For this milestone, a simple loop on the current thread is acceptable; adding Tokio ensures clean async cancellation when integrating timers later.

FFI types and signatures (cxx::bridge)
- In `crates/reducer/src/ffi.rs`:
  - `#[cxx::bridge(namespace = "reducer_agg")] mod ffi { ... }`
  - Types and fns:
    - `struct EqView { data: *mut u8, n_elems: u32, buf_len: u32 }`
    - `extern "Rust" { type AggregationCore; fn aggregation_core_new(queues: &CxxVector<EqView>, shard: u32) -> UniquePtr<AggregationCore>; fn aggregation_core_run(self: Pin<&mut AggregationCore>); fn aggregation_core_stop(self: Pin<&mut AggregationCore>); }`
  - Destructor: dropping `UniquePtr<AggregationCore>` suffices; `aggregation_core_stop` is idempotent and can be called before drop to ensure termination.

C++ integration in AggCore
- Include the generated header: `reducer_aggregation_cxxbridge.h`.
- Member field: `rust::UniquePtr<reducer_agg::AggregationCore>`.
- In constructor: build `rust::Vec<reducer_agg::EqView>` from `matching_to_aggregation_queues.make_readers(shard_num)` and call `aggregation_core_new` to initialize the member.
- `run()`: call `aggregation_core_run(*core_)`.
- `stop_async()`: call `aggregation_core_stop(*core_)`, then optionally `Core::stop_async()`.

Logging output (first milestone)
- One line per handled message:
  - Format: `agg[shard={shard}] eq={i} ts={timestamp} rpc_id={rpc_id} size={len}`
- Also print timeslot advances: `agg[shard={shard}] timeslot_advanced`.

Testing strategy
- Unit scope: none required for this milestone.
- Manual / dev run:
  - Build reducer and run end‑to‑end with a collector; verify that aggregation thread prints flowing messages with sane rpc_ids and sizes.
  - Validate that shutdown signals stop the aggregation thread and program exits cleanly.

Edge cases and notes
- Parser needs metadata for all aggregation RPCs; use `render::ebpf_net::aggregation::all_message_metadata()`.
- Perfect hash function: use the render‑generated perfect hash for aggregation (from `hash.rs`).
- Stop semantics: prefer per‑instance `CancellationToken` and Tokio task for clean cancellation; fallback to an `AtomicBool` if we decide to avoid async for the first drop.
- Safety: `ElementQueue::new_from_contiguous` is unsafe; values from C++ must be correct (derived from live queues). Reads are strictly read‑only in aggregation.

Concrete file changes
- `crates/reducer/Cargo.toml`: add dependencies `element-queue`, `timeslot`, `render_parser`, `encoder_ebpf_net_aggregation`, and optionally `tokio` + `tokio-util` if we use cancellation tokens.
- `crates/reducer/src/aggregation_core.rs`: new module with `AggregationCore` implementation.
- `crates/reducer/src/ffi.rs`: new `cxx::bridge` with `EqView` and opaque `AggregationCore`; expose new/run/stop API.
- `reducer/CMakeLists.txt`: `add_rust_cxxbridge(reducer_aggregation_cxxbridge crates/reducer/src/ffi.rs)` and link `reducerlib` against it.
- `reducer/aggregation/agg_core.h/cc`: hold a `UniquePtr<AggregationCore>`, construct it in the ctor with the queues, and implement `run()`/`stop_async()` to call into Rust.

Acceptance criteria
- reducer builds successfully.
- On startup, aggregation shard threads run Rust `AggregationCore`.
- Messages are consumed from ElementQueues and logged with queue index, timestamp, rpc_id, and body length.
- reducer shutdown stops aggregation threads cleanly.
