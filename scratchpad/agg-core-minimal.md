AggCore → Rust AggregationCore: Minimal Bridging Plan

Goal
- Wire the C++ AggCore to a new Rust reducer::aggregation_core::AggregationCore that consumes the existing ElementQueues and prints basic info about messages (size, timestamp, rpc_id, source queue index).
- Keep everything compilable and runnable with the current reducer binary, using cxx::bridge for C++↔Rust calls.

Scope of this milestone
- Do not re‑implement aggregation logic yet.
- Only plumb ElementQueues into Rust and run a simple loop “similar to Core::handle_rpc” that reads, parses, and logs minimal info.
- Keep internal stats flushing and other infra intact on the C++ side where possible.

Current architecture (relevant pieces)
- C++ reducer Core handles: timers via libuv, ElementQueue plumbing, and VirtualClock updates (reducer/core.cc).
- AggCore extends CoreBase and currently uses Core::run() which drives handle_rpc via timers (reducer/aggregation/agg_core.{h,cc}).
- ElementQueue memory layout is C‑compatible (util/element_queue.h). Rust crate crates/element-queue matches that layout and can build a queue from a contiguous pointer.
- Rust crates already present: timeslot::VirtualClock, element-queue, render_parser, and render/ebpf_net/aggregation which exposes all_message_metadata().
- Build system already supports cxx::bridge (cmake/rust_cxxbridge.cmake) and is used for reducer entrypoint and otlp exporter.

Design overview
1) Rust side (in crates/reducer)
   - New module: reducer::aggregation_core providing struct AggregationCore with:
     - Fields: Vec<ElementQueue>, VirtualClock, Parser<()> (with a large capacity perfect-hash map; initial hash function can be identity on u16 over capacity 65536 to avoid collisions).
     >> can use crates/render/ebpf_net/aggregation/src/hash.rs for the perfect hash and wire_messages.rs in the same directory for MessageMetadata
     - fn run(&mut self, stop_flag: &AtomicBool): Round‑robin over queues; peek u64 timestamp, update VirtualClock; if current, read message bytes, parse with Parser, print: queue_index, rpc_id, timestamp, message.len(). Also call clock.advance() and optionally print timeslot advances.
     >> Yes, please print timeslot advances
   - FFI shim file: crates/reducer/src/ffi.rs exposing extern "Rust" fns for C++ to call:
     - aggregation_core_run(queues: Vec<EqView>, shard: u32) -> blocks until stop_flag set.
     - aggregation_core_stop(shard: u32) to signal stop for that shard.
     - EqView is a POD: { data: usize, n_elems: u32, buf_len: u32 } representing the contiguous queue storage base pointer and sizes.
     >> I think it would be more stable to use a u8 pointer for data?
   - Implementation builds ElementQueue from EqView via unsafe new_from_contiguous, registers aggregation message metadata (render::ebpf_net::aggregation::all_message_metadata()) into the Parser, and loops similarly to Core::handle_rpc (without libuv timers for this milestone). Use a high‑level constant for per‑queue batch cap (kMaxRpcBatchPerQueue = 10_000) and optionally a soft time budget like RPC_HANDLE_TIME (~20ms) later.

2) C++ side (AggCore)
   - Override AggCore::run() to:
     - Gather a rust::Vec<EqView> with one entry per rpc_client_. For each client, compute:
       - data = reinterpret_cast<uintptr_t>(rpc_client.queue.shared) (base of contiguous block)
       - n_elems = rpc_client.queue.elem_mask + 1
       - buf_len = rpc_client.queue.buf_mask + 1
       >> I'm concerned it might not be safe to let rpc_client hold the queue. Can we instead hijack the queues and not pass them to add_rpc_clients in AggCore::AggCore, instead passing those queues to the Rust AggregationCore constructor via FFI, and holding an opaque pointer to it in the C++ AggCore. Then in run() we call the run method on the Rust Aggregation core.
     - Call reducer_agg::aggregation_core_run(eqs, shard_num()). This blocks until stop.
     - Maintain thread name setup (mirroring Core::run) for debuggability.
   - Override AggCore::stop_async() to:
     - Call reducer_agg::aggregation_core_stop(shard_num()) to flip the Rust stop flag for this shard.
     - Optionally also call Core::stop_async() to keep semantics aligned (harmless when not using libuv timers here).
   - Keep write_internal_stats() and on_timeslot_complete() as‑is (no change for this milestone). Future: if needed, a periodic internal‑stats timer can be re‑introduced for aggregation in Rust or C++.

3) Build system (CMake)
   - Add a second cxx::bridge for crates/reducer/src/ffi.rs, e.g. target name reducer_aggregation_cxxbridge.
   - Link reducerlib against reducer_aggregation_cxxbridge (similar to reducer_entrypoint_cxxbridge already in reducer/CMakeLists.txt).
   - Generated header to include from AggCore, e.g. #include "reducer_aggregation_cxxbridge.h".

Rust implementation details
- AggregationCore::new
  - Build queues: unsafe ElementQueue::new_from_contiguous(n_elems, buf_len, data_ptr)
  - clock.add_inputs(queues.len())
  - Build Parser with capacity 65536 and hash |k| k (identity). Register all_message_metadata() entries.

- AggregationCore::run(stop_flag)
  - Round‑robin over queues using an index (like Core::next_rpc_client_):
    - Skip if !clock.can_update(i)
    - queue.start_read_batch()
    - For up to kMaxRpcBatchPerQueue and while queue.peek() > 0 and clock.can_update(i):
      - let ts: u64 = queue.peek_value()?; on error log and queue.read() to drain, continue.
      - match clock.update(i, ts) { Ok(()) => {}, Err(PastTimeslot) => log error and continue; Err(NotPermitted) => continue; }
      - if clock.is_current(i):
        - let len = queue.read(ptr)
        - let data = slice::from_raw_parts(ptr as *const u8, len as usize)
        >> The ElementQueue interface already returns a slice, need to fix here
        - match parser.handle(data) { Ok(h) => extract rpc_id = u16::from_ne_bytes(h.message[0..2]), print: [agg shard, eq=i, ts=h.timestamp, rpc_id, size=h.message.len()], Err(e) => log minimal error and continue }
    - queue.finish_read_batch()
    >> The ElementQueue interface now has guards, look again..
  - If clock.advance() returns true, optionally print a timeslot advance marker.
  - Check stop_flag.load(Ordering::Relaxed) each outer loop; break when set.

FFI types and signatures (cxx::bridge)
- In crates/reducer/src/ffi.rs:
  - #[cxx::bridge(namespace = "reducer_agg")] mod ffi {
      struct EqView { data: usize, n_elems: u32, buf_len: u32 }
      extern "Rust" {
        fn aggregation_core_run(queues: &Vec<EqView>, shard: u32);
        fn aggregation_core_stop(shard: u32);
      }
    }
    >> Expose a Rust struct and create/destroy functions (destroy can also do stop). If these can look like a constructor/destructor that will be ideal IMO
- Rust keeps a global map shard->AtomicBool stop flags (or a small vec pre‑sized by core count) so stop works per shard.
>> each instance can hold a tokio_util::sync::CancellationToken and stop() would trigger it. On Run, we start a tokio task that select()s the CancellationToken

C++ integration in AggCore
- Include the generated header: reducer_aggregation_cxxbridge.h
- run():
  - rust::Vec<reducer_agg::EqView> v;
  - For each rpc_client_ index i:
    - auto &q = rpc_clients_[i].queue;
    - reducer_agg::EqView e{ (uintptr_t)q.shared, (uint32_t)(q.elem_mask + 1), (uint32_t)(q.buf_mask + 1) };
    - v.push_back(e);
  - reducer_agg::aggregation_core_run(v, (uint32_t)shard_num());
- stop_async():
  - reducer_agg::aggregation_core_stop((uint32_t)shard_num());
  - Core::stop_async(); // optional, harmless

Logging output (first milestone)
- One line per handled message:
  - Format: "agg[shard={shard}] eq={i} ts={timestamp} rpc_id={rpc_id} size={len}"
- Optionally print timeslot advances: "agg[shard={shard}] timeslot_advanced".

Testing strategy
- Unit scope: none required for this milestone.
- Manual / dev run:
  - Build reducer and run end‑to‑end with a collector; verify that aggregation thread prints flowing messages with sane rpc_ids and sizes.
  - Validate that shutdown signals stop the aggregation thread (stop_async -> aggregation_core_stop) and program exits cleanly.

Edge cases and notes
- Parser needs metadata for all aggregation RPCs; use render::ebpf_net::aggregation::all_message_metadata().
- Perfect hash function: for simplicity and to avoid collisions, use capacity 65536 with identity hash over u16 rpc_ids; switch to generator‑provided perfect hash later.
>> See other comment on which hash to use
- Stop semantics: per‑shard AtomicBool keyed by shard; simple and sufficient now. Can integrate with libuv in a later milestone if needed.
- Safety: ElementQueue::new_from_contiguous is unsafe; values from C++ must be correct (they are, since derived from the live queue). Reads are strictly read‑only in aggregation.

Concrete file changes
- crates/reducer/Cargo.toml: add dependencies element-queue, timeslot, render_parser, encoder_ebpf_net_aggregation.
- crates/reducer/src/aggregation_core.rs: new module with AggregationCore implementation.
- crates/reducer/src/ffi.rs: new cxx::bridge with EqView + extern "Rust" fns; implement run/stop wiring into AggregationCore.
- reducer/CMakeLists.txt: add_rust_cxxbridge(reducer_aggregation_cxxbridge crates/reducer/src/ffi.rs) and link reducerlib against it.
- reducer/aggregation/agg_core.h/cc: add run() and stop_async() overrides to call the Rust functions; include generated header.

Acceptance criteria
- reducer builds successfully.
- On startup, aggregation shard threads run Rust AggregationCore.
- Messages are consumed from ElementQueues and logged with queue index, timestamp, rpc_id, and body length.
- reducer shutdown stops aggregation threads cleanly.

