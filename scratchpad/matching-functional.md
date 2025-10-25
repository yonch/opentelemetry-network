Title: Matching Core — Functional Specification

Purpose

- Provide a clear, outcome-focused description of what the Matching Core accomplishes in the data path between ingest and aggregation.
- Define externally observable behaviors, inputs, outputs, and guarantees without prescribing internal data structures or C++/Rust implementation details.

Outcome Overview

- Transforms raw network and protocol telemetry into canonical, enriched node identities for both sides of a flow.
- Produces time-sliced, deduplicated protocol metrics for canonical node-pair keys and forwards them to aggregation.
- Maintains consistent orientation and sharding of node pairs to ensure stable aggregation across shards and restarts.
- Exposes internal health and diagnostic signals for operability and troubleshooting.

Inputs (by semantics, not wire format)

- Flow lifecycle: notifications that a bidirectional flow starts/ends and the participating 5-tuples (addresses/ports).
- Host/process identity: agent details, environment, namespace, and process command (e.g., comm, cgroup name).
- Socket context: local/remote addresses and ports, client/server indication, and optional remote DNS name.
- Kubernetes identity: pod and container identity, names, owner metadata, image/version, and namespace.
- Service labels: optional service name for grouping.
- Protocol stats: time-sliced aggregates for TCP, UDP, HTTP, and DNS (including counts, bytes, errors, and latencies).
- Cloud enrichment: optional AWS role/availability-zone/instance-id keyed by IP; enable/disable is configurable.

Primary Outputs

- Aggregation updates
  - Node attachments: for each side of a canonical node pair, emit identity updates containing id, role, version, env, namespace, availability zone, node type, address, process command, container name, pod name, and role UID when known.
  - Protocol metrics: per-timeslot, per-direction aggregates for TCP, UDP, HTTP, and DNS associated with the canonical node pair.
- Observability signals
  - Core stats: span/utilization, queue latencies, heartbeats.
  - Diagnostics: targeted logs for missing/invalid metadata and notable resolution or mapping events.

Core Responsibilities

- Identity resolution
  - For each flow side, derive a single node identity using precedence: Kubernetes > Container > Process > Cloud role > DNS name > IP address.
  - Populate node attributes (id, role, version, env, namespace, az, node type, address, names, process command) from the best available evidence.
  - Degrade gracefully: if higher-fidelity metadata is missing, fall back to lower precedence without failing the flow.

- Canonical pairing and sharding
  - Combine two resolved nodes into a canonical pair key that is consistent across shards and restarts.
  - Orientation rule: order the pair lexicographically by role (and, when applicable, availability zone) so both directions of the same relationship coalesce under one key.
  - Sharding rule: include availability zone in the key only when at least one side is an IP-classified node; otherwise shard by roles alone to avoid over-fragmentation.

- Metric aggregation and flush
  - Ingest time-sliced protocol updates and attribute them to one canonical pair per flow.
  - Deduplicate by side: select exactly one emitting side for volume-style counts for the lifetime of a flow to prevent double counting when both agents report.
  - Direction semantics: preserve A→B vs B→A direction on metrics; both directions may be present for the same pair within a slot.
  - Latency placement: when both sides report for HTTP/DNS, record total latency on the client and processing time on the server to reflect user-perceived and server-side views.
  - Flush cadence: emit completed-slot aggregates on timeslot boundaries based on the system’s virtual clock; only flush data that is complete for the previous slot.

- Cloud and network classifications
  - AWS enrichment: when enabled and available, annotate nodes by role, az, and instance id, keyed by IP.
  - Autonomous System (AS): when a remote IP maps to an AS organization, record it in the node’s az/organization field; optionally collapse address and id to the literal "AS" when the AS-collapse toggle is disabled.
  - Instance metadata: recognize link-local instance metadata addresses and classify them explicitly; inherit az/id from the peer when appropriate.

- Operability and diagnostics
  - Emit periodic core stats and queue latencies for capacity and health monitoring.
  - Produce targeted diagnostics (e.g., unresolved Kubernetes container-to-pod relationships) to aid troubleshooting.
  - Heartbeat the internal index/runtime to signal liveness.

Behavioral Guarantees

- No double counting across sides for volume metrics within a flow: exactly one side contributes counts for the flow’s lifetime.
- Stable canonical keys: the same pair of roles (and az when applicable) always collapses to the same orientation, regardless of ingest order.
- Graceful degradation: flows still contribute metrics with DNS/IP identity when richer metadata is missing.
- Slot integrity: only complete timeslot data is emitted; partial data waits until the slot closes.

Special Cases

- DNS flows: when remote port 53 or DNS context is detected, default server identity derives from the DNS side; the latency-placement rules above still apply.
- Instance metadata: link-local instance metadata traffic is labeled with a distinct node type and does not assume generic IP behavior; peer-derived az/id may be used for context.
- Autonomous System handling: AS classification augments or, when configured, replaces granular IP identity to support privacy- or aggregation-oriented deployments.

Configuration and Toggles

- AWS enrichment enable/disable: governs whether cloud role/az/id are attached from the enrichment stream.
- Autonomous system collapse: when disabled, collapse IP identities that map to an AS to a generic "AS" node; when enabled, retain the more specific IP/AS details.
- Optional GeoIP database: when present, enables AS organization lookups; when absent, AS features are skipped without error.

Lifecycle and Timing

- Flow start: initialize per-flow state from the first arriving messages; identity resolution can evolve as richer metadata arrives.
- Flow updates: accumulate protocol stats and refine node identity as new evidence appears; the emitting side for counts remains fixed once chosen.
- Timeslot close: on each slot boundary, flush aggregates for flows with completed data for the previous slot; attach/refresh node identity alongside metrics as needed.
- Flow end: allow final flush; remaining partial slot data is emitted only if the slot closes.

External Contracts (observable without implementation details)

- Aggregation receives
  - update_node for each side of a canonical pair when identity is new or changed.
  - update_tcp/udp/http/dns_metrics per timeslot and direction, keyed by the canonical pair.
- Logging receives
  - Core stats snapshots, queue/latency metrics, heartbeats.
  - Diagnostics for notable resolution or mapping failures (e.g., Kubernetes container without resolvable pod).

Non-Goals

- Defining generated runtime types, span handles, or container iteration mechanics.
- Specifying internal data structures for caches or indices.
- Describing aggregation-core storage or query semantics beyond the contract above.

Example Acceptance Scenarios

- Two Kubernetes services communicate; both sides report. Aggregation shows a single canonical pair keyed by their roles, bidirectional TCP bytes, and HTTP metrics with client total latency and server processing time.
- A pod talks to an external IP that maps to an AS. With AS collapse disabled, aggregation includes the AS organization label; with collapse enabled, the node appears as a generic "AS" with volume metrics preserved.
- An agent emits socket/protocol stats before Kubernetes metadata arrives. Early slots attribute to DNS/IP; later slots seamlessly upgrade identity to container/pod without duplicating counts.
- Calls to the instance metadata endpoint are labeled as such and do not pollute generic IP buckets; identity inherits available az/id context from the peer’s agent.

