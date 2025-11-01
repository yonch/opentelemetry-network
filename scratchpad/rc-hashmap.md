RcHashMap: Single-threaded, refcounted map with a boxless SlotMap

Overview
- Goal: A HashMap-like structure storing reference-counted key→value pairs. Clients get Rc-like handles (Ref) that provide identity and lifetime management; actual value access happens only through RcHashMap methods to ensure exclusivity rules.
- Design: Store Entry<K,V> by value inside a SlotMap. A separate index HashMap stores small, prehashed keys mapping to SlotMap indices. Ref identifies entries by the SlotMap key. All value access uses RcHashMap::{access, access_mut} so that inserts and other &mut operations cannot run while a user holds a reference to V.
- New decisions: Ref no longer borrows the map; it holds a raw, non-null pointer to an internal Inner. To avoid conflicts with live accesses, we defer physical removals: when an entry’s refcount reaches 0, it is pushed onto a singly linked “dropped slots” list. We clean this list at the start of every &mut self operation (insert/get_or_insert/access_mut/shrink_to_fit) and expose an explicit shrink_to_fit method.
- Constraints: Single-threaded; no atomics; no per-entry heap allocations; no global registries. Ref cloning, hashing, equality, and drop must be O(1).

Prior Art
- The internment crate’s ArcIntern (see /workspaces/internment/src/arc.rs) demonstrates counted headers with pointer-like handles. We adapt that pattern to a per-instance, single-threaded structure with SlotMap-backed storage and no per-entry allocation.

Non-Goals
- Thread-safe operation or cross-thread sharing.
- Weak handles (can be added later).
- Long-lived references to values without holding a read guard.

Data Model
- Entry<K, V>
  - key: K — logical key; Eq + Hash.
  - value: V — stored inline.
  - ref_or_next: Cell<RefOrNext> — either a positive refcount or a next-slot pointer when queued for deletion (see Deferred Deletion).
  - slot_key: slotmap::DefaultKey — index of this Entry in SlotMap.
  - hash: u64 — precomputed hash of `key` using the map’s BuildHasher.
- Containers inside RcHashMap<K, V, S = RandomState>
  - rc_inner: Rc<Inner<K, V, S>> — owning handle of map state. Refs capture a raw pointer to this Inner at creation time; RcHashMap itself does not store a NonNull cache.
- Inner<K, V, S>
  - entries: slotmap::SlotMap<DefaultKey, Entry<K, V>>
  - index: std::collections::HashMap<PreHashedKey, (), S>
  - hasher: S — BuildHasher used for all key hashing.
  - dropped_head: Cell<Option<DefaultKey>> — head of a singly linked list of slots whose refcount reached 0 and are pending physical removal.
  - keepalive: Cell<Option<Rc<Inner<K, V, S>>>> — populated when RcHashMap is being dropped while Refs still exist, allowing Inner to outlive RcHashMap until cleanup completes. (See Keepalive.)
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
  - Clone: increments Entry refcount via interior Cell.
  - Drop: decrements; when zero, pushes slot onto the dropped list by rewriting `ref_or_next` to Next(head) and updating `dropped_head` to this slot. Physical removal is deferred.
  - Does not implement Deref. All value access goes through RcHashMap::{access, access_mut}.
  - Owner pointer acquisition: computed once at Ref creation via `NonNull::new_unchecked(Rc::as_ptr(&rc_inner))`. This is O(1) and does not increment the Rc count. RcHashMap does not cache a NonNull; storing it only in Ref avoids needless duplication.

Hashing and Equality
- Entry<K,V>: Hash/Eq delegate to `key`.
- PreHashedKey: Hash by `hash`, Eq by `slot`.
- Ref: Hash and Eq use `(owner_ptr, slot)` to avoid cross-map collisions, where `owner_ptr` is the `NonNull<Inner>` address.

Public API
- Types
  - pub struct RcHashMap<K, V, S = RandomState>
  - pub struct Ref<K, V, S = RandomState>
- Constructors and basics
  - RcHashMap::new() -> Self
  - RcHashMap::with_hasher(S) -> Self
  - len(&self) -> usize, is_empty(&self) -> bool
  - contains_key<Q: ?Sized>(&self, q: &Q) -> bool where K: Borrow<Q>, Q: Eq + Hash
- Lookup and insertion
  - get<Q: ?Sized>(&self, q: &Q) -> Option<Ref<K, V, S>>
    - Does not clean up dropped entries; treats queued-for-deletion entries as absent.
  - get_or_insert_with(&mut self, key: K, mk_val: impl FnOnce() -> V) -> Ref<K, V, S>
    - Runs cleanup first.
  - insert(&mut self, key: K, value: V) -> Ref<K, V, S>
    - Runs cleanup first.
- Handle utilities (all value access via RcHashMap)
  - Ref::slot_key(&self) -> DefaultKey
  - RcHashMap::access<'a>(&'a self, r: &Ref<K, V, S>) -> Option<&'a V>
    - Returns None if the slot is queued/removed.
  - RcHashMap::access_mut<'a>(&'a mut self, r: &Ref<K, V, S>) -> Option<&'a mut V>
    - Runs cleanup first; returns None if the slot is queued/removed.
  - Ref::refcount(&self) -> usize
- Maintenance
  - shrink_to_fit(&mut self) — cleans dropped slots and shrinks internal storage.
  - clear(&mut self) — debug-asserts all refcounts are zero; drops all storage.

Indexing Strategy (boxless; no key duplication)
- Store only PreHashedKey { hash, slot } in the index; do not duplicate `K`.
- For lookups by Q:
  - Compute `hash(q)` using the map’s hasher.
  - Use `raw_entry().from_hash(hash, |p: &PreHashedKey| entries.get(p.slot).map_or(false, |e| e.is_alive() && e.key.borrow() == q))` to probe. Entries queued for deletion (ref_or_next is Next) are treated as absent.
- For insertions:
  - Begin with cleanup of dropped slots to keep the index accurate (see Deferred Deletion).
  - Same raw lookup; if vacant, create Entry and insert in SlotMap; then insert PreHashedKey { hash, slot } into index.

Operational Semantics
- make_hash(&self, q): computes a u64 via the map’s `S: BuildHasher`.
- Cleanup before mutation:
  - Each &mut self operation (insert/get_or_insert_with/access_mut/shrink_to_fit) starts with `cleanup_dropped()`. It repeatedly pops `dropped_head`, follows the next pointers encoded in `ref_or_next`, removes each slot from `index` (using the stored `hash`) and from `entries`, until the list is empty.
- Insertion path (missing key):
  1) Cleanup dropped slots.
  2) Compute `hash = make_hash(&key)`.
  3) Probe `index.raw_entry().from_hash(hash, eq_by_q)` using `entries[...]` in the equality closure.
  4) If vacant, insert `Entry { key, value: mk_val(), ref_or_next: Cell::new(Count(1)), slot_key: DefaultKey::default(), hash }` into `entries` and get `slot`.
  5) Write `slot` back to the stored Entry’s `slot_key`.
  6) Insert `PreHashedKey { hash, slot }` into `index` with value `()`.
  7) Return Ref { slot, owner: inner }.
- Lookup path (existing key, read-only `get`):
  1) Does not run cleanup.
  2) Compute `hash(q)` and probe index with raw_entry + equality closure that also checks `e.is_alive()`.
  3) If found and alive, increment the `Count` via the entry’s Cell, and return Ref; otherwise return None.
- Value access:
  - access(&self, r): returns `&V` if the entry is alive; otherwise None. Does not perform cleanup.
  - access_mut(&mut self, r): runs cleanup first, then returns `&mut V` if alive; otherwise None.
- Ref drop (deferred deletion):
  1) Read the entry’s `ref_or_next` Cell; if Count(n>1), decrement to Count(n-1) and return.
  2) If Count(1), change it to Next(current_dropped_head), and set `dropped_head` to this slot. Do not touch `index` or `entries` structure.
- shrink_to_fit():
  - Public &mut method to force cleanup and then shrink the internal containers.
- Keepalive (map lifetime while Refs exist):
  - On RcHashMap::drop, if there are outstanding Refs (detectable via `dropped_head` and/or per-entry counts), set `keepalive` to Some(rc_inner.clone()) before dropping; this ensures the map’s storage outlives RcHashMap. When the last Ref across all slots is dropped and cleanup runs, `keepalive` is cleared, allowing Inner to be freed.

Complexity
- get/insert/remove: O(1) average; cleanup is proportional to the number of dropped slots waiting in the list (amortized O(1) when cleanup happens incrementally).
- Ref clone/drop: O(1). Drops never touch the index or the SlotMap — they only update Cells and the dropped list head.
- Hashing Ref: O(1) by hashing `(owner_ptr, slot)`.
  - Memory: one SlotMap element per entry plus a tiny PreHashedKey in the index; no per-entry Box; dropped-but-not-cleaned entries are bounded by user patterns and cleaned on next operation or shrink_to_fit().

Safety and Soundness
- Ref does not implement Deref. All value access is via RcHashMap.
- access(&self, …) returns `&V` and access_mut(&mut self, …) returns `&mut V`, ensuring inserts/get_or_insert (which require &mut self) cannot run while the reference is live.
- Drops do not attempt structural mutation: Ref::drop only updates interior Cells and the dropped list head. Physical removals happen during cleanup in &mut self methods.
- Lookups treat queued (dead) entries as absent, preserving correctness even if cleanup hasn’t run yet.
- Precomputed hashes are derived from the map’s `BuildHasher` and stored in the Entry; physical removal uses the stored hash to remove from the index without touching the key after removal.
- Ref equality/hash include the map identity via the Inner pointer to avoid cross-map aliasing.

Trait Bounds
- K: Eq + Hash + Borrow<Q> for lookups; no 'static bound required.
- V: no bounds.
- S: BuildHasher + Default (RandomState default).

Type Sketch
```rust
use core::{cell::Cell, hash::Hash, borrow::Borrow, ptr::NonNull};
use hashbrown::HashMap; // or std::collections::HashMap
use slotmap::{SlotMap, DefaultKey};
use std::rc::Rc;

#[derive(Copy, Clone)]
enum RefOrNext { Count(usize), Next(Option<DefaultKey>) }

struct Entry<K, V> {
    key: K,
    value: V,
    ref_or_next: Cell<RefOrNext>,
    slot_key: DefaultKey,
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
    keepalive: Cell<Option<Rc<Inner<K, V, S>>>>, // populated on RcHashMap::drop if Refs exist
}

pub struct RcHashMap<K, V, S = RandomState> {
    rc_inner: Rc<Inner<K, V, S>>,
}

pub struct Ref<K, V, S = RandomState> {
    slot: DefaultKey,
    owner: NonNull<Inner<K, V, S>>,
}

impl<K, V, S> RcHashMap<K, V, S> {
    pub fn access<'a>(&'a self, r: &Ref<K, V, S>) -> Option<&'a V> {
        let inner: &Inner<K, V, S> = &self.rc_inner;
        let e = inner.entries.get(r.slot)?;
        if e.is_alive() { Some(&e.value) } else { None }
    }

    pub fn access_mut<'a>(&'a mut self, r: &Ref<K, V, S>) -> Option<&'a mut V> {
        // Cleanup dropped slots before mutation
        self.cleanup_dropped();
        let inner_mut: &mut Inner<K, V, S> = Rc::get_mut(&mut self.rc_inner)?;
        let e = inner_mut.entries.get_mut(r.slot)?;
        if e.is_alive() { Some(&mut e.value) } else { None }
    }

    fn cleanup_dropped(&mut self) {
        let Some(inner_mut) = Rc::get_mut(&mut self.rc_inner) else { return };
        while let Some(head) = inner_mut.dropped_head.get() {
            // Read next pointer
            let next = match inner_mut.entries.get(head).map(|e| e.ref_or_next.get()) {
                Some(RefOrNext::Next(n)) => n,
                _ => None,
            };
            // Remove from index and entries
            if let Some(ent) = inner_mut.entries.get(head) {
                let hash = ent.hash;
                inner_mut.index.raw_entry_mut().from_hash(hash, |p| p.slot == head).remove();
            }
            inner_mut.entries.remove(head);
            inner_mut.dropped_head.set(next);
        }
        // Optionally shrink_to_fit here
    }

    pub fn shrink_to_fit(&mut self) {
        self.cleanup_dropped();
        if let Some(inner_mut) = Rc::get_mut(&mut self.rc_inner) {
            inner_mut.index.shrink_to_fit();
            inner_mut.entries.shrink_to_fit();
        }
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
let len = map.access(&a1).map(|v| v.len()).unwrap();

// Mutate via exclusive access to the map
let mut map = map; // make the binding mutable for demonstration
{
    let vmut: &mut Vec<u8> = map.access_mut(&a2).unwrap();
    vmut.push(4);
} // vmut drops here; map usable again

drop(a1); drop(a2); // last drop removes index + SlotMap entry
assert!(!map.contains_key("alpha"));
```

Testing Plan
- Lifecycle: insert → clone Ref → drop clones → ensure queuing onto dropped list → cleanup removes from index and SlotMap.
- Hash/Eq correctness: Ref equality and hashing include map identity and slot.
- Access safety: `access`/`access_mut` enforce exclusivity via &self/&mut self; verify inserts/get_or_insert cannot run while references are live.
- Liveness checks: `get(&self)` and `access(&self)` treat queued-for-deletion entries as absent.
- Capacity maintenance: `shrink_to_fit()` triggers cleanup and shrinks containers; clear with zero refcounts.
- Stress: repeat insert/get/drop sequences; ensure len == 0 after cleanup and no panics.

Rationale Recap
- Boxless entries avoid per-entry allocations while SlotMap gives stable, cheap keys.
- Prehashed raw-entry indexing avoids duplicating K and avoids self-referential references.
- Decoupling handles from borrowing the map removes the &self lifetime coupling while keeping all value access centralized in RcHashMap via access/access_mut.
- Deferred deletion keeps handle operations O(1) and avoids borrow conflicts; users can call shrink_to_fit() to proactively reclaim memory and run cleanup.
