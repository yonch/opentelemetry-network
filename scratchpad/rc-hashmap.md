RcHashMap: Single-threaded, refcounted map with a boxless SlotMap

Overview
- Goal: A HashMap-like structure storing reference-counted key→value pairs. Clients get Rc-like handles (Ref) that provide identity and lifetime management; actual value access happens only through RcHashMap methods to ensure exclusivity rules.
- Design: Store Entry<K,V> by value inside a SlotMap. A separate index HashMap stores small, prehashed keys mapping to SlotMap indices. Ref identifies entries by the SlotMap key. All value access uses RcHashMap::{access, access_mut} so that inserts and other &mut operations cannot run while a user holds a reference to V.
- New decisions: Ref no longer borrows the map; it holds a raw, non-null pointer to an internal Inner. To avoid conflicts with live accesses, we defer physical removals: when an entry’s refcount reaches 0, it is pushed onto a singly linked “dropped slots” list. We track the dropped-list length to know when all entries are dead. We clean this list at the start of every &mut self operation (insert/get_or_insert/access_mut/shrink_to_fit) and expose an explicit shrink_to_fit method. The map owns a `Box<Inner>`; if the map is dropped while Refs exist, `Inner` stores a raw self-keepalive pointer via `Box::into_raw` and is finally destroyed when the last Ref drops.
- Constraints: Single-threaded; no atomics; no per-entry heap allocations; no global registries. Ref cloning, hashing, equality, and drop must be O(1).

Prior Art
- The internment crate’s ArcIntern (see /workspaces/internment/src/arc.rs) demonstrates counted headers with pointer-like handles. We adapt that pattern to a per-instance, single-threaded structure with SlotMap-backed storage and no per-entry allocation.

Non-Goals
- Thread-safe operation or cross-thread sharing.
- Weak handles (can be added later).
- Long-lived references to values without holding a read guard.
- Iteration APIs. Follow-up work will define live-only iteration that ignores queued-for-deletion entries.

Data Model
- Entry<K, V>
  - key: K — logical key; Eq + Hash.
  - value: V — stored inline.
  - ref_or_next: Cell<RefOrNext> — either a positive refcount or a next-slot pointer when queued for deletion (see Deferred Deletion).
  - hash: u64 — precomputed hash of `key` using the map’s BuildHasher.
- Containers inside RcHashMap<K, V, S = RandomState>
  - inner: Box<Inner<K, V, S>> — owning handle of map state. Refs capture a raw pointer to this Inner at creation time; RcHashMap itself does not store a NonNull cache. RcHashMap is not Clone by design: a Clone would yield an empty map because all Refs point to the original map; users should just make a new map to get the same result.
- Inner<K, V, S>
  - entries: slotmap::SlotMap<DefaultKey, Entry<K, V>>
  - index: std::collections::HashMap<PreHashedKey, (), S>
  - hasher: S — BuildHasher used for all key hashing.
  - dropped_head: Cell<Option<DefaultKey>> — head of a singly linked list of slots whose refcount reached 0 and are pending physical removal.
  - dropped_len: Cell<usize> — number of slots currently queued on the dropped list.
  - keepalive_raw: Cell<Option<NonNull<Inner<K, V, S>>>> — populated when RcHashMap is being dropped while Refs still exist, storing a raw pointer obtained via `Box::into_raw`. When `keepalive_raw.is_some() && dropped_len.get() == entries.len()`, the last Ref drop reconstructs `Box::from_raw` and drops it to free `Inner`.
- PreHashedKey
  - hash: u64 — precomputed key hash.
  - slot: DefaultKey — SlotMap key of the Entry.
  - Hash/Eq: Hash uses `hash`; Eq compares `slot`.
- RefOrNext (internal representation)
  - Count(usize) — current Ref count; Count(1) means one live handle.
  - Next(Option<DefaultKey>) — queued for deletion; value stores the next slot in the dropped list (None marks end).

Handles
- Ref<K, V, S>
  - slot: DefaultKey — identifies the Entry in the SlotMap.
  - owner: NonNull<Inner<K, V, S>> — raw pointer to map state for O(1) handle ops without borrowing the map or bumping an Rc.
  - marker: PhantomData<Cell<()>> — ties `Ref` to single-threaded semantics and prevents `Send`/`Sync` auto-traits.
  - Clone: increments Entry refcount via interior Cell.
  - Drop: decrements; when zero, pushes slot onto the dropped list by rewriting `ref_or_next` to Next(head) and updating `dropped_head` to this slot; increments `dropped_len`. If `keepalive_raw.is_some()` and `dropped_len == entries.len()`, reconstructs a `Box` with `Box::from_raw` from `keepalive_raw` and drops it to free `Inner`. Physical removal is deferred.
  - Does not implement Deref. All value access goes through RcHashMap::{access, access_mut}.
  - Does not implement Copy. Cloning a Ref increments the entry’s refcount.
  - Owner pointer acquisition: computed once at Ref creation via `NonNull::from(self.inner.as_ref())`.

Hashing and Equality
- Entry<K,V>: Hash/Eq delegate to `key`.
- PreHashedKey: Hash by `hash`, Eq by `slot`.
- Ref: Hash and Eq use `(owner_ptr, slot)` to avoid cross-map collisions, where `owner_ptr` is the `NonNull<Inner>` address.
 - Index operations:
   - Lookup by `Q` uses `raw_entry` with the stored prehash and a predicate that confirms key equality and liveness.
   - Removal by slot uses `remove(&PreHashedKey { hash, slot })`, since the exact key is known.

Public API
- Types
  - pub struct RcHashMap<K, V, S = RandomState>
  - pub struct Ref<K, V, S = RandomState>
- Constructors and basics
  - RcHashMap::new() -> Self
  - RcHashMap::with_hasher(S) -> Self
  - len(&self) -> usize, is_empty(&self) -> bool
    - Count only live (non-queued) entries; queued-for-deletion entries are excluded.
  - contains_key<Q: ?Sized>(&self, q: &Q) -> bool where K: Borrow<Q>, Q: Eq + Hash
    - Checks only live entries; queued-for-deletion entries are ignored.
- Lookup and insertion
  - get<Q: ?Sized>(&self, q: &Q) -> Option<Ref<K, V, S>>
    - Does not clean up dropped entries; treats queued-for-deletion entries as absent.
  - get_or_insert_with(&mut self, key: K, mk_val: impl FnOnce() -> V) -> Ref<K, V, S>
    - Runs cleanup first.
  - insert(&mut self, key: K, value: V) -> Ref<K, V, S>
    - Runs cleanup first.
- Handle utilities (all value access via RcHashMap)
  - RcHashMap::access<'a>(&'a self, r: &Ref<K, V, S>) -> &'a V
    - Guaranteed alive while `Ref` exists; no Option needed. Panics if `r` does not belong to this map.
  - RcHashMap::access_mut<'a>(&'a mut self, r: &Ref<K, V, S>) -> &'a mut V
    - Runs cleanup first; guaranteed alive while `Ref` exists; no Option needed. Panics if `r` does not belong to this map.
  - Ref::refcount(&self) -> usize
- Maintenance
  - cleanup(&mut self) — runs deferred deletions without shrinking; called explicitly by users in read-heavy workloads.
  - shrink_to_fit(&mut self) — first calls cleanup(), then shrinks internal storage.
  - clear(&mut self) — debug-asserts all refcounts are zero; drops all storage.

Indexing Strategy (boxless; no key duplication)
- Store only PreHashedKey { hash, slot } in the index; do not duplicate `K`.
- Double-hash convention (accepted for simplicity):
  - Inner hash h1 = make_hash(K) using the map’s BuildHasher (stored in Entry.hash).
  - Outer hash h2 is computed by hashing a temporary `PreHashedKey { hash: h1, slot: any }` with the same BuildHasher (slot is ignored by PreHashedKey::Hash). This mirrors exactly what normal HashMap hashing would do.
  - Insert and lookups must both use h2 in raw_entry().from_hash to be consistent.
- For lookups by Q:
  - Compute `h1 = make_hash(q)`.
  - Compute `h2 = make_outer(h1)` (build a hasher, write_u64(h1), finish()).
  - Use `raw_entry().from_hash(h2, |p: &PreHashedKey| entries.get(p.slot).map_or(false, |e| e.is_alive() && e.key.borrow() == q))` to probe. Entries queued for deletion (ref_or_next is Next) are treated as absent.
- For insertions:
  - Begin with cleanup of dropped slots to keep the index accurate (see Deferred Deletion).
  - Compute `h1 = make_hash(&key)` and `h2 = make_outer(h1)`.
  - Probe with `raw_entry_mut().from_hash(h2, …)`; if vacant, create Entry and insert in SlotMap; then insert PreHashedKey { hash: h1, slot } into index using ordinary insert.

- Operational Semantics
- make_hash(&self, q) -> u64 (h1): computes the inner hash via the map’s `S: BuildHasher`.
- make_outer(&self, h1: u64) -> u64 (h2): builds a fresh `S::Hasher`, hashes `PreHashedKey { hash: h1, slot: any }` into it (slot is ignored by Hash), then returns finish().
- Cleanup before mutation:
  - Each &mut self operation (insert/get_or_insert_with/access_mut/cleanup/shrink_to_fit) starts with `cleanup_dropped()`. It repeatedly pops `dropped_head`, follows the next pointers encoded in `ref_or_next`, removes each slot from `index` via `index.remove(&PreHashedKey { hash, slot })` and from `entries`, until the list is empty.
- Insertion path (missing key):
  1) Cleanup dropped slots.
  2) Compute `h1 = make_hash(&key)` and `h2 = make_outer(h1)`.
  3) Probe `index.raw_entry().from_hash(h2, eq_by_q)` using `entries[...]` in the equality closure.
  4) If vacant, insert `Entry { key, value: mk_val(), ref_or_next: Cell::new(Count(1)), hash }` into `entries` and get `slot`.
  5) Insert `PreHashedKey { hash, slot }` into `index` with value `()`.
  6) Return Ref { slot, owner: inner }.
- Lookup path (existing key, read-only `get`):
  1) Does not run cleanup.
  2) Compute `h1 = make_hash(q)`, `h2 = make_outer(h1)`, and probe index with `raw_entry().from_hash(h2, …)` and equality closure that also checks `e.is_alive()`.
  3) If found and alive, increment the `Count` via the entry’s Cell, and return Ref; otherwise return None.
- Value access:
  - access(&self, r): returns `&V`. By invariant, if a `Ref` exists, the entry is alive and not queued for deletion. Does not perform cleanup. Panics if `r` does not belong to this map.
  - access_mut(&mut self, r): runs cleanup first, then returns `&mut V`. The invariant holds because `Ref` existence implies the entry is alive. Panics if `r` does not belong to this map.
- Ref drop (deferred deletion):
  1) Read the entry’s `ref_or_next` Cell; if Count(n>1), decrement to Count(n-1) and return.
  2) If Count(1), change it to Next(current_dropped_head), set `dropped_head` to this slot, and increment `dropped_len`. Do not touch `index` or `entries` structure.
- shrink_to_fit():
  - Public &mut method to force cleanup and then shrink the internal containers.
- Keepalive (map lifetime while Refs exist):
  - On RcHashMap::drop, if there are outstanding Refs (detectable via per-entry counts vs. `dropped_len`), move `inner` into a raw pointer with `Box::into_raw` and store it as `keepalive_raw`. When `keepalive_raw.is_some()` and `dropped_len == entries.len()`, the last `Ref` drop reconstructs a `Box` via `unsafe { Box::from_raw(ptr) }` and drops it, freeing `Inner`.

Complexity
- get/insert/remove: O(1) average; cleanup is proportional to the number of dropped slots waiting in the list (amortized O(1) when cleanup happens incrementally).
- Ref clone/drop: O(1). Drops never touch the index or the SlotMap — they only update Cells and the dropped list head.
- Hashing Ref: O(1) by hashing `(owner_ptr, slot)`.
  - Memory: one SlotMap element per entry plus a tiny PreHashedKey in the index; no per-entry Box; dropped-but-not-cleaned entries are bounded by user patterns and cleaned on next operation or shrink_to_fit().

Safety and Soundness
- Ref does not implement Deref. All value access is via RcHashMap.
- access(&self, …) returns `&V` and access_mut(&mut self, …) returns `&mut V` (no Option). Invariant: if a Ref exists for a slot, that slot’s ref_or_next is Count(n>0), so the entry is alive and present in entries; only the last Ref::drop transitions to Next and there are no Refs left afterward.
- Inserts/get_or_insert require &mut self, so they cannot run while a `&V` or `&mut V` from access/access_mut is live.
- Drops do not attempt structural mutation: Ref::drop only updates interior Cells and the dropped list head. Physical removals happen during cleanup in &mut self methods.
- Lookups treat queued (dead) entries as absent, preserving correctness even if cleanup hasn’t run yet.
- Precomputed hashes are derived from the map’s `BuildHasher` and stored in the Entry; physical removal uses the stored hash to remove from the index without touching the key after removal.
- Ref equality/hash include the map identity via the Inner pointer to avoid cross-map aliasing.
 - Owner identity is checked on every API that consumes a Ref; a mismatch panics with a clear message.
 - The lifetime of `&V` from `access` remains valid even if the last `Ref` to that entry is dropped immediately afterward; physical removal is deferred to `&mut self` operations, which cannot run while `&self` is borrowed.
- Reentrancy-safe cleanup: `cleanup_dropped` reads the next pointer before removal, unlinks the head, removes from `index` and `entries`, updates `dropped_len`, then proceeds. Drop impls of `K`/`V` must not reenter this map’s `&mut` API while a `&mut` op is active.
- Auto-traits: `Ref` is `!Send + !Sync` via a `PhantomData<Cell<()>>` marker, enforcing single-threaded use.
 - Accessors use `debug_assert!` and `unwrap_unchecked()` when fetching entries by slot, based on the invariant that a `Ref` always points to a live entry.

Trait Bounds
- K: Eq + Hash + Borrow<Q> for lookups; no 'static bound required.
- V: no bounds.
- S: BuildHasher + Default (RandomState default).

Type Sketch
```rust
use core::{cell::Cell, hash::Hash, borrow::Borrow, ptr::NonNull, marker::PhantomData};
use hashbrown::HashMap; // or std::collections::HashMap
use slotmap::{SlotMap, DefaultKey};
use std::hash::Hasher; // for write_u64/finish in outer-hash examples

#[derive(Copy, Clone)]
enum RefOrNext { Count(usize), Next(Option<DefaultKey>) }

struct Entry<K, V> {
    key: K,
    value: V,
    ref_or_next: Cell<RefOrNext>,
    hash: u64,
}

impl<K, V> Entry<K, V> { fn is_alive(&self) -> bool { matches!(self.ref_or_next.get(), RefOrNext::Count(_)) } }

#[derive(Copy, Clone)]
struct PreHashedKey { hash: u64, slot: DefaultKey }
impl core::hash::Hash for PreHashedKey { fn hash<H: core::hash::Hasher>(&self, h: &mut H) { h.write_u64(self.hash) } }
impl PartialEq for PreHashedKey { fn eq(&self, other: &Self) -> bool { self.slot == other.slot } }
impl Eq for PreHashedKey {}

struct Inner<K, V, S> {
    entries: SlotMap<DefaultKey, Entry<K, V>>,
    index: HashMap<PreHashedKey, (), S>,
    hasher: S,
    dropped_head: Cell<Option<DefaultKey>>,
    dropped_len: Cell<usize>,
    keepalive_raw: Cell<Option<NonNull<Inner<K, V, S>>>>, // populated on RcHashMap::drop if Refs exist; when dropped_len == entries.len(), Box::from_raw frees Inner
}

pub struct RcHashMap<K, V, S = RandomState> {
    inner: Box<Inner<K, V, S>>,
}

pub struct Ref<K, V, S = RandomState> {
    slot: DefaultKey,
    owner: NonNull<Inner<K, V, S>>,
    marker: PhantomData<Cell<()>>,
}

impl<K, V, S> RcHashMap<K, V, S> {
    pub fn access<'a>(&'a self, r: &Ref<K, V, S>) -> &'a V {
        // Enforce owner identity
        let owner_ptr = NonNull::from(self.inner.as_ref());
        assert!(core::ptr::eq(owner_ptr.as_ptr(), r.owner.as_ptr()), "Ref belongs to a different RcHashMap");
        let inner: &Inner<K, V, S> = &self.inner;
        let e_opt = inner.entries.get(r.slot);
        debug_assert!(e_opt.is_some(), "dangling Ref");
        // Safety: by invariant, a Ref only exists for live entries; slot exists
        let e = unsafe { e_opt.unwrap_unchecked() };
        &e.value
    }

    pub fn access_mut<'a>(&'a mut self, r: &Ref<K, V, S>) -> &'a mut V {
        // Cleanup dropped slots before mutation
        self.cleanup_dropped();
        // Enforce owner identity
        let owner_ptr = NonNull::from(self.inner.as_ref());
        assert!(core::ptr::eq(owner_ptr.as_ptr(), r.owner.as_ptr()), "Ref belongs to a different RcHashMap");
        let inner_mut: &mut Inner<K, V, S> = &mut self.inner;
        let e_opt = inner_mut.entries.get_mut(r.slot);
        debug_assert!(e_opt.is_some(), "dangling Ref");
        // Safety: by invariant, a Ref only exists for live entries; slot exists
        let e = unsafe { e_opt.unwrap_unchecked() };
        &mut e.value
    }

    fn cleanup_dropped(&mut self) {
        let inner_mut: &mut Inner<K, V, S> = &mut self.inner;
        while let Some(head) = inner_mut.dropped_head.get() {
            // Read next pointer
            let next = match inner_mut.entries.get(head).map(|e| e.ref_or_next.get()) {
                Some(RefOrNext::Next(n)) => n,
                _ => None,
            };
            // Remove from index and entries
            if let Some(ent) = inner_mut.entries.get(head) {
                let h1 = ent.hash;
                let key = PreHashedKey { hash: h1, slot: head };
                inner_mut.index.remove(&key);
            }
            inner_mut.entries.remove(head);
            inner_mut.dropped_head.set(next);
            inner_mut.dropped_len.set(inner_mut.dropped_len.get().saturating_sub(1));
        }
        // Optionally shrink_to_fit here
    }

    pub fn cleanup(&mut self) {
        self.cleanup_dropped();
    }

    pub fn shrink_to_fit(&mut self) {
        self.cleanup();
        let inner_mut: &mut Inner<K, V, S> = &mut self.inner;
        inner_mut.index.shrink_to_fit();
        inner_mut.entries.shrink_to_fit();
    }
}
```

Example Usage
```rust
let map: RcHashMap<String, Vec<u8>> = RcHashMap::new();

let a1 = map.get_or_insert_with("alpha".to_string(), || vec![1,2,3]);
let a2 = map.get("alpha").unwrap();

// Hash Ref cheaply by (map id, slot)
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
let mut h1 = DefaultHasher::new(); a1.hash(&mut h1);
let mut h2 = DefaultHasher::new(); a2.hash(&mut h2);
assert_eq!(h1.finish(), h2.finish());

// Read via RcHashMap::access
let len = map.access(&a1).len();

// Mutate via exclusive access to the map
let mut map = map; // make the binding mutable for demonstration
{
    let vmut: &mut Vec<u8> = map.access_mut(&a2);
    vmut.push(4);
} // vmut drops here; map usable again

drop(a1); drop(a2); // last drop removes index + SlotMap entry
assert!(!map.contains_key("alpha"));
```

Testing Plan
- Lifecycle: insert → clone Ref → drop clones → ensure queuing onto dropped list → cleanup removes from index and SlotMap.
- Hash/Eq correctness: Ref equality and hashing include map identity and slot.
- Access safety: `access`/`access_mut` enforce exclusivity via &self/&mut self; verify inserts/get_or_insert cannot run while references are live.
 - Identity safety: APIs that take a Ref verify map identity and panic on mismatch.
- Liveness invariant: while any Ref exists, access/access_mut always return references to live entries; get(&self) treats queued-for-deletion entries as absent.
- Capacity maintenance: `shrink_to_fit()` triggers cleanup and shrinks containers; clear with zero refcounts.
- Stress: repeat insert/get/drop sequences; ensure len == 0 after cleanup and no panics.

Rationale Recap
- Boxless entries avoid per-entry allocations while SlotMap gives stable, cheap keys.
- Prehashed raw-entry indexing avoids duplicating K and avoids self-referential references.
- Decoupling handles from borrowing the map removes the &self lifetime coupling while keeping all value access centralized in RcHashMap via access/access_mut.
- Deferred deletion keeps handle operations O(1) and avoids borrow conflicts; users can call shrink_to_fit() to proactively reclaim memory and run cleanup.
