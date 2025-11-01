RcHashMap proposal review

Scope: Review of `scratchpad/rc-hashmap.md` covering safety, unspecified behavior, and ergonomics. Suggestions include spec clarifications and design tweaks that keep the single-threaded, boxless, O(1) handle goals.

**Key Risks (Non‑Safe Behaviors)**
- `access_mut` relies on `Rc::get_mut`.
  - Any additional `RcHashMap` clones, or a populated `keepalive`, will make `Rc::get_mut` return `None` causing a panic. This turns benign patterns (e.g., accidental clone) into runtime aborts.
  - Current resolution: RcHashMap is not `Clone`, and `keepalive` only exists after the map has been dropped, so mutation paths are not available concurrently. As an alternative for future ergonomics, consider the “Box self‑keepalive” pattern (map holds `Box<Inner>` and, on drop with live refs, stores a raw pointer to free when the last ref is dropped) to avoid `Rc::get_mut` and possibly allow `Clone`.

**Unspecified Behavior (Should Be Specified)**
- Cleanup cadence and memory retention.
  - Make it explicit that physical removals are run:
    - At the start of every `&mut self` op (insert/get_or_insert/access_mut/shrink_to_fit), and
    - Optionally via an explicit `shrink_to_fit`/`cleanup` API.
  - Consider adding a `try_cleanup(&self)` that is a no‑op now (to avoid surprising users), or use `RefCell` internally so `cleanup` can be safe on `&self`.

- Cleanup cadence and memory retention.
  - Make it explicit that physical removals are run:
    - At the start of every `&mut self` op (insert/get_or_insert/access_mut/shrink_to_fit), and
    - Optionally via an explicit `shrink_to_fit`/`cleanup` API.
  - Consider adding a `try_cleanup(&self)` that is a no‑op now (to avoid surprising users), or use `RefCell` internally so `cleanup` can be safe on `&self`.

- What happens to `Ref`s after the last map handle is dropped.
  - `Ref`s remain usable for hashing/eq and for Drop, but they cannot access values without a map handle. State that `Ref` methods that require a map (if any are added later) will panic/return `None` after `Inner` is freed. With keepalive+counter, `Inner` stays alive until the last `Ref` drops.

- Error handling surfaces.
  - The sketch uses `expect("dangling Ref")`. Decide whether dangling is possible in safe usage; if not, prefer `debug_assert!` and make release builds UB‑free and panic‑free. If panics are acceptable, keep panic messages consistent.

**Ergonomics Issues**
- Access requires borrowing the map.
  - `access(&self, &Ref) -> &V` ties the lifetime of `&V` to `&self`. This disallows long‑lived `&V` without pinning a borrow of the map, which is a stated non‑goal but will surprise users compared to `HashMap`/`Rc` ergonomics. Consider offering a read guard type (e.g., `ReadGuard<'a, V>`) if you want ergonomic long reads while still preventing mutation.

- Cleanup only on `&mut` ops can retain memory.
  - Workloads that do mostly reads (`get` + drop `Ref`s) never trigger cleanup and can accumulate queued entries. Add an explicit `cleanup`/`reclaim` method, or document the need to call `shrink_to_fit` periodically.

- Panics from `Rc::get_mut`.
  - Partially addressed by making `RcHashMap` non‑Clone; consider the Box self‑keepalive pattern to avoid `Rc::get_mut` and enable `Clone` without panics.

  

- Limited introspection/iteration.
  - The proposal does not cover iteration over live entries, which is common with maps. Iteration must skip queued entries and either borrow the map for the iterator’s lifetime or provide an internal snapshot. Worth specifying later.

- `Ref` size vs. safety tradeoff.
  - If you switch `Ref` to hold `Rc<Inner>`, handles become larger but simpler and obviously safe. This may be acceptable given single‑threaded constraints and the desire for unambiguous behavior when all map handles drop.

**Concrete Spec/Design Adjustments**
- Map/handle lifecycle (keepalive without a global counter)
  - Maintain `dropped_len` alongside `dropped_head`. On the last `Ref::drop` for a slot, increment `dropped_len`; during cleanup/removal, decrement it. When the map is dropped with live refs, set a strong `keepalive`. When `keepalive.is_some()` and `dropped_len == entries.len()`, clear `keepalive` so `Inner` can drop. No separate `live_ref_count` or `map_dropped` flag required.

- Mutation path without `Rc::get_mut`
  - Prefer `Rc<RefCell<Inner>>` and keep the public API `&mut self` only. Internally, `&mut self` borrows the `RefCell` mutably. This removes the strong‑count footgun while preserving the external exclusivity guarantees.
  - Alternative: adopt a Box self‑keepalive approach to remove `Rc` entirely from the map/inner boundary while preserving O(1) handle ops.

 

**Smaller Observations**
- Generational `DefaultKey` prevents within‑map ABA after removal; cross‑map ABA is prevented by owner checks.
- Storing `slot_key` in `Entry` is redundant; the SlotMap key is already known at insertion time. Consider removing unless needed for debugging.
- `Entry.hash` must be computed with the same `BuildHasher` instance (`Inner.hasher`) used for the `index`. Ensure it’s immutable per `Inner` to keep prehash stable.
- Make `Ref` non‑Copy to preserve refcount invariants.

**Summary**
- The core idea (boxless SlotMap + refcounted handles + deferred deletion) looks solid. Integrated clarifications now cover map identity checks, single‑threaded `Ref`, intended `&V` lifetime, index usage, keepalive teardown, and visible/length semantics. Remaining open items:
  - Consider eliminating `Rc::get_mut` via a Box self‑keepalive pattern to enable `Clone` without panics (or keep non‑Clone as a deliberate constraint).
  - Decide whether to add a user‑callable `cleanup` on `&self` for read‑heavy workloads (likely via interior mutability) or keep cleanup strictly on `&mut`.
  - Define iteration ergonomics (live‑only iteration, borrowing rules, snapshots).
  - Tighten error‑surface policy (panic messages vs `Option` returns) consistently across APIs.
