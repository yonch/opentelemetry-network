Kubernetes Collector Port Plan — Analysis

Scope
- File reviewed: `scratchpad/k8s-collector.md`
- Goal: Assess completeness, risks, simplifications, and testability of the Rust port plan.

Summary
- The plan is thorough and mirrors current semantics closely (Go watcher + C++ relay) while proposing a pragmatic Rust merge using kube-rs.
- Architecture is decomposed well (sources → converters → stores → matcher → sink → resync), with good testing hooks.
- Main risks are around downstream writer integration and resync signaling, plus potential over-engineering (tombstones, periodic rotations) that kube-rs can simplify.

Findings (Verified in Code)
- kube-rs watcher::Event
  - Current variants: `Apply`, `Delete`, `Init`, `InitApply`, `InitDone` (not `Restarted`).
  - Behavior: on dropped watches, it restarts from last resourceVersion; on HTTP 410 Gone, it performs a fresh list and emits `Init`…`InitDone` before resuming incremental events.
- Auto-recovery
  - The watcher FSM restarts watch streams and re-lists as needed; no manual rotation required.
- Writer handshake + LZ4
  - On connect, `version_info` is sent uncompressed and flushed; compression is then enabled; subsequent messages (`connect`, `report_cpu_cores`, labels, cloud/node metadata, `metadata_complete`) are compressed.
  - During data phase, `pod_resync(epoch)` clears reducer k8s pod state for this agent; collector must resend current live pods.
  - No need to restart the upstream TCP/TLS connection to resync; bumping the epoch and resending snapshot is sufficient for the reducer.
- Legacy parity details
  - Container IDs: watcher sends `status.containerStatuses[*].containerID` verbatim; reducer strips the runtime scheme (`docker://`, `containerd://`) before indexing.
  - is_host_network: derived from `pod.spec.hostNetwork` and carried through to `pod_new_with_name`.
  - Pod IPs: only `status.podIP` is used; `status.podIPs` is not currently surfaced (gap noted below).

Completeness
- Resource coverage: Watches Pods, ReplicaSets, Jobs; escalates to Deployment/CronJob via ownerReferences. This covers Deployment→RS→Pod and CronJob→Job→Pod paths and direct owners (DaemonSet, StatefulSet). Looks sufficient for current behavior.
- Data fidelity: Captures uid, name, ns, podIP, hostNetwork, containers (id/name/image), version string (from status.containerStatuses[].image). Matches described legacy behavior.
- Ordering and live rules: Emits PodNew once per UID (non-empty IP), defers until owner ready, emits containers on updates, PodDelete on deletion. Matches legacy system.
- Resync/epoch: Mirrors legacy conditions (downstream reconnect, guard rail, watcher restart). Present, but see risks below.
- Gaps to call out:
  - Dual-stack IPs: Only `status.podIP` considered; `status.podIPs` ignored. If we need dual-stack, codify selection policy. If parity with legacy is intended, this may be acceptable.
  - Container ID normalization: `status.containerStatuses[*].containerID` includes runtime scheme (e.g., `containerd://…`). Confirm whether legacy strips the scheme or passes as-is.
  - HostNetwork mapping: Ensure `is_host_network` is actually sent in PodNew downstream mapping; the sink interface does not explicitly mention it.
  - Config/RBAC: Not specified (kubeconfig, in-cluster, label/field selectors, namespace scoping). Likely out of scope for the scratchpad, but will be required for a shippable binary.
  - Backpressure: Channel sizes and writer flushing policy are unspecified; defaults may suffice but could affect latency/CPU.

Risky or Unclear Areas
- Downstream writer integration
  - The plan assumes a render-generated Writer/Encoder available from Rust and with reconnect callbacks to drive resync. Validate:
    - Is there a Rust crate for the Writer, or do we need FFI to C/C++? If FFI, define minimal safe wrapper and lifecycle (init, flush, reconnect callback).
    - Are “reconnect notifications” actually surfaced? If not, resync-on-reconnect cannot be implemented as written; we’ll need an alternative trigger (e.g., periodic resync or a heartbeat-ack mechanism).
- Resync semantics in the merged binary
  - No centralized resync controller is needed. The Pod watcher `Event::Init` is the resync signal.
  - On Pod `Init`, the Writer sends `pod_resync(epoch++)`. The Matcher clears its waiting_by_owner and resets emission gating for the new epoch. It then treats `InitApply` and `Apply` identically for Pods and Owners: Pods are released immediately if their Owner is available (or NoOwner), otherwise queued until the Owner arrives.
  - Do not restart kube watchers; kube-rs handles re-lists and recovery.
- Periodic watcher rotation (every 5 minutes)
  - kube-rs watcher/reflector handles RV expiry and restarts. Manual rotation is likely unnecessary and can cause avoidable load and corner cases.
- Tombstones/deleted-retention for owners (and generic Store tombstones)
  - The C++ relay retains a bounded FIFO of deleted owners to stabilize UID mapping. The Rust plan drops UID compaction, so the core reason for retaining deleted owners weakens.
  - Retaining tombstones in generic stores may be needless if they are not consulted for matching once a pod is emitted (live), and not needed for escalation.
- Owner changes after PodNew
  - The plan mirrors legacy (no new PodNew when owner changes). Confirm downstream is tolerant to owner changes without a fresh PodNew; otherwise, pods could remain attributed to stale owners.
- Version string policy
  - Uses `status.containerStatuses[*].image` (tags), not `imageID` (digest). This may be non-deterministic across pull policies. If legacy relies on this, fine; otherwise consider `imageID`.

 What We Can Simplify
- Drop periodic watcher rotation
  - Rely on kube-rs restart behavior; handle `Init`/`InitApply`/`InitDone` from the watcher/Store. Removes timers and cross-kind RV bookkeeping.
- Use kube-rs watcher/reflector stores
  - Instead of custom Store<T> with tombstones, use `kube_runtime::reflector::Store` per kind for live state. Drive matching directly from events; only maintain a minimal in-process “waiting_by_owner” and “live set”.
- Eliminate owner tombstones unless required by tests
  - If UID compaction is gone and we don’t consult deleted owners for matching, skip tombstones for OwnerStore. Keep a small cap for waiting_by_owner only.
- Narrow resync controller
  - Trigger resync on downstream reconnect (if detectable), Pod watcher `Init`, and waiting-by-owner cap. Avoid forcing watcher restarts; bump epoch and emit `pod_resync`, then re-send live pods from the Store after `InitDone`.
- Reduce emission surface
  - Consider a single `pod_new(pod, owner)` and `pod_containers(uid, containers)` without separate `pod_new_no_owner`. A `NoOwner` enum is already sufficient.

Parts Likely to Need Adjustment to Work
- Downstream reconnect signaling
  - If the Writer API does not expose reconnect callbacks, implement a simple periodic health/ack protocol or add an explicit control channel from the writer into the collector.
- Start-up ordering across kinds
  - kube-rs creates independent watchers. To preserve “owners first,” explicitly perform list-in-order RS → Jobs → Pods and only then start watchers. The plan already proposes this; it’s feasible, but keep it minimal and avoid multi-kind RV management — let watcher own RVs.
- Backpressure and flushing
  - Avoid flushing the writer after every message. Batch within an interval or use the writer’s own buffering. Blocking flushes will stall the matcher.
- Owner escalation edge cases
  - Validate behavior for adoption/orphaning (RS/Job changes ownerRef) and for controllers that set controller=false; ensure `controller=true` filter is applied.

Testability Assessment
- Overall
  - Strong test story: MockWatchSource, pure converters, deterministic matcher, test sink. Good isolation and granularity.
- Additional high-value tests
  - Dual-stack clusters: ensure IPv4/IPv6 selection matches expectations.
  - ContainerID/image changes: verify container updates trigger only `pod_containers` and the version string behavior.
  - Owner adoption/orphaning: pod first, owner late; owner deletion then re-creation; RS→Deployment and Job→CronJob escalation.
  - Watcher re-list (Init/InitApply/InitDone): ensure epoch handling doesn’t duplicate PodNew, and we don’t regress live-set invariants.
  - Large waiting queues: ensure guard rail triggers exactly once, clears state predictably, and system recovers.

Concrete Recommendations
- Minimal viable first cut
  - Use kube-rs watchers with reflector stores per kind. No periodic rotation.
  - Do explicit initial list RS → Jobs → Pods to front-load owners; then start watchers.
  - Implement Matcher with: live_set, waiting_by_owner, escalate_owner(pod, owner_meta_opt).
  - Emit PodNew when IP present and owner resolved or declared NoOwner; emit PodContainers on all live updates; emit PodDelete when deleted.
  - Implement Resync only on downstream reconnect (if signal available) or waiting cap. Do not force watcher restarts; just advance epoch and let downstream reconcile.
  - Skip owner tombstones and generic Store tombstones unless a clear need emerges in tests.
- Validate Writer integration early
  - Spike a small Rust prototype that links the render-generated Writer (crate or FFI) and confirms reconnect callbacks or some hook to drive epoch changes.
- Defer non-essential features
  - `collector_health` control messages, resource usage reporting, and periodic rotations can be deferred until parity issues are proven.

Open Questions
- Is there a Rust-native render-generated Writer/Encoder already available in this repo, or must we introduce FFI?
- Does the downstream pipeline require a watcher restart on reconnect, or is emitting `pod_resync` plus re-sending live pods sufficient? The reducer currently clears state on `pod_resync` and expects a fresh snapshot.
- Should we support `status.podIPs` selection and/or normalize container IDs (strip runtime scheme) now, or keep strict parity first?
- What TTL/capacity bounds are truly required in practice (waiting_by_owner, live sets)? Any SLOs to guide defaults?

RBAC: Go client vs kube-rs
- Capabilities
  - Both clients support in-cluster and kubeconfig-based auth, exec credential plugins, and TLS.
  - Both expose typed APIs for RBAC and Authorization (e.g., `authorization.k8s.io/v1 SelfSubjectAccessReview`).
- Needed permissions for this collector
  - cluster-wide `list`, `watch` on: core/v1 Pods; apps/v1 ReplicaSets; batch/v1 Jobs.
  - Optionally, the Authorization API for self-checks if we add them.

Protocol Requirements (Writer)
- Connection handshake:
  - Send `version_info(major, minor, patch)` uncompressed, flush, then switch on LZ4.
  - Send `connect(client_type=k8s, hostname)`, `report_cpu_cores`, config labels, cloud/node metadata, and `metadata_complete`.
- Resync:
  - Emit `pod_resync(epoch)` on downstream reconnect or Pod watcher `Init`. After `InitDone`, rebuild reducer state by sending the live pod set (with owners/containers), then resume incremental events.

Conclusion
- The plan is solid and implementable. The biggest uncertainty is downstream writer/reconnect signaling in Rust. Start by proving that integration, simplify watchers/stores leveraging kube-rs, and limit resync to essential triggers. Avoid periodic rotations and tombstones unless tests or production behavior indicate a need.
