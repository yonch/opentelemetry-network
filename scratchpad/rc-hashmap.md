RcHashMap: Single‑threaded, refcounted hash‑interning with slotmap values

Overview
- Goal: A HashMap-like structure storing reference-counted key→value pairs. Clients get Rc-like handles (RcEntry) that deref to the value and automatically remove the map entry when the last handle is dropped.
- Key idea: Store an Entry per logical key. Entry hashes and compares using the key. Values live in a SlotMap; each Entry stores the slotmap key. RcEntry hashes by that slot key for O(1) hashing.
- Constraints: Single-threaded; no atomics; no global registries; minimal runtime overhead. RcEntry must be cheap to clone/hash/compare; removal on last-drop must reclaim the HashMap slot and the SlotMap value.

Prior Art
- internment/ArcIntern (thread-safe, global containers) shows the shape of the pattern: a counted header (RefCount) and a handle (ArcIntern) pointing to it. See internment/src/arc.rs. We adapt the core ideas but:
  - Use single-threaded counting (Cell/usize) not atomics.
  - Use a per-instance container (no global DashMap); our RcEntry has a lifetime tied to its owning map.
  - Values are placed in a SlotMap and identified by the SlotMap key; RcEntry hashes by that key.

Non‑Goals
- No thread-safety or cross-thread sharing.
- No weak handles initially (can be added later).
- No concurrent map operations; interior mutability is via RefCell.

Data Model
- Entry<K>: Metadata for a key’s value.
  - key: K — logical key; implements Eq+Hash.
  - refcount: Cell<usize> — single-threaded reference count.
  - slot_key: slotmap::DefaultKey — index into the SlotMap storing the value.
  - val_ptr: NonNull<V> — raw pointer to the heap allocation of V (stored as Box<V> inside SlotMap) for fast deref without borrowing the map.
  - Hash/Eq: delegate to `key` so Entry is hashable and equatable by K.

- Containers inside RcHashMap<K, V, S = RandomState>:
  - map: HashMap<Box<Entry<K>>, (), S>
    - We store Box<Entry<K>> as the key (value is `()`), mirroring internment’s BoxRefCount<T> key shape. The Box makes the Entry’s address stable; HashMap rehashing does not move the pointee.
    - Implement Borrow<K> for Box<Entry<K>> so we can do keyed lookups with `&K` and removals with an `&Entry<K>`.
  - values: slotmap::SlotMap<DefaultKey, Box<V>>
    - Values are stored as Box<V> so the address of V is stable even if SlotMap reallocates its internal storage.
  - hasher: S — BuildHasher for the outer HashMap.

- RcEntry<'map, K, V>: The handle given to users.
  - entry: NonNull<Entry<K>> — stable pointer into the boxed Entry owned by the map.
  - owner: &'map RefCell<Inner<K, V, S>> — backpointer to the map’s inner state for drop-time cleanup. The lifetime ties RcEntry to RcHashMap; RcEntry cannot outlive the map.
  - Traits:
    - Clone: increments Entry.refcount.
    - Drop: decrements; when zero, removes from SlotMap and HashMap and drops the Entry.
    - Deref<Target = V>: returns `&V` via Entry.val_ptr.
    - Eq/Hash: compare/hash by Entry.slot_key; cheap and pointer-agnostic.

Interior Mutability and Lifetimes
- RcHashMap stores `inner: RefCell<Inner<K, V, S>>` to allow RcEntry::drop to mutate the containers without requiring &mut RcHashMap.
- RcEntry holds `&'map RefCell<Inner<…>>` so the map must outlive all RcEntry instances; no `Weak` is necessary.
- Entry lives on the heap (Box) so its address is stable and safe to retain in RcEntry as NonNull.
- Value pointers are stable because values are boxed before insertion into SlotMap.
- No aliasing UB: refcount is in a Cell; mutations happen via shared references, which is allowed for Cell.

Hashing and Equality
- Entry<K>:
  - impl Hash for Entry<K> { key.hash() }
  - impl PartialEq/Eq for Entry<K> { key.eq(&other.key) }
  - impl Borrow<K> for Entry<K>, and Borrow<K> for Box<Entry<K>> -> &entry.key
- RcEntry<'_, K, V>:
  - impl Hash { slot_key.hash() }
  - impl PartialEq/Eq { self.slot_key == other.slot_key }

Public API (proposed)
- Types
  - pub struct RcHashMap<K, V, S = RandomState>
  - pub struct RcEntry<'map, K, V>

- Constructors and basics
  - RcHashMap::new() -> Self
  - RcHashMap::with_hasher(S) -> Self
  - len(&self) -> usize, is_empty(&self) -> bool
  - contains_key<Q: ?Sized>(&self, q: &Q) -> bool
    where K: Borrow<Q>, Q: Eq + Hash

- Lookup and insertion
  - get<Q: ?Sized>(&self, q: &Q) -> Option<RcEntry<'_, K, V>>
    - If present, increments refcount and returns RcEntry.
  - get_or_insert_with(&self, key: K, mk_val: impl FnOnce() -> V) -> RcEntry<'_, K, V>
    - Fast path if key exists; otherwise creates new Entry and SlotMap value.
  - insert(&self, key: K, value: V) -> RcEntry<'_, K, V>
    - Equivalent to `get_or_insert_with(key, || value)`.

- Entry/handle utilities
  - RcEntry::key(&self) -> &K
  - RcEntry::slot_key(&self) -> slotmap::DefaultKey
  - impl Deref<Target = V>
  - impl Debug/Display bounds mirroring V

- Maintenance
  - shrink_to_fit(&self)
    - Tries to shrink the HashMap and SlotMap capacities; only safe when no outstanding borrows; fine because we don’t expose &mut to inner containers.
  - clear(&self)
    - Option A (strict): debug_assert that all refcounts are zero; otherwise panic in debug builds; release all storage.
    - Option B (lenient): drop entries with zero refcount and return number of retained entries. Recommend Option A for clarity and safety in v1.

Operational Semantics
- Insertion path (missing key):
  1) Box the value: `let mut vb = Box::new(value);`
  2) Take a stable pointer before moving the box: `let val_ptr = NonNull::from(vb.as_ref());`
  3) Insert the box into SlotMap, transferring ownership: `let slot_key = values.insert(vb);`
  3) Create `entry = Box::new(Entry { key, refcount: Cell::new(1), slot_key, val_ptr })`.
  4) Insert `entry` into HashMap as key with value `()`.
  5) Return RcEntry { entry: NonNull::from(&*entry), owner: &self.inner }.

- Lookup path (existing key):
  1) Find `&Box<Entry<K>>` by borrowed key via `map.get(&q)`.
  2) Convert to raw pointer `NonNull<Entry<K>>`.
  3) Increment `entry.refcount`.
  4) Return RcEntry pointing at this entry.

- Drop path (RcEntry::drop):
  1) Decrement `entry.refcount`.
  2) If it becomes zero:
     - Remove value from SlotMap via `values.remove(entry.slot_key)`; this drops Box<V> and thus `V`.
     - Remove the Entry from HashMap with `map.remove(&*entry)` (using Borrow<Entry<K>>).
     - Box<Entry<K>> is dropped, freeing the Entry allocation; RcEntry must not access entry fields after removal.

Complexity
- get/insert/remove: O(1) average.
- RcEntry clone/drop: O(1).
- Hashing RcEntry: O(1) by hashing `DefaultKey`.
- Memory: per-entry overhead ~ Box<Entry<K>> + K + Cell<usize> + DefaultKey + pointer `val_ptr` + HashMap + SlotMap bookkeeping + Box<V>.

Safety, Soundness, and Edge Cases
- Lifetime tie: RcEntry<'map, K, V> holds `&'map RefCell<Inner<…>>`; Rust enforces that RcEntry cannot outlive the map. This prevents dangling owner pointers.
- Stable addresses: Entry is heap-allocated; Value is boxed; both addresses are stable across container resizes.
- Deref correctness: RcEntry::deref returns `&V` by dereferencing Entry.val_ptr. This is safe because:
  - The value is dropped only when refcount reaches zero; no RcEntry can be dereferenced afterward because it no longer exists.
  - Single-threaded: there is no concurrent removal while references are alive.
- Interior mutability: All mutation goes through RefCell borrows inside get/insert/drop. We ensure no RefCell borrow is held across user code calls; RcEntry::deref does not borrow the RefCell, avoiding borrow conflicts.
- clear/shrink_to_fit: Must not be called while holding RcEntry::deref’ed references across the call boundary. Because we only use RefCell internally and deref uses raw pointers, we do not create runtime borrow conflicts, but clear must still respect refcounts to avoid use-after-free; hence the debug assertion on zero refcounts.
- Reentrancy in Drop: Drop borrows the owner RefCell mutably. If a user holds an active mutable borrow (not exposed by API) this could panic; our API avoids exposing borrows to Inner and methods avoid holding a borrow when constructing/dropping RcEntry.

Trait Bounds and Generics
- K: Eq + Hash + 'static (or same lifetime as RcHashMap if needed). Must support Borrow<Q> for lookups.
- V: 'static for pointer stability; no other bounds required.
- S: BuildHasher + Default (RandomState by default).

Sketch of Core Types (signatures only)
```rust
use core::{cell::Cell, hash::Hash, ptr::NonNull};
use hashbrown::HashMap; // or std::collections::HashMap
use slotmap::{SlotMap, DefaultKey};
use std::cell::RefCell;

struct Entry<K> {
    key: K,
    refcount: Cell<usize>,
    slot_key: DefaultKey,
    val_ptr: NonNull<V>, // Requires V in scope; in concrete code Entry would be Entry<K, V>
}

struct Inner<K, V, S> {
    map: HashMap<Box<Entry<K>>, (), S>,
    values: SlotMap<DefaultKey, Box<V>>,
}

pub struct RcHashMap<K, V, S = RandomState> {
    inner: RefCell<Inner<K, V, S>>,
}

pub struct RcEntry<'map, K, V> {
    entry: NonNull<Entry<K /*, V*/>>, // Entry references V via val_ptr
    owner: &'map RefCell<Inner<K, V, S /* erased via lifetime only */>>, // S not used in RcEntry methods
}
```

Notes on the sketch:
- In actual implementation Entry must be generic over V to store `val_ptr: NonNull<V>`.
- RcEntry should not name S; it only needs access to the owner RefCell to run removals.
- We can use std::collections::HashMap or hashbrown; SlotMap is as vendored in /workspaces/slotmap.

Key trait impls to enable borrowing and identity
```rust
impl<K: Eq> PartialEq for Entry<K> { /* key == other.key */ }
impl<K: Eq> Eq for Entry<K> {}
impl<K: Hash> Hash for Entry<K> { /* key.hash(h) */ }
impl<K> std::borrow::Borrow<K> for Entry<K> { fn borrow(&self) -> &K { &self.key } }
impl<K> std::borrow::Borrow<K> for Box<Entry<K>> { fn borrow(&self) -> &K { &self.key } }

impl<'m, K, V> Clone for RcEntry<'m, K, V> { /* inc refcount via entry.refcount */ }
impl<'m, K, V> Drop for RcEntry<'m, K, V> { /* dec; on zero, remove from SlotMap + HashMap */ }
impl<'m, K, V> std::ops::Deref for RcEntry<'m, K, V> { type Target = V; fn deref(&self) -> &V { /* via val_ptr */ } }
impl<'m, K, V> PartialEq for RcEntry<'m, K, V> { /* compare slot_key */ }
impl<'m, K, V> Eq for RcEntry<'m, K, V> {}
impl<'m, K, V> Hash for RcEntry<'m, K, V> { /* hash slot_key */ }
```

Unsafe usage (justification)
- Converting shared references to NonNull pointers for Entry and V.
- Dereferencing NonNull<V> in RcEntry::deref.
- These are sound given the invariants:
  - Entry lives until removed due to last-drop; RcEntry::drop removes it exactly once.
  - V is stored in Box inside SlotMap; it is deallocated only when refcount reaches zero and RcEntry::drop removes it. No remaining RcEntry exists afterwards.
  - Single-threaded context eliminates data races; RefCell mediates container mutations.

Example Usage
```rust
let map: RcHashMap<String, Vec<u8>> = RcHashMap::new();

// Insert new or return existing
let e1 = map.get_or_insert_with("alpha".to_string(), || vec![1,2,3]);
let e2 = map.get("alpha");
assert!(e2.is_some());
let e2 = e2.unwrap();

// Cheap hashing by slot key
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
let mut h1 = DefaultHasher::new();
e1.hash(&mut h1);
let mut h2 = DefaultHasher::new();
e2.hash(&mut h2);
assert_eq!(h1.finish(), h2.finish());

// Deref to value
assert_eq!(&*e1, &vec![1,2,3]);

// Last-drop removes from both containers
drop(e1);
drop(e2); // removal happens here
assert!(!map.contains_key("alpha"));
```

Testing Plan
- Basic lifecycle: insert → clone RcEntry → drop clones → ensure removal from both containers.
- Hash/Eq correctness: RcEntry hash/eq aligns with SlotMap key and deduplicated entries.
- Borrow ergonomics: ensure no RefCell borrow panics; deref never borrows the map.
- Capacity maintenance: shrink_to_fit correctness; clear with zero refcounts.
- Fuzz/sequential stress: repeat insert/get/drop sequences; ensure no leaks (len == 0) and no panics.

Potential Extensions
- WeakRcEntry: non-owning pointer that doesn’t keep refcount > 0; upgrade() returns Option<RcEntry> if still alive.
- MT variant: ArcHashMap using Arc<Mutex<Inner>> and AtomicUsize refcounts if needed.
- Custom slot key type: parameterize over `KIdx` if consumers want typed slot keys.
- Eviction strategies: exposing TTL/LRU by augmenting Entry and removal paths.

Rationale Recap
- Boxing Entry and V decouples container relocation from pointer stability.
- Using SlotMap provides O(1) key allocation and tiny, copyable, hashable IDs for RcEntry hashing and equality.
- Single-threaded Cell-based counting is minimal and avoids atomics; RefCell provides necessary interior mutability without global registries.
- The design mirrors proven patterns (internment’s ArcIntern) while ensuring RcEntry lifetime is tied to an owning map instance, not a global container.
