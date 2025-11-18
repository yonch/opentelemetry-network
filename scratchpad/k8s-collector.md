Kubernetes Metadata Collector (current Go watcher + C++ relay)

Purpose
- Collect Kubernetes pod metadata and forward to the downstream ingest pipeline.
- Only emit pod metadata once the pod and its “effective” owner are known.
- Maintain a live set of pods, with container details, and handle deletions and resynchronization.

Existing Components
- k8s-watcher (Go): connects to the relay via gRPC and streams Kubernetes events.
- k8s-relay (C++): gRPC server that ingests watcher events, correlates pods with owners, and writes normalized messages to the ingest pipeline. Implements resync machinery.

Existing Protocol and Data Model
- gRPC service: `Collector.Collect(stream Info) returns (stream Response)`
  - Client (watcher) streams `Info` messages to the server (relay).
  - Server may stream back a single `Response` to signal a controlled reset; after which the server cancels the RPC.
- Info message types (`collector.Info.type`):
  - `K8S_REPLICASET`: carries `ReplicaSetInfo` (UID + OwnerInfo)
  - `K8S_POD`: carries `PodInfo` (UID, IP, Name, Namespace, OwnerInfo, Version, HostNetwork, Containers)
  - `K8S_JOB`: carries `JobInfo` (UID + OwnerInfo)
- Event kinds (`collector.Info.event`): `ADDED`, `MODIFIED`, `DELETED`, `ERROR`.
- Owner resolution: OwnerInfo derives from the Kubernetes `ownerReferences` array by selecting the first entry with `controller=true`.
- Pod “version”: a stable string built from the set of container images found in `pod.status.containerStatuses`. Images are collected as quoted strings, sorted, and joined by commas. Example: `'gcr.io/foo/app:1','gcr.io/foo/sidecar:2'`.
- Containers: each pod reports `container_infos` with `id` (runtime container ID), `name`, and `image` from `status.containerStatuses`.

Watcher Behavior (Go)
- Connection
  - Dials the relay via gRPC (insecure), opens a bidirectional stream to `Collector.Collect`.
  - Spawns a goroutine that calls `stream.Recv()` exactly once. Any response from the relay triggers a local cancel path to immediately stop all watches and exit `run()`; `main()` will retry after a short sleep.
- Initial list (full snapshot)
  - Lists (namespaceAll) in order: ReplicaSets, then Jobs, then Pods.
  - Sends an `Info` message with event `ADDED` for every resource returned in each list.
  - Saves the `ListMeta.resourceVersion` for each kind (pod/rs/job) to start watches from a consistent point.
- Watches (incremental changes)
  - Starts three watches using the saved resource versions:
    - Pods: `corev1.Pods(...).Watch(ResourceVersion=pod_version)`
    - ReplicaSets: `appsV1.ReplicaSets(...).Watch(ResourceVersion=rs_version)`
    - Jobs: `batchV1.Jobs(...).Watch(ResourceVersion=job_version)`
  - Processes events in a `select` loop, per-kind handler:
    - Maps K8s watch event type to `Info.event` (Added/Modified/Deleted/Error).
    - Type-asserts the object; on failure, returns error and restarts the whole run.
    - Builds the appropriate `*Info` message and sends.
    - Updates the tracked resourceVersion with the event object’s `metadata.resourceVersion`.
  - Rotation: every 5 minutes, stops all watches and restarts the watch loop using the latest tracked resourceVersions (no re-list).
- Error handling and reset
  - If the stream receives a `Response` from the relay or the gRPC context is canceled, stops all watches and returns, causing `main()` to restart the entire `run()` sequence (including a fresh full re-list).
  - Any error in handling events or starting watches stops all watches and returns, causing a retry.
- Owner extraction and pod payload details
  - Owner: picks the first controller ownerReference (if any). If none, `owner` is omitted in the sent `PodInfo`.
  - Pod IP: uses `status.podIP`.
  - Host-network flag: uses `spec.hostNetwork`.
  - Containers: uses `status.containerStatuses` to populate `(id, name, image)`.
  - Version string: deterministically built from current container images.
  - Local test mode exists to generate synthetic RS+Pod streams without connecting to the API server.

Relay Behavior (C++)
- High level
  - Receives `Info` events over gRPC.
  - Maintains in-memory state for owners (ReplicaSets and Jobs) and pods.
  - Emits messages downstream (via `ebpf_net::ingest::Writer`) only when sufficient context is available.
  - Implements resynchronization and controlled watcher resets through a queue and callback mechanism.
- State
  - UID compaction: maps string UIDs to sequential u64 IDs for internal maps.
  - Owner store (by owner UID):
    - `infos`: current `OwnerInfo` for known ReplicaSets/Jobs (the owner’s owner is stored here; e.g., RS->Deployment, Job->CronJob).
    - `waiting`: map from owner UID to a vector of pod IDs that need this owner context before emission.
    - `deleted`: FIFO of recently deleted owners. Capacity is 10,000; when exceeded, the oldest owner is purged from `infos` and its UID mapping removed.
  - Pod store (by pod UID):
    - `infos`: latest `PodInfo` seen for each pod (merged on updates).
    - `live`: set of pods that have already been emitted downstream via a “pod new” event.
    - `waiting`: set of pods currently waiting for an owner to become available.
- Emission rules (downstream)
  - Only sends “pod new” once per pod UID (tracked via `live`).
  - Only sends a pod when `pod.status.podIP` is non-empty. If IP absent, defers.
  - Ownerless pods: if no controller owner exists, sends “pod new (NoOwner)” using the pod’s own name as owner/display name.
  - Pod owned by neither ReplicaSet nor Job (e.g., DaemonSet, StatefulSet without RS): sends “pod new” with that direct owner as-is.
  - Pod owned by a ReplicaSet:
    - If the ReplicaSet’s owner is a Deployment, sends “pod new” with the Deployment as the effective owner.
    - Otherwise, sends “pod new” with the ReplicaSet as owner.
  - Pod owned by a Job:
    - If the Job’s owner is a CronJob, sends “pod new” with the CronJob as owner.
    - Otherwise, sends “pod new” with the Job as owner.
  - Containers: whenever a “pod new” is sent, all containers `(id, name, image)` are sent in separate messages. If a pod is already live and receives further updates, only container messages are sent (no repeat “pod new”).
  - Pod deletion: on `DELETED`, if the pod was live, sends a “pod delete”. Removes the pod from all stores and UID map.
- Owner lifecycle
  - On RS/Job `ADDED`/`MODIFIED`: upserts `OwnerInfo` and releases any pods waiting on that owner (re-checks that the pod still belongs to this owner UID before emitting).
  - On RS/Job `DELETED`: tracks deletion in `deleted` queue for bounded time; purges oldest beyond the 10k threshold. No direct downstream message is sent for owner deletion.
- Guard rail: waiting pods cap
  - If the number of pods waiting on owners reaches 10,000, the relay marks the stream for restart (see Resync below) to recover from pathological skew or missed owner events.

Resynchronization and Reset Semantics
- Motivation: keep watcher and relay state aligned with the downstream pipeline, and recover from missed events or connection disruptions.
- Building blocks
  - `ResyncQueue` (C++): a shared queue between the relay’s gRPC handler (producer) and a background `ResyncProcessor` (consumer). Each producer (gRPC stream) is wrapped in a `ResyncChannel` tagged with a monotonically increasing resync generation.
  - `ResyncProcessor`: reads from the queue and relays messages to the ingest pipeline via a reconnecting channel. Tracks an `active_resync_` generation and whether any messages were sent since the last resync (`dirty_`). Periodically polls and also sends heartbeats and resource-usage.
  - Reset callback: each `ResyncChannel` holds a callback that, when invoked, writes a single `Response` to the watcher and immediately cancels the gRPC context. The watcher detects this and restarts.
- When resets happen
  - Relay-initiated guard rail: if waiting pods >= 10,000, the relay stops reading and returns `CANCELLED`, causing the watcher to reconnect and perform a full re-list. The `ResyncProcessor` then observes a new channel/resync generation.
  - Downstream reconnect (pipeline connection reset): upon reconnect, if any data had been sent before (`dirty_==true`), the `ResyncProcessor` triggers a producer reset (`consumer_reset()`), which invokes the channel’s reset callback. The watcher receives a `Response`, stops watches, and restarts with a full list.
  - Inconsistent queue state: if the consumer observes a malformed element (length <= 8), it sends a `pod_resync` (if dirty), advances `active_resync_` and calls `consumer_reset()` to force a watcher restart.
  - Periodic watcher rotation: independent of the above, the watcher restarts its three watches every 5 minutes, with updated `resourceVersion` (no full re-list). This mitigates long-lived watch drift.
- Resync message to downstream
  - When the resync generation increases and `dirty_` is true, the `ResyncProcessor` sends a `pod_resync(active_resync_)` control message to the ingest pipeline and flushes, indicating that downstream should reconcile its Kubernetes view. Subsequent pod events are treated as authoritative for the new generation.

Failure Handling Summary
- Watcher to relay gRPC
  - Any error creating watches, handling events, or an explicit server cancel leads to closing the stream and retrying the entire run after a short delay (200ms). Retries always begin with a fresh full re-list of RS, Jobs, and Pods.
  - A single server `Response` is treated as a shutdown signal; watcher stops watches and restarts.
- Kubernetes API watches
  - Watchers are started from the last seen resourceVersion. Every 5 minutes they are torn down and re-established using the latest resourceVersion. Errors during watch handling cause a full reconnect and re-list on the next `run()`.
  - Pod events with empty IP are ignored until a subsequent event provides an IP.
  - Owner lookups depend on controller ownerReferences; no ownerReference implies “ownerless pod” handling.
- Relay to downstream pipeline
  - Uses a reconnecting channel. On reconnects with prior activity, it forces a watcher reset and bumps the resync generation.
  - After every accepted watcher message, the relay flushes its buffered writer (messages may still be batched internally by the underlying queue).

Exact Ordering and De-duplication
- Initial sequence on a fresh connection:
  1) Watcher lists RS, then Jobs, then Pods; emits ADDED for each in that order.
  2) Relay upserts RS/Job owners first, so that when pod ADDED arrives, many pods can be emitted immediately without waiting.
- For pods that arrive before their owners:
  - Relay stores the pod, marks it waiting, and defers emission until the owner (or owner’s owner) arrives; once available, emits a single “pod new” and associated containers.
- Deduplication:
  - A pod is considered “live” once a “pod new” has been emitted for its UID. Subsequent pod MODIFIED events only cause container info updates, not another “pod new”. Deletions are only forwarded if the pod was live.

Edge Cases and Nuances
- Owner escalation:
  - RS->Deployment: pod owner escalates to Deployment.
  - Job->CronJob: pod owner escalates to CronJob.
  - Otherwise, pod owner is the immediate controller owner.
- Owner deletions:
  - Relay does not emit anything downstream when owners (RS/Job) are deleted. Owners are retained in a bounded `deleted` list to avoid immediate churn in UID mapping; purged oldest-first beyond 10k entries.
- Pod updates after live:
  - If a pod’s owner changes or IP changes after it has been marked live, the relay does not emit a new “pod new”; it only re-sends container info on subsequent updates. The downstream is expected to manage such transitions based on container and later delete events.
- Version string:
  - Based on `status.containerStatuses` images, not `spec.containers`. Intended to be deterministic and reflect actual runtime images.

Key Source References
- Watcher implementation: `collector/k8s/k8s-watcher/main.go:1`
- gRPC API (proto): `collector/k8s/collector.proto:1`, `collector/k8s/kubernetes_info.proto:1`
- Relay server and state machine: `collector/k8s/kubernetes_rpc_server.cc:1`
- Resync channel/queue/processor: `collector/k8s/resync_channel.h:1`, `collector/k8s/resync_queue.h:1`, `collector/k8s/resync_processor.cc:1`

Proposed Rust Collector Design (kube-rs)

Goals
- Merge watcher and relay into one Rust binary with kube-rs.
- Keep semantics identical to the current system while simplifying implementation.
- Strong testability with clear, small, composable components and traits.
- Continue to use render-generated ingest Writer/Encoder (no render Index in the collector).

High-Level Architecture
- Tokio-based app wiring distinct async “pipes”:
  - kube-rs watchers (Pods, ReplicaSets, Jobs) → Event-converters → typed Event streams (PodMeta, OwnerMeta)
  - Stream adapter (Owner tombstone): defers Delete events to keep recently-deleted owners available
  - Reflector Stores over Meta types (Store<PodMeta>, Store<OwnerMeta>) fed by the converted streams
  - Matching engine that consumes PodMeta and OwnerMeta events and consults Stores to emit metadata events
  - Downstream sink that maps metadata events to render Writer calls
  - Resync behavior integrated with the Pod stream (no central controller)

Rust Port: kube-rs Watcher Adoption
- Event protocol: adopt `kube::runtime::watcher::Event` end-to-end.
  - Variants: `Apply(K)`, `Delete(K)`, `Init`, `InitApply(K)`, `InitDone`.
  - Semantics: `Init` marks a re-list sequence; a stream of `InitApply` follows with all current objects; `InitDone` finalizes the snapshot; thereafter, incremental `Apply`/`Delete` events resume.
- Auto-recovery: kube-rs watcher reconnects/restarts automatically.
  - If the watch drops, it restarts from the last seen resourceVersion; if resourceVersion is too old (HTTP 410 Gone), watcher falls back to a fresh list and emits `Init`…`InitDone`.
  - Remove periodic rotation and bespoke re-list logic; rely on watcher recovery.
- Stores: use `kube_runtime::reflector::Store` per kind to maintain live sets.
  - Feed watcher events into the Store; it understands `Init`/`InitApply`/`InitDone` to atomically refresh state.
  - Keep lightweight, in-process maps for correlation: owners and pods, plus a bounded waiting index for pods missing owner context.

Changes vs. Legacy (Go watcher + C++ relay)
- Replace custom Added/Modified/Deleted + out-of-band restart with `watcher::Event` protocol (`Init`/`InitApply`/`InitDone`, `Apply`, `Delete`).
- Remove periodic watcher rotation and manual watch+list restarts; rely on kube-rs auto-recovery and re-list signaling.
- Replace relay-driven gRPC cancel/reset with local epoch bumps and `pod_resync` to downstream; keep kube watchers running.

Writer Protocol (unified Rust binary)
- Handshake and compression
  - On connect, send `version_info(major, minor, patch)` uncompressed and flush.
  - Immediately enable LZ4 compression for subsequent messages.
  - Send `connect(client_type=k8s, hostname)`, `report_cpu_cores`, configuration labels, cloud/node metadata, and `metadata_complete(0)`.
- Data phase
  - Control: `pod_resync(epoch)` when resynchronizing downstream state.
  - Data: `pod_new_with_name(...)`, `pod_container(...)`, `pod_delete(...)`, `collector_health(...)` as needed.
  - Reducer behavior: on `pod_resync`, the reducer clears its k8s pod state for this agent and expects a fresh snapshot. The upstream TCP/TLS connection does not need to be restarted; the collector bumps epoch and resends the live set.

Compatibility Notes (parity with legacy)
- Container IDs: send `status.containerStatuses[*].containerID` as-is (with runtime prefix); reducer normalizes by stripping `<runtime>://`.
- Host network: preserve `spec.hostNetwork` and forward as `is_host_network` in `pod_new`.
- Pod IPs: use `status.podIP` (single IP) for now; `status.podIPs` (dual-stack) remains a documented gap.

Core Data Types
- PodMeta: { uid, ip, name, ns, version, is_host_network, containers: [{id, name, image}], owner: Option<OwnerRef>, resource_version }
- OwnerMeta: { uid, name, ns?, kind, controller: Option<OwnerRef>, resource_version } // RS/Job; controller is RS→Deployment or Job→CronJob if present
- OwnerRef: { uid, name, kind }
- Watcher Event<T>: kube-rs protocol: `Apply(T)`, `Delete(T)`, `Init`, `InitApply(T)`, `InitDone`
- EmitEvent: PodNew { pod: PodMeta, owner: OwnerRef | NoOwner }, PodContainers { uid, containers }, PodDelete { uid }, Resync { epoch }
- OwnerKind enum matches current behavior: ReplicaSet, Deployment, Job, CronJob, NoOwner, Other

1) Watch + Convert + Reflect
- Sources: kube-rs `watcher()` for Pods, ReplicaSets, Jobs over all namespaces with desired selectors.
- Converters:
  - Map `watcher::Event<k8s_obj>` to `watcher::Event<PodMeta|OwnerMeta>` by transforming object payloads while preserving the event variant (`Init`, `InitApply`, `InitDone`, `Apply`, `Delete`).
  - pod_to_meta(pod: k8s::Pod) -> PodMeta
  - rs_to_owner(rs: k8s::ReplicaSet) -> OwnerMeta
  - job_to_owner(job: k8s::Job) -> OwnerMeta
- Owner tombstone adapter (stream-level):
  - Intercepts `Event::Delete(OwnerMeta)` and retains it in a bounded TTL queue; forwards other events immediately.
  - Releases oldest delete when capacity is exceeded; releases expired deletes opportunistically before forwarding any non-delete event.
  - Purpose: keep deleted owners briefly available to satisfy straggling Pod updates without maintaining custom store tombstones.
- Reflectors over Meta:
  - Use `reflector::store()` to obtain `(StoreWriter<PodMeta>, Store<PodMeta>)` and `(StoreWriter<OwnerMeta>, Store<OwnerMeta>)`.
  - Feed the converted streams into `reflector(writer, stream)`. The Store understands `Init`/`InitApply`/`InitDone` and swaps atomically on `InitDone`.
  - Note: `Store<K>` requires `K: reflector::Lookup`. Implement `Lookup` for `PodMeta` and `OwnerMeta` (see notes below).
 - Tests: use a `MockWatchSource` and verify event mapping and store content across `Init` cycles.

2) Converters and Stream Mapping
- Convert K8s objects → Meta while preserving kube-rs Event protocol:
  - `Event::Apply(obj)` → `Event::Apply(f(obj))`
  - `Event::InitApply(obj)` → `Event::InitApply(f(obj))`
  - `Event::Delete(obj)` → `Event::Delete(f(obj))`
  - `Event::Init` and `Event::InitDone` passed through unchanged
- pod_to_meta: OwnerRef from controller ownerReferences (if any), version from sorted status.containerStatuses[*].image, containers meta, hostNetwork, podIP.
- rs_to_owner / job_to_owner: capture immediate owner and controller chain (Deployment/CronJob) if present.
- Rationale: keep memory footprint small and enable Stores over compact Meta.

3) Stores and Tombstones
- Stores over Meta:
  - Use `kube_runtime::reflector::Store<PodMeta>` and `Store<OwnerMeta>`.
  - Implement `reflector::Lookup` for both Meta types so Stores can key objects by name/namespace and track resourceVersion/uid in `ObjectRef::extra`.
- Owner tombstones via stream adapter (not custom Store):
  - The delete-retainer sits before the OwnerMeta Store writer and defers `Delete` events within TTL/capacity limits.
  - On `Init`/`InitApply`/`InitDone`, the Store buffers and then swaps; deferred deletes are no-ops if the owner is already absent post-swap.
  - Parameters: TTL ≈ 60s; capacity ≈ 10k (tunable).
  - PodMeta Store receives deletes immediately; we do not defer pod deletions.

4) Matching Engine (Pods ↔ Owners)
- Purpose: replicate K8sHandler logic with clearer structure.
- API:
  - struct Matcher { live_pods, live_owners, waiting_by_owner: HashMap<owner_uid, Vec<pod_uid>>, live_set: HashSet<pod_uid>, epoch }
  - fn handle_pod(ev: watcher::Event<PodMeta>) -> Vec<EmitEvent>
  - fn handle_owner(ev: watcher::Event<OwnerMeta>) -> Vec<EmitEvent>
  - fn escalate_owner(pod: &PodMeta, owner_meta: Option<&OwnerMeta>) -> OwnerRef
    - If pod owner.kind ∈ {ReplicaSet, Job} and owner_meta exists:
      - If owner_meta.controller.kind ∈ {Deployment, CronJob} return controller; else return pod.owner.
    - If pod has owner not in {ReplicaSet, Job}: return that owner.
    - If no owner: return NoOwner.
- Emission rules:
  - Only emit PodNew when pod.ip != "" and not already live.
  - If ownerless: emit PodNew with NoOwner and PodContainers.
  - If owner is not RS/Job: emit PodNew with that owner and PodContainers.
  - If owner is RS/Job and RS/Job meta is unknown: queue pod in waiting_by_owner; enforce waiting_limit to trigger Resync.
  - On Owner Added/Modified: drain waiters, recompute escalate_owner, emit PodNew+PodContainers; mark pods live.
  - On Pod Apply for a live pod: emit PodContainers only.
  - On Pod Delete: if live, emit PodDelete and remove from live/waiting.
- Resync behavior (no central controller):
  - On Pod `Event::Init`: bump epoch, the Writer sends `pod_resync(epoch)`, and the Matcher resets its waiting_by_owner and emission gating for the new epoch (either clear `live_set` or track per-epoch emission to re-emit pods).
  - Treat `InitApply` and `Apply` uniformly for both Pods and Owners: a Pod is released immediately if its Owner is already available (or NoOwner), otherwise it is queued in `waiting_by_owner`. When the Owner `Apply` arrives, drain and release waiting pods.
- Tests: table-driven sequences for all orderings and edge cases; property tests around waiting-limit behavior.

5) Downstream Mapping (Render Writer)
- Trait: DownstreamSink
  - fn pod_new(&mut self, pod: &PodMeta, owner: &OwnerRef)
  - fn pod_new_no_owner(&mut self, pod: &PodMeta)
  - fn pod_container(&mut self, uid: &str, c: &ContainerMeta)
  - fn pod_delete(&mut self, uid: &str)
  - fn pod_resync(&mut self, epoch: u64)
  - fn collector_health(&mut self)
- Impl RenderSink: wraps buffered writer + encoder (render-generated) and maps calls to writer methods used today (no Index).
- TestSink: records calls for assertion in unit tests.

Resync Behavior (Integrated)
- No centralized controller.
- On Pod `Event::Init`:
  - Writer emits `pod_resync(epoch++)`.
  - Matcher clears its waiting_by_owner and resets emission gating for the new epoch (clear `live_set` or track per-epoch).
- During re-list (`InitApply` … `InitDone`):
  - Matcher processes `InitApply` exactly like `Apply` for both Pods and Owners.
  - Pods are released as soon as they satisfy emission rules (owner known or NoOwner, IP present). Others are queued by owner and released when the Owner `Apply` event arrives.
  - Watchers are not restarted; the kube-rs watcher/reflector manages recovery and atomic store swaps.

Implementation Notes: Lookup for Meta types
- The reflector `Store<K>` requires `K: reflector::Lookup`.
- Implement `Lookup` for `PodMeta` and `OwnerMeta` by:
  - Defining `type DynamicType = ()` and using unit `()` for the dynamic type with `Default`.
  - Returning stable literals for `kind`, `group`, `version`, `plural` (e.g., kind "PodMeta", group "otel.ebpf.net", version "v1alpha1").
  - Mapping `name`, `namespace`, `resource_version`, `uid` from the Meta fields (ensure Meta carries `resource_version`).

Concurrency and Composition
- Tokio tasks per watch source; outputs are fused with `futures::stream::select` into a single event bus with typed channels.
- Matching runs in a single task that receives Owner and Pod events via mpsc channels to ensure ordered, deterministic handling.
- DownstreamSink runs in the same task (synchronous calls) to keep emission ordering simple; optionally buffered.

Testing Strategy
- Unit tests
  - Converters: K8s fixtures → expected PodMeta/OwnerMeta.
  - Stores: insertion, deletion retention, TTL GC, capacity GC.
  - Matcher: exhaustive scenarios for owner-first/pod-first, ownerless, non-RS/Job owners, CronJob/Deployment escalation, empty IP deferral, updates, deletions, waiting-limit resync.
  - Downstream mapping: TestSink assertions match expected calls and ordering.
- Integration tests (no cluster)
  - MockWatchSource streams piped through converters → stores → matcher → TestSink; assert end-to-end emission.
- E2E tests (kind cluster)
  - Deploy minimal workloads: Deployment, CronJob, DaemonSet; churn pods; assert observed emissions and resync epochs.

Operational Notes
- We will not use the render Index in the collector; only the generated Writer/Encoder.
- Container IDs and version-string semantics preserved as described above.
- Optional periodic watcher rotation can be disabled if kube-rs watcher/reflector restart behavior is sufficient; epoch/resync will still be driven by downstream reconnects and guard-rails.
- Rust Writer Implementation
- Existing pieces: generated Rust encoder exists at `crates/render/ebpf_net/ingest/src/encoder.rs`.
- Missing pieces: a Rust `Writer` wrapper with buffered I/O, TCP/TLS channel, and LZ4 framing, plus connection caretaker (version handshake, metadata, heartbeat).
- Plan: implement a minimal channel and caretaker mirroring `ReconnectingChannel` + `ConnectionCaretaker` behavior in Rust, then map collector calls to encoder functions.
