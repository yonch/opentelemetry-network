RcHashMap: Single-threaded, refcounted map with a boxless SlotMap

Overview
- Goal: A HashMap-like structure storing reference-counted key→value pairs. Clients receive Rc-like handles (Ref) that provide identity and lifetime management; all value access happens only through RcHashMap methods to preserve exclusivity and safety.
- Design: Store Entry<K,V> inline inside a SlotMap for stable keys; maintain a separate raw index keyed by a precomputed u64 hash that maps to SlotMap slots. A Ref identifies entries by the SlotMap key. All value access uses RcHashMap::{access, access_mut} so that inserts and other &mut operations cannot run while a user holds a reference to V.
- Deferred deletion: When an entry’s refcount reaches 0, it is queued on a singly linked “dropped slots” list. Physical removals occur during `&mut self` operations (insert/get_or_insert/access_mut/shrink_to_fit) and via explicit `cleanup()`/`shrink_to_fit()`.
- Keepalive-on-drop: The map owns a `Box<Inner>`. If the map is dropped while Refs exist, `Inner` is put into a keepalive state via `Box::into_raw`. The last `Ref` drop reconstructs the box and drops it to free `Inner`.
- Constraints: Single-threaded; no atomics; no per-entry heap allocations; no global registries. Ref clone/hash/eq/drop are O(1).

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
  - inner: Box<Inner<K, V, S>> — owning handle of map state. Refs capture a raw pointer to this Inner at creation time. RcHashMap is not Clone by design.
- Inner<K, V, S>
  - entries: slotmap::SlotMap<DefaultKey, Entry<K, V>>
  - index: hashbrown::raw::RawTable<DefaultKey> — stores slots, keyed by the precomputed u64 hash; lookups compare candidate entries’ keys for Eq.
  - hasher: S — BuildHasher used for all key hashing.
  - dropped_head: Cell<Option<DefaultKey>> — head of a singly linked list of slots whose refcount reached 0 and are pending physical removal.
  - dropped_len: Cell<usize> — number of slots currently queued on the dropped list.
  - keepalive_raw: Cell<Option<NonNull<Inner<K, V, S>>>> — populated when RcHashMap is dropped while Refs exist; reclaimed exactly once by the last Ref.
- RefOrNext (internal representation)
  - Count(usize) — current Ref count; Count(1) means one live handle.
  - Next(Option<DefaultKey>) — queued for deletion; value stores the next slot in the dropped list (None marks end).

Handles
- Ref<K, V, S>
  - slot: DefaultKey — identifies the Entry in the SlotMap.
  - owner: NonNull<Inner<K, V, S>> — raw pointer to map state for O(1) handle ops without borrowing the map or bumping an Rc.
  - marker: PhantomData<Rc<()>> — forces `!Send + !Sync` and single-threaded semantics.
  - Clone: increments Entry refcount via interior Cell.
  - Drop: decrements; when zero, pushes slot onto the dropped list by rewriting `ref_or_next` to Next(head) and updating `dropped_head`; increments `dropped_len`. If `keepalive_raw.is_some()` and `dropped_len == entries.len()`, reclaim `keepalive_raw` with `take()` and `Box::from_raw` to free `Inner`. Physical removal is deferred.
  - Does not implement Deref. All value access goes through RcHashMap::{access, access_mut}.
  - Does not implement Copy. Cloning a Ref increments the entry’s refcount.
  - Owner pointer acquisition: computed once at Ref creation via `NonNull::from(self.inner.as_ref())`.

Hashing and Equality
- Entry<K,V>: Hash/Eq delegate to `key`.
- Ref: Hash and Eq use `(owner_ptr, slot)` to avoid cross-map collisions, where `owner_ptr` is the `NonNull<Inner>` address.

Index Operations (RawTable)
- Insert: compute u64 hash with the map’s hasher; insert the slot into `index` using that hash. Store the same hash in Entry.
- Get: compute u64 hash; probe `index` for that hash and linearly check candidate slots’ `entries[slot].key == query_key`.
- Remove: compute u64 hash; probe for candidates and remove the matching slot if keys are equal.

Drop and Keepalive
- Dropping RcHashMap:
  - First decide if keepalive is required using the live-length check: if `len() == 0` (i.e., `entries.len() == dropped_len` because `len() = entries.len() - dropped_len`), drop `inner` normally (no cleanup), which also drops all entries.
  - Otherwise (live refs exist), run `cleanup_dropped()` to reclaim all queued-for-deletion entries, then move `inner` into a raw pointer with `Box::into_raw` and store it in `keepalive_raw`. The map value is now logically dropped; only `Refs` keep `Inner` alive.
- Dropping Ref:
  - On last drop of an entry, queue it on the dropped list and increment `dropped_len`.
  - If `keepalive_raw.is_some()` and `dropped_len == entries.len()`, take the pointer out of `keepalive_raw` and call `unsafe { drop(Box::from_raw(ptr)) }` exactly once to free `Inner`.
- Notes:
  - `RcHashMap::drop` only runs `cleanup_dropped()` on the keepalive path (when live refs exist). If no live refs exist, dropping `inner` directly is more effective than explicit cleanup.
  - It never removes entries with non-zero refcounts.
  - Mutating operations require a live `RcHashMap`, so they are mutually exclusive with `keepalive_raw.is_some()`; during drop, keepalive is set only after keepalive-path cleanup completes.

Cleanup Algorithm (Reentrancy-safe)
- Physical removals are performed by `cleanup_dropped()` as part of `&mut self` methods and `cleanup()`/`shrink_to_fit()`.
- To avoid losing nodes if `Ref::drop` runs during cleanup (e.g., via K/V drop code), `cleanup_dropped()` “takes” the producer list:
  - Read and clear the head: `let to_clean = mem::take(&dropped_head)`.
  - Process the private chain `to_clean` to completion: for each slot, read its next pointer first; remove from `index` and `entries`; then decrement `dropped_len` (with a debug assert it was > 0).
  - Do not write to `dropped_head` while processing `to_clean`. Any concurrent pushes (from `Ref::drop`) go onto the now-empty producer head and are not lost.
  - Loop until `dropped_head` is empty; if pushes occur while processing, repeat to ensure all queued entries are physically removed before completing the mutating operation.

Safety and Soundness
- Single-threaded: `RcHashMap<K, V, S>` and `Ref<K, V, S>` are not Send nor Sync (PhantomData<Rc<()>>).
- Access paths: `Ref` never dereferences to `V`. All `&V`/`&mut V` are obtained exclusively via `RcHashMap::{access, access_mut}`, tying lifetimes to a map borrow.
- Owner validation: All APIs that accept a `Ref` validate map identity. On mismatch, they return `None` instead of panicking.
- Liveness: An entry is live when `refcount > 0`. `len()` counts only live entries and is computed as `entries.len() - dropped_len`. `get`/`contains_key` ignore queued-for-deletion entries.
- Drops avoid structural mutation: `Ref::drop` only updates interior Cells and the dropped list head/length. Physical removals happen during cleanup in `&mut self` methods.
- Hash collisions: The raw index is keyed by the precomputed u64; candidates are verified with `K: Eq`. No per-entry heap allocations.
- Expect over unchecked: Accessors use checked unwraps (`expect`) for SlotMap lookups, even in release builds. Although liveness is an invariant, we prefer safety here.
- Refcount bounds: Refcount increments/decrements use default Rust overflow behavior: in debug builds, overflow panics; in release builds, overflowing the refcount is considered undefined behavior and documented as such.
- Reentrancy: Rust’s borrow rules prevent reentrancy into `&mut self` APIs while they are active. However, K/V Drop code can run during cleanup; the “take the list” algorithm ensures no nodes are lost in that case.

Trait Bounds
- K: Eq + Hash + Borrow<Q> for lookups; no 'static bound required.
- V: no bounds.
- S: BuildHasher + Default (RandomState default).

Type Sketch
```rust
use core::{cell::Cell, hash::Hash, borrow::Borrow, ptr::NonNull, marker::PhantomData, mem};
use std::rc::Rc;
use hashbrown::raw::RawTable;
use slotmap::{SlotMap, DefaultKey};

#[derive(Copy, Clone)]
enum RefOrNext { Count(usize), Next(Option<DefaultKey>) }

struct Entry<K, V> {
    key: K,
    value: V,
    ref_or_next: Cell<RefOrNext>,
    hash: u64,
}

struct Inner<K, V, S> {
    entries: SlotMap<DefaultKey, Entry<K, V>>,
    index: RawTable<DefaultKey>,
    hasher: S,
    dropped_head: Cell<Option<DefaultKey>>,
    dropped_len: Cell<usize>,
    keepalive_raw: Cell<Option<NonNull<Inner<K, V, S>>>>,
}

pub struct RcHashMap<K, V, S = RandomState> {
    inner: Box<Inner<K, V, S>>,
    _nosend: PhantomData<Rc<()>>,
}

pub struct Ref<K, V, S = RandomState> {
    slot: DefaultKey,
    owner: NonNull<Inner<K, V, S>>,
    _nosend: PhantomData<Rc<()>>,
}

impl<K, V, S> RcHashMap<K, V, S> {
    pub fn access<'a>(&'a self, r: &Ref<K, V, S>) -> Option<&'a V> {
        // Fail if the handle belongs to a different map
        let owner_ptr = NonNull::from(self.inner.as_ref());
        if !core::ptr::eq(owner_ptr.as_ptr(), r.owner.as_ptr()) { return None }
        let inner: &Inner<K, V, S> = &self.inner;
        let e = inner.entries.get(r.slot).expect("dangling Ref: slot not found");
        Some(&e.value)
    }

    pub fn access_mut<'a>(&'a mut self, r: &Ref<K, V, S>) -> Option<&'a mut V> {
        self.cleanup_dropped();
        let owner_ptr = NonNull::from(self.inner.as_ref());
        if !core::ptr::eq(owner_ptr.as_ptr(), r.owner.as_ptr()) { return None }
        let inner_mut: &mut Inner<K, V, S> = &mut self.inner;
        let e = inner_mut.entries.get_mut(r.slot).expect("dangling Ref: slot not found");
        Some(&mut e.value)
    }

    fn cleanup_dropped(&mut self) {
        let inner: &mut Inner<K, V, S> = &mut self.inner;
        // Take the producer list to avoid clobbering pushes during cleanup
        while let Some(mut head) = inner.dropped_head.take() {
            // Process the private chain to completion
            loop {
                // Read next pointer before any structural removals
                let next = match inner.entries.get(head).map(|e| e.ref_or_next.get()) {
                    Some(RefOrNext::Next(n)) => n,
                    _ => None,
                };
                // Remove from index (by stored hash) and entries
                if let Some(ent) = inner.entries.get(head) {
                    let h = ent.hash;
                    // remove one matching slot with this hash and key
                    inner.index.remove_entry(h, |&slot| slot == head);
                }
                inner.entries.remove(head);
                // dropped_len must be > 0 here
                let dl = inner.dropped_len.get();
                debug_assert!(dl > 0);
                inner.dropped_len.set(dl - 1);
                match next {
                    Some(n) => head = n,
                    None => break,
                }
            }
            // Loop: if pushes occurred during processing, dropped_head will be non-empty now
        }
    }

    pub fn cleanup(&mut self) { self.cleanup_dropped(); }

    pub fn shrink_to_fit(&mut self) {
        self.cleanup();
        let inner_mut: &mut Inner<K, V, S> = &mut self.inner;
        // shrink raw table if desired (implementation-specific)
        let _ = inner_mut; // placeholder
    }
}
```

Example Usage
```rust
let mut map: RcHashMap<String, Vec<u8>> = RcHashMap::new();

let a1 = map.get_or_insert_with("alpha".to_string(), || vec![1,2,3]);
let a2 = map.get("alpha").unwrap();

// Hash Ref cheaply by (map id, slot)
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
let mut h1 = DefaultHasher::new(); a1.hash(&mut h1);
let mut h2 = DefaultHasher::new(); a2.hash(&mut h2);
assert_eq!(h1.finish(), h2.finish());

// Read via RcHashMap::access (fallible owner check)
let len = map.access(&a1).unwrap().len();

// Mutate via exclusive access to the map
{
    let vmut: &mut Vec<u8> = map.access_mut(&a2).unwrap();
    vmut.push(4);
}

drop(a1); drop(a2); // last drop removes index + SlotMap entry
assert!(!map.contains_key("alpha"));
```

Testing Plan
- Lifecycle: insert → clone Ref → drop clones → ensure queuing onto dropped list → cleanup removes from index and SlotMap.
- Hash/Eq correctness: Ref equality and hashing include map identity and slot.
- Access safety: `access`/`access_mut` enforce exclusivity via &self/&mut self; verify inserts/get_or_insert cannot run while references are live.
- Identity safety: APIs that take a Ref verify map identity and return None on mismatch.
- Liveness invariant: while any Ref exists, access/access_mut always return references to live entries; get(&self) treats queued-for-deletion entries as absent.
- Capacity maintenance: `shrink_to_fit()` triggers cleanup and shrinks containers; clear with zero refcounts.
- Stress: repeat insert/get/drop sequences; ensure len == 0 after cleanup and no panics.

Rationale Recap
- Boxless entries avoid per-entry allocations while SlotMap gives stable, cheap keys.
- RawTable-based index keyed by precomputed u64 avoids duplicating K and keeps lookups O(1) average while tolerating collisions via K: Eq checks.
- Decoupling handles from borrowing the map removes the &self lifetime coupling while keeping all value access centralized in RcHashMap via access/access_mut.
- Deferred deletion keeps handle operations O(1) and avoids borrow conflicts; users can call shrink_to_fit() to proactively reclaim memory and run cleanup.
