RcHashMap: Single-threaded, refcounted map with a boxless SlotMap

Overview
- Goal: A HashMap-like structure storing reference-counted key→value pairs. Clients get Rc-like handles (Ref) that provide read access and automatically remove the entry when the last handle is dropped.
- Design: Store Entry<K,V> by value inside a SlotMap. A separate index HashMap stores small, prehashed keys mapping to SlotMap indices. Ref identifies entries by the SlotMap key and exposes read access via short-lived guards to keep the design sound without per-entry Box.
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
  - refcount: Cell<usize> — single-threaded reference count.
  - slot_key: slotmap::DefaultKey — index of this Entry in SlotMap.
  - hash: u64 — precomputed hash of `key` using the map’s BuildHasher.
- Containers inside RcHashMap<K, V, S = RandomState>
  - entries: RefCell<slotmap::SlotMap<DefaultKey, Entry<K, V>>>
  - index: RefCell<std::collections::HashMap<PreHashedKey, (), S>>
  - hasher: S — BuildHasher used for all key hashing.
- PreHashedKey
  - hash: u64 — precomputed key hash.
  - slot: DefaultKey — SlotMap key of the Entry.
  - Hash/Eq: Hash uses `hash`; Eq compares `slot`.

Handles and Read Guards
- Ref<'map, K, V, S>
  - slot: DefaultKey — identifies the Entry in the SlotMap.
  - owner: &'map RcHashMap<K, V, S> — backpointer for refcount updates and storage access.
  - Clone: increments Entry.refcount.
  - Drop: decrements; when zero, removes from index and SlotMap (reclaiming all resources).
  - Does not implement Deref; instead offers read guards and explicit mutable access via the map.
- RefGuard<'a, K, V>
  - Holds an immutable borrow of the SlotMap plus the slot key.
  - Implements Deref<Target = V> and provides key() -> &K.
  - Lifetime is tied to the borrow, preventing mutation while `&V` is live.

Hashing and Equality
- Entry<K,V>: Hash/Eq delegate to `key`.
- PreHashedKey: Hash by `hash`, Eq by `slot`.
- Ref: Hash and Eq use `(owner_id, slot)` to avoid cross-map collisions.

Public API
- Types
  - pub struct RcHashMap<K, V, S = RandomState>
  - pub struct Ref<'map, K, V, S = RandomState>
  - pub struct RefGuard<'a, K, V>
- Constructors and basics
  - RcHashMap::new() -> Self
  - RcHashMap::with_hasher(S) -> Self
  - len(&self) -> usize, is_empty(&self) -> bool
  - contains_key<Q: ?Sized>(&self, q: &Q) -> bool where K: Borrow<Q>, Q: Eq + Hash
- Lookup and insertion
  - get<Q: ?Sized>(&self, q: &Q) -> Option<Ref<'_, K, V, S>>
  - get_or_insert_with(&self, key: K, mk_val: impl FnOnce() -> V) -> Ref<'_, K, V, S>
  - insert(&self, key: K, value: V) -> Ref<'_, K, V, S>
- Handle utilities
  - Ref::slot_key(&self) -> DefaultKey
  - Ref::with_ref<R>(&self, f: impl FnOnce(RefGuard<'_, K, V>) -> R) -> R
  - RcHashMap::access<'a>(&'a mut self, r: &Ref<'a, K, V, S>) -> &'a mut V
    - Provides exclusive mutable access to V tied to a unique borrow of the map. This prevents any concurrent map mutation while `&mut V` is alive.
  - Ref::refcount(&self) -> usize
- Maintenance
  - shrink_to_fit(&self)
  - clear(&self) — debug-asserts all refcounts are zero; drops all storage.

Indexing Strategy (boxless; no key duplication)
- Store only PreHashedKey { hash, slot } in the index; do not duplicate `K`.
- For lookups by Q:
  - Compute `hash(q)` using the map’s hasher.
  - Use `raw_entry().from_hash(hash, |p: &PreHashedKey| entries.get(p.slot).map_or(false, |e| e.key.borrow() == q))` to probe.
- For insertions:
  - Same raw lookup; if vacant, create Entry and insert in SlotMap; then insert PreHashedKey { hash, slot } into index.

Operational Semantics
- make_hash(&self, q): computes a u64 via the map’s `S: BuildHasher`.
- Insertion path (missing key):
  1) Compute `hash = make_hash(&key)`.
  2) Borrow `entries` immutably; probe `index.raw_entry().from_hash(hash, eq_by_q)`; if found, drop borrow, increment refcount and return handle.
  3) If vacant: drop immutable borrow; borrow `entries` and `index` mutably.
  4) Insert `Entry { key, value: mk_val(), refcount: Cell::new(1), slot_key: DefaultKey::default(), hash }` into `entries` and get `slot`.
  5) Write `slot` back to the stored Entry’s `slot_key`.
  6) Insert `PreHashedKey { hash, slot }` into `index` with value `()`.
  7) Return RcEntry { slot, owner }.
- Lookup path (existing key):
  1) Compute `hash(q)`.
  2) Borrow `entries` immutably; probe index with raw_entry + equality closure reading `entries[p.slot].key`.
  3) If found, get `slot`, drop immutable borrow, increment `refcount` via a mutable borrow of `entries`, and return Ref.
- Ref read access:
  - Ref::with_ref borrows `entries` immutably, builds RefGuard { borrow, slot }, and applies the user function.
- Ref drop:
  1) Borrow `entries` mutably; fetch Entry at `slot`; decrement `refcount`.
  2) If nonzero, return. Otherwise, copy `hash = entry.hash` and remove via `entries.remove(slot)`.
  3) Borrow `index` mutably; remove with `raw_entry_mut().from_hash(hash, |p| p.slot == slot).remove()`.

Complexity
- get/insert/remove: O(1) average.
- Ref clone/drop: O(1).
- Hashing Ref: O(1) by hashing `(owner_id, slot)`.
- Memory: one SlotMap element per entry plus a tiny PreHashedKey in the index; no per-entry Box.

Safety and Soundness
- No per-entry pointers are stored; Ref does not implement Deref. All `&V` references are produced via RefGuard, which holds an active immutable borrow of the SlotMap preventing mutation/reallocation while `&V` is live.
- Single-threaded: RefCell enforces exclusive mutable borrows for insert/remove/drop and shared immutable borrows for reads.
- Precomputed hashes are derived from the map’s `BuildHasher` and stored in the Entry; removal uses the stored hash to query the index without reading the key after removal.
- Ref equality/hash include the map identity to avoid cross-map aliasing.

Trait Bounds
- K: Eq + Hash + Borrow<Q> for lookups; no 'static bound required.
- V: no bounds.
- S: BuildHasher + Default (RandomState default).

Type Sketch
```rust
use core::{cell::Cell, hash::Hash, borrow::Borrow};
use hashbrown::HashMap; // or std::collections::HashMap
use slotmap::{SlotMap, DefaultKey};
use std::cell::{Ref, RefCell};

struct Entry<K, V> {
    key: K,
    value: V,
    refcount: Cell<usize>,
    slot_key: DefaultKey,
    hash: u64,
}

#[derive(Copy, Clone)]
struct PreHashedKey {
    hash: u64,
    slot: DefaultKey,
}

impl core::hash::Hash for PreHashedKey {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) { h.write_u64(self.hash) }
}
impl PartialEq for PreHashedKey { fn eq(&self, other: &Self) -> bool { self.slot == other.slot } }
impl Eq for PreHashedKey {}

struct Inner<K, V, S> {
    entries: RefCell<SlotMap<DefaultKey, Entry<K, V>>>,
    index: RefCell<HashMap<PreHashedKey, (), S>>,
    hasher: S,
}

pub struct RcHashMap<K, V, S = RandomState> { inner: Inner<K, V, S> }

pub struct Ref<'map, K, V, S = RandomState> {
    slot: DefaultKey,
    owner: &'map RcHashMap<K, V, S>,
}

pub struct RefGuard<'a, K, V> {
    entries: Ref<'a, SlotMap<DefaultKey, Entry<K, V>>>,
    slot: DefaultKey,
}

impl<'a, K, V> RefGuard<'a, K, V> {
    pub fn key(&self) -> &K { &self.entries[self.slot].key }
}
impl<'a, K, V> core::ops::Deref for RefGuard<'a, K, V> {
    type Target = V;
    fn deref(&self) -> &V { &self.entries[self.slot].value }
}

impl<K, V, S> RcHashMap<K, V, S> {
    // Exclusive mutable access to the value behind a Ref.
    pub fn access<'a>(&'a mut self, r: &Ref<'a, K, V, S>) -> &'a mut V {
        let entries: &mut SlotMap<DefaultKey, Entry<K, V>> = self.inner.entries.get_mut();
        &mut entries.get_mut(r.slot).expect("dangling Ref").value
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

// Read via guard
let v = a1.with_ref(|g| g.clone()); // requires V: Clone

// Mutate via exclusive access to the map
let mut map = map; // make the binding mutable for demonstration
{
    let vmut: &mut Vec<u8> = map.access(&a2);
    vmut.push(4);
} // vmut drops here; map usable again

drop(a1); drop(a2); // last drop removes index + SlotMap entry
assert!(!map.contains_key("alpha"));
```

Testing Plan
- Lifecycle: insert → clone Ref → drop clones → ensure removal from index and SlotMap.
- Hash/Eq correctness: Ref equality and hashing include map identity and slot.
- Guard safety: no RefCell borrow conflicts; reads use RefGuard; mutation prohibited while guards live.
- access correctness: `access` returns `&mut V` only with `&mut self`; cannot coexist with guards; compile-time and RefCell runtime checks enforce exclusivity.
- Capacity maintenance: shrink_to_fit correctness; clear with zero refcounts.
- Stress: repeat insert/get/drop sequences; ensure len == 0 and no panics.

Rationale Recap
- Boxless entries avoid per-entry allocations while SlotMap gives stable, cheap keys.
- Prehashed raw-entry indexing avoids duplicating K and avoids self-referential references.
- Read guards preserve safety when returning `&V` in a design where elements may move on SlotMap reallocation.
