RcHashMap proposal review

Scope: Review of `scratchpad/rc-hashmap.md` covering safety, unspecified behavior, and ergonomics. Suggestions include spec clarifications and design tweaks that keep the single-threaded, boxless, O(1) handle goals.

**Key Risks (Non‑Safe Behaviors)**
- Owner pointer dangling after map drop.
  - Ref stores `NonNull<Inner>` and does not bump an `Rc`. If all `RcHashMap` owners drop and `Inner` is freed, any later `Ref::drop`/`clone`/`hash`/`eq` that dereferences `owner` is UB.
  - Proposal’s keepalive (`Cell<Option<Rc<Inner>>>`) prevents free, but introduces a ref‑cycle unless explicitly broken. As written, it’s unspecified when/how it’s cleared, so either UB (if omitted) or leak (if kept forever).
  - Fix options:
    - Keep strong keepalive but add a global handle count in `Inner` (e.g., `live_ref_count: Cell<usize>`) and a “map handles” count or flag. On `RcHashMap::drop`, if `live_ref_count > 0`, set `keepalive = Some(Rc::clone(&rc_inner))` and mark `map_dropped = true`. On each `Ref::clone`/`drop`, bump/decr `live_ref_count`. When `map_dropped && live_ref_count == 0`, `take()` and drop `keepalive` to break the cycle. This ensures no UB and no leak.
    - Or, put `Rc<Inner>` inside `Ref` instead of a raw pointer. This is the simplest safe approach (no cycles, no UB). It increases `Ref` size and each clone/drop touches the Rc count, but still O(1). Given single‑threaded, this is cheap and predictable.
    >> Since all slots already keep a count of Refs to that slot, live_ref_count is the sum of those counts. It is non-zero iff any slot has a non-zero count. We can keep the length of the drop list, then the number of non-zero slots is slotmap.len - drop_list_len. So on Ref::drop, if we add to the drop list, we can check if the drop list length == slotmap.len, and if so, we can drop the keepalive. This means we need to maintain the drop list length carefully, but we don't need a separate live_ref_count. And note that map_dropped == true iff keepalive.is_some(), so we don't need a separate map_dropped flag either.

- Missing owner identity check in accessors.
  - `RcHashMap::access`/`access_mut` in the sketch ignore `r.owner` and index into `self.rc_inner.entries` using only `r.slot`. Passing a `Ref` from map A to map B can return the “wrong” `&V` or panic; `Ref::drop` will still mutate map A. This violates intended identity and can mask logic errors.
  - Fix: validate `r.owner == NonNull::from(Rc::as_ref(&self.rc_inner))` in every API that consumes a `Ref`. Decide policy: panic on mismatch (cheaper), or return `Option`/`Result`.
  >> Yes let's ensure we compare and panic on mismatch.

- `access_mut` relies on `Rc::get_mut`.
  - Any additional `RcHashMap` clones, or a populated `keepalive`, will make `Rc::get_mut` return `None` causing a panic. This turns benign patterns (e.g., accidental clone) into runtime aborts.
  - Fix options:
    - Do not implement `Clone` for `RcHashMap` (and document that it is single‑owner), or make `Clone` perform a deep copy to keep strong count at 1 for mutable ops.
    - Replace `Rc<Inner>` with `Rc<RefCell<Inner>>` (or `UnsafeCell` + careful API) so `&mut self` can always access `Inner` without relying on unique `Rc`. You can still keep all mutation behind `&mut self` at the public API boundary.
    - If keepalive stays, ensure it is never present while any `&mut self` op can be called (i.e., no map handle exists), or gate `access_mut` to return `Option<&mut V>` when uniqueness fails.
    >> Is there a pattern where RcHashMap holds a Box to Inner, and if there are live Refs while RcHashMap is dropped, we let Inner hold the Box to itself until the last Ref is dropped? This would avoid Rc::get_mut entirely, and allow RcHashMap to be Clone.

- Auto‑traits for `Ref` may accidentally allow Send/Sync.
  - Single‑threaded is a constraint, but `Ref` contains `NonNull<Inner>` and `DefaultKey` which may end up `Send`/`Sync` via auto traits. `Inner` uses `Cell`/`Rc` (which are !Sync), but do not rely on inference through raw pointers.
  - Fix: explicitly tie `Ref` to single‑threaded by adding `PhantomData<Rc<Inner<K,V,S>>>` or implement `impl !Send for Ref` / `impl !Sync for Ref` to forbid cross‑thread moves.
  >> Yes let's add PhantomData to Ref to ensure !Send + !Sync.

- Reentrancy during cleanup and drops.
  - `cleanup_dropped` removes entries, dropping `K` and `V`. Their destructors could run arbitrary code, including touching this map or dropping other `Ref`s. The singly linked dropped list updates are interior‑mutable and should remain consistent, but it’s easy to miss corner cases.
  - Recommendations:
    - Treat `cleanup_dropped` as reentrant‑safe: read `next` before removal, then unlink, then remove from `entries` and `index`, then continue with the saved `next`. Your sketch already does this in that order; keep it strict.
    >> Agreed.
    - Document that user `Drop` impls for `K`/`V` must not reenter the same map’s `&mut` API while a `&mut` op is active (this is naturally enforced by Rust borrows, but worth stating as a spec rule).
    >> Yes let's document that Drop impls for K/V must not reenter the same map's &mut API while a &mut op is active.

- Use of `PreHashedKey` with `Hash` != `Eq` semantics.
  - The key in `index` hashes by `hash` and compares Eq by `slot`. That’s fine when using `raw_entry` with custom equality, but must never rely on the key type’s `Eq` in standard ops. The removal code shown uses `raw_entry_mut().from_hash(.., |p| p.slot == head)`, which is OK. Specify this invariant and ensure all lookups/removals use raw_entry paths.
  >> Agreed

- Lifetime of `&V` returned from `access` vs refcount transitions.
  - Callers can drop the last `Ref` immediately after calling `access(&self, &r) -> &V`, leaving a live `&V` while the entry is queued for deletion. This is safe as physical removal only happens under `&mut self`, which cannot be taken while `&self` is borrowed for `&V`. Document this as intended to avoid confusion.
  >> Yes let's document that this is intended behavior.

**Unspecified Behavior (Should Be Specified)**
- Keepalive lifecycle and who tears it down.
  - Define exact rules: when the last `RcHashMap` drops and `live_ref_count > 0`, store a strong `keepalive`. When `live_ref_count` hits zero, clear `keepalive` and allow `Inner` to drop. If `live_ref_count == 0` at map drop time, do not set `keepalive`.
  - Specify the counters needed, where they live, and when they change (Ref clone/drop, map clone/drop if allowed).

- Map identity check policy.
  - Decide whether `access`/`access_mut`/other ops panic on mismatched owner vs. return `Option`. Include exact error text and add a `debug_assert_eq!` in debug builds at minimum.

- Semantics of `len()`/`is_empty()`.
  - Do they count only live (non‑queued) entries or also queued ones? Recommend: count only live entries (`ref_or_next` is `Count(_)`). State this explicitly.

- `get(&self)` when an entry is queued for deletion.
  - The doc says “treat queued‑for‑deletion entries as absent.” Specify the exact predicate (i.e., `matches!(ref_or_next, Count(n) if n > 0)`).

- `contains_key` and `get` hash/eq behavior.
  - State that lookup uses the stored prehash to filter, then confirms match by comparing `entries[p.slot].key.borrow() == q` via `raw_entry` predicate. This ensures correctness even with hash collisions and not duplicating `K` in the index.

- Behavior with multiple `RcHashMap` owners.
  - If `RcHashMap` is `Clone`, document whether `access_mut` may panic (current sketch) or will return `None`. If `Clone` is not implemented, say so and explain why.

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
  - As noted, accidental clones or an active keepalive make mutation APIs panic. Either make `RcHashMap` non‑Clone, or change internals to avoid `Rc::get_mut`.

- Cross‑map misuse is not signaled clearly.
  - Without explicit owner checks, bugs become runtime panics or, worse, wrong data. Enforce owner checks with a clear error or `Option` return.

- Limited introspection/iteration.
  - The proposal does not cover iteration over live entries, which is common with maps. Iteration must skip queued entries and either borrow the map for the iterator’s lifetime or provide an internal snapshot. Worth specifying later.

- `Ref` size vs. safety tradeoff.
  - If you switch `Ref` to hold `Rc<Inner>`, handles become larger but simpler and obviously safe. This may be acceptable given single‑threaded constraints and the desire for unambiguous behavior when all map handles drop.

**Concrete Spec/Design Adjustments**
- Map/handle lifecycle
  - Add to `Inner`:
    - `live_ref_count: Cell<usize>` — total number of live `Ref`s across all entries.
    - `map_handles: Cell<usize>` or `map_dropped: Cell<bool>` — counts/flag for map instances.
  - On `RcHashMap::new`, set `map_handles = 1`.
  - If `RcHashMap` is clonable, bump/decr `map_handles` on clone/drop. If it’s non‑Clone, omit and use a boolean flag flipped on drop.
  - On `RcHashMap::drop`:
    - Run a final `cleanup_dropped`.
    - If `live_ref_count > 0`, store a strong `keepalive = Some(Rc::clone(&rc_inner))` and mark `map_dropped = true`.
  - On `Ref::clone`/`drop`, bump/decr `live_ref_count`. On the transition to zero and `map_dropped == true`, `take()` the keepalive to drop `Inner`.

- Owner identity enforcement
  - In all APIs that take a `Ref`, verify match with current map (pointer equality to `Inner`). Decide on panic vs `Option` return and document.

- Mutation path without `Rc::get_mut`
  - Prefer `Rc<RefCell<Inner>>` and keep the public API `&mut self` only. Internally, `&mut self` borrows the `RefCell` mutably. This removes the strong‑count footgun while preserving the external exclusivity guarantees.

- Auto‑trait control for `Ref`
  - Add `PhantomData<Rc<Inner<K,V,S>>>` or explicit negative impls so `Ref` is `!Send + !Sync`. Consider adding `Unpin` if needed.

- Index correctness
  - Require that all index lookups/removals use `raw_entry` with a predicate that compares `entries[p.slot].key` to the input `Q` (or to the target slot for removals). Do not rely on `PreHashedKey`’s `Eq` in normal APIs; it’s a raw‑entry helper only.

- Explicit behavior statements
  - `len`/`is_empty` count only live entries.
  - `get` returns `None` for queued entries.
  - `contains_key` ignores queued entries.
  - `access(_)/access_mut(_)` panic or return `None` on owner mismatch.
  - `shrink_to_fit` first runs cleanup, then shrinks.
  - `Drop` of the last map handle with no `Ref`s drops `Inner` immediately; otherwise `keepalive` retains it until `live_ref_count` hits zero.

**Smaller Observations**
- Generational `DefaultKey` prevents within‑map ABA after removal; cross‑map ABA is prevented by owner checks.
- Storing `slot_key` in `Entry` is redundant; the SlotMap key is already known at insertion time. Consider removing unless needed for debugging.
- `Entry.hash` must be computed with the same `BuildHasher` instance (`Inner.hasher`) used for the `index`. Ensure it’s immutable per `Inner` to keep prehash stable.
- Make `Ref` non‑Copy to preserve refcount invariants.

**Summary**
- The core idea (boxless SlotMap + refcounted handles + deferred deletion) is sound for single‑threaded use, but it needs stronger lifecycle and identity rules to avoid UB and panics:
  - Specify and implement a definitive keepalive strategy (or switch `Ref` to hold `Rc<Inner>`).
  - Enforce owner identity on all handle‑consuming APIs.
  - Remove reliance on `Rc::get_mut` for mutation or forbid `RcHashMap::Clone`.
  - Nail down semantics for queued entries, len/contains/get, and cleanup cadence.
  - Mark `Ref` as `!Send + !Sync`.
These changes keep O(1) handle ops and exclusivity guarantees while making behavior explicit and ergonomic surprises rarer.

