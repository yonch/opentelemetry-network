RcHashMap: Single‑threaded, refcounted hash‑interning with SlotMap entries

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
- Entry<K, V>: Reference-counted record for a logical key.
  - key: K — logical key; Eq + Hash.
  - value: V — stored inline in Entry.
  - refcount: Cell<usize> — single-threaded reference count.
  - slot_key: slotmap::DefaultKey — index of this Entry in SlotMap.
  - Hash/Eq: delegate to `key` so Entry is hashable/equatable by K.

- Containers inside RcHashMap<K, V, S = RandomState>:
  - entries: slotmap::SlotMap<DefaultKey, Box<Entry<K, V>>>
    - SlotMap holds Box<Entry<K,V>> so Entry’s address is stable across SlotMap reallocations; only the Box pointer is stored in SlotMap.
  - index: std::collections::HashMap<EntryPtr<K, V>, (), S>
    - EntryPtr<K,V> is a thin, copyable key wrapper containing a NonNull<Entry<K,V>>. It lets us index by the entry’s key (without duplicating K) while keeping a stable identity into SlotMap. It implements Hash/Eq by reading the pointed-to `key`, and Borrow<Q> to support `get` lookups by `&Q` (e.g., `&str` when `K=String`).
  - hasher: S — BuildHasher for the index HashMap.

- RcEntry<'map, K, V>: The handle given to users.
  - ptr: NonNull<Entry<K, V>> — stable pointer into the Entry stored in Box within the SlotMap.
  - owner: &'map RefCell<Inner<K, V, S>> — backpointer for drop-time cleanup. Lifetime ties RcEntry to the map instance.
  - Traits:
    - Clone: increments Entry.refcount.
    - Drop: decrements; when zero, removes from both containers and drops the Entry.
    - Deref<Target = V>: returns `&V` by reading through `ptr` to `Entry.value` (no container borrow involved).
    - Eq/Hash: compare/hash by `Entry.slot_key` for O(1) hashing.

Interior Mutability and Lifetimes
- RcHashMap stores `inner: RefCell<Inner<K, V, S>>` to allow RcEntry::drop to mutate containers from `&self`.
- RcEntry holds `&'map RefCell<Inner<…>>`; RcEntry cannot outlive its owner map.
- Entry is boxed so its address is stable; RcEntry keeps a NonNull pointer to Entry and never depends on SlotMap’s element address stability.
- No aliasing UB: refcount is in a Cell; deref reads through the stable Entry pointer; SlotMap mutations (insertion/removal) do not move Entry since SlotMap stores Box<Entry<…>> by value (pointer).

Hashing and Equality
- Entry<K,V>:
  - impl Hash for Entry<K,V> { key.hash() }
  - impl PartialEq/Eq for Entry<K,V> { key.eq(&other.key) }
- EntryPtr<K,V> (HashMap key):
  - impl Hash { unsafe { ptr.as_ref().key.hash(h) } }
  - impl PartialEq/Eq { unsafe { ptr.as_ref().key == other.ptr.as_ref().key } }
  - impl<Q: ?Sized> Borrow<Q> where K: Borrow<Q> { returns `&Q` via `entry.key.borrow()` }
- RcEntry<'_, K, V>:
  - impl Hash { entry.slot_key.hash(h) }
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
    where K: Borrow<Q>, Q: Eq + Hash
    - If present, increments refcount and returns RcEntry.
  - get_or_insert_with(&self, key: K, mk_val: impl FnOnce() -> V) -> RcEntry<'_, K, V>
    - Fast path if key exists; otherwise creates new Entry in SlotMap and indexes it.
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
  1) Allocate Entry: `let entry = Box::new(Entry { key, value: mk_val(), refcount: Cell::new(1), slot_key: DefaultKey::null() /* temp */ });`
  2) Get a stable pointer: `let ptr = NonNull::from(entry.as_ref());`
  3) Insert `entry` into SlotMap: `let slot_key = entries.insert(entry);`
  4) Patch the stored `slot_key` inside the pointed Entry (via `unsafe { ptr.as_ref() }` + interior Cell or by initializing after insert with entry.slot_key = slot_key before moving the Box).
  5) Insert `EntryPtr { ptr }` into the HashMap `index` with value `()`.
  6) Return RcEntry { ptr, owner: &self.inner }.

- Lookup path (existing key):
  1) Find the pointer key in `index.get(&q)` (using Borrow<Q> on EntryPtr to borrow as `&Q`).
  2) Recover `ptr` from the key; increment `entry.refcount`.
  3) Return RcEntry { ptr, owner }.

- Drop path (RcEntry::drop):
  1) Decrement `entry.refcount`.
  2) If it becomes zero:
     - Remove from the HashMap `index` using `EntryPtr { ptr }` as the lookup key.
     - Read the slot key from `entry.slot_key` and remove the Box<Entry<…>> from SlotMap via `entries.remove(slot_key)`; this drops K and V and the Entry box.
     - RcEntry must not access entry fields after removal.

Complexity
- get/insert/remove: O(1) average.
- RcEntry clone/drop: O(1).
- Hashing RcEntry: O(1) by hashing `DefaultKey`.
- Memory: per-entry overhead ~ one Box<Entry<K,V>> + K + V + Cell<usize> + DefaultKey + HashMap + SlotMap bookkeeping.

Safety, Soundness, and Edge Cases
- Lifetime tie: RcEntry<'map, K, V> holds `&'map RefCell<Inner<…>>`; RcEntry cannot outlive the map.
- Stable addresses: Entry is boxed; its address is stable even if SlotMap reallocates; deref reads `&Entry.value` via the stable pointer.
- Deref correctness: RcEntry::deref returns `&V` by dereferencing the stable Entry pointer. The value is dropped only when refcount reaches zero; Rust prevents dropping the last RcEntry while a reference borrowed from it is alive.
- Interior mutability: Mutations go through RefCell inside get/insert/drop. RcEntry::deref does not borrow the RefCell, avoiding borrow conflicts and allowing references to V that are independent of transient container borrows.
- clear/shrink_to_fit: As before, don’t clear while outstanding RcEntry exist; enforce via debug assertions on zero refcounts.
- Reentrancy in Drop: Drop borrows the owner RefCell mutably; we avoid holding any RefCell borrow across user callbacks and do removal in a strict, linear sequence.

Trait Bounds and Generics
- K: Eq + Hash + 'static (or same lifetime as RcHashMap if needed). Must support Borrow<Q> for lookups.
- V: 'static for pointer stability; no other bounds required.
- S: BuildHasher + Default (RandomState by default).

Sketch of Core Types (signatures only)
```rust
use core::{cell::Cell, hash::Hash, ptr::NonNull, borrow::Borrow, marker::PhantomData};
use hashbrown::HashMap; // or std::collections::HashMap
use slotmap::{SlotMap, DefaultKey};
use std::cell::RefCell;

struct Entry<K, V> {
    key: K,
    value: V,
    refcount: Cell<usize>,
    slot_key: DefaultKey,
}

#[derive(Copy, Clone)]
struct EntryPtr<K, V> {
    ptr: NonNull<Entry<K, V>>,
    _pd: PhantomData<(K, V)>,
}

impl<K, V> EntryPtr<K, V> {
    unsafe fn as_ref(&self) -> &Entry<K, V> { self.ptr.as_ref() }
}

struct Inner<K, V, S> {
    index: HashMap<EntryPtr<K, V>, (), S>,
    entries: SlotMap<DefaultKey, Box<Entry<K, V>>>,
}

pub struct RcHashMap<K, V, S = RandomState> {
    inner: RefCell<Inner<K, V, S>>,
}

pub struct RcEntry<'map, K, V> {
    ptr: NonNull<Entry<K, V>>, // Stable pointer into Box<Entry<…>> stored in SlotMap
    owner: &'map RefCell<Inner<K, V, S /* erased via lifetime only */>>, // S not used in RcEntry methods
}
```

Notes on the sketch:
- SlotMap stores Box<Entry<K,V>> to make Entry addresses stable; this removes the need for a separate Box<V>.
- The HashMap stores EntryPtr (a NonNull pointer) as its key, avoiding duplication of K. Lookups by `&Q` work via `Borrow<Q>` implementations that read the key from the pointed Entry.
- RcEntry does not name S in its type; it only needs the owner RefCell to run removals.
- Use std::collections::HashMap or hashbrown; SlotMap is as vendored in /workspaces/slotmap.

Key trait impls to enable borrowing and identity
```rust
impl<K: Eq, V> PartialEq for Entry<K, V> { /* key == other.key */ }
impl<K: Eq, V> Eq for Entry<K, V> {}
impl<K: Hash, V> Hash for Entry<K, V> { /* key.hash(h) */ }

impl<K, V> EntryPtr<K, V> {
    fn key(&self) -> &K { unsafe { &self.as_ref().key } }
}
impl<K, V> std::hash::Hash for EntryPtr<K, V>
where K: Hash
{ fn hash<H: std::hash::Hasher>(&self, h: &mut H) { self.key().hash(h) } }
impl<K, V> PartialEq for EntryPtr<K, V>
where K: PartialEq
{ fn eq(&self, other: &Self) -> bool { self.key() == other.key() } }
impl<K: Eq, V> Eq for EntryPtr<K, V> {}
impl<K, V, Q: ?Sized> Borrow<Q> for EntryPtr<K, V>
where K: Borrow<Q>
{ fn borrow(&self) -> &Q { self.key().borrow() } }

impl<'m, K, V> Clone for RcEntry<'m, K, V> { /* inc refcount via entry.refcount */ }
impl<'m, K, V> Drop for RcEntry<'m, K, V> { /* dec; on zero, remove from HashMap + SlotMap */ }
impl<'m, K, V> std::ops::Deref for RcEntry<'m, K, V> { type Target = V; fn deref(&self) -> &V { /* via ptr -> Entry.value */ } }
impl<'m, K, V> PartialEq for RcEntry<'m, K, V> { /* compare slot_key */ }
impl<'m, K, V> Eq for RcEntry<'m, K, V> {}
impl<'m, K, V> Hash for RcEntry<'m, K, V> { /* hash slot_key */ }
```

Unsafe usage (justification)
- NonNull pointers: RcEntry and EntryPtr store NonNull<Entry<…>> for stable addressing and O(1) deref.
- Dereferencing NonNull<Entry> in RcEntry::deref and EntryPtr trait impls.
- These are sound given invariants:
  - Entry is heap-allocated (Box) and not moved while alive; SlotMap stores the Box by value.
  - Last-drop removes from index first, then from SlotMap; after removal, no further deref occurs.
  - Single-threaded: no concurrent aliasing; RefCell mediates mutations.

On “Entry stored directly in SlotMap”
- Storing `Entry<K,V>` by value inside SlotMap (without an extra Box) looks attractive, but is incompatible with safe `Deref` that returns `&V` while allowing insertions:
  - SlotMap may reallocate and move elements on growth; `&V` references obtained earlier would be invalidated.
  - Using RefCell to mutate from `&self` would bypass compile-time borrow checks, risking UB by invalidating outstanding `&V`.
- The adopted compromise stores `Box<Entry<K,V>>` in SlotMap: a single heap allocation per entry (no separate Box<V>), stable addresses for safe `&V` deref, and cheap hashing by the SlotMap key on RcEntry.

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
