Aggregation Core Semantics (Reducer)

Scope
- This document specifies the behavior of the aggregation core as implemented today in C++ using render-generated code. It is intended to serve as the authoritative reference for porting this functionality to Rust while minimizing reliance on generated code (ideally using generation only for message serialization).
- It covers: entities and keys, input messages, aggregation topology and propagation, time-bucketing, emitted metrics and labels, feature flags, and internal stats.

High-Level Overview
- The core ingests enriched “flow” updates (entities + metrics) into the root span agg_root and produces time-series metrics on progressively coarser projections:
  - node-node (id_id)
  - az-node (az_id and id_az)
  - az-az (az_az)
- Each projection is directional: A→B and B→A are separate streams. Aggregation windows are 30s.
- Some outputs are optionally enabled (id_id, az_id/id_az, flow logs, percentile latencies).

Entity Model and Keys
- role (role + metadata)
  - Key: (s, version, env, ns, node_type, process, container)
  - s is the role name string (spans::role::s_t = short_string<256>); it is the primary identifier for the workload name.
  - Fields written: uid (external role UID)
  - “Fields written” means non-key fields set after allocating the span by key. update_node sets role.uid via role_ref.modify().uid(role_uid). This stores a stable workload UID (e.g., Kubernetes owner UID) for labeling/correlation; it is not part of the key.
  - Source: render/ebpf_net.render:1445, release/generated/ebpf_net/aggregation/spans.h
- az (availability zone + role)
  - Key: (s, role)
  - s is the AZ name string (spans::az::s_t = short_string<32>); az is role-scoped and references the role span.
  - Source: render/ebpf_net.render:1410, release/generated/ebpf_net/aggregation/spans.h
- node (endpoint instance)
  - Key: (id, ip, az)
  - Fields written: pod_name
  - Non-key field set post-lookup. update_node writes node.pod_name via node_ref.modify().pod_name(pod_name); it does not affect key uniqueness.
  - IP can be globally disabled; when disabled, the ip field is left empty and does not participate in uniqueness beyond the blank value
  - Source: render/ebpf_net.render:1405, release/generated/ebpf_net/aggregation/spans.h

Root Span and Aggregation Tree
- Root span: agg_root holds the pair (node1, node2) and per-protocol, per-direction metric stores. It is the ingestion point for messages.
- Aggregation graph (projections), all metric stores at 30s interval:
  - agg_root.(proto)_{a_to_b,b_to_a} → node_node.(proto)_{a_to_b,b_to_a}
  - node_node.(proto)_{a_to_b,b_to_a} → az_node.(proto)_{a_to_b,b_to_a} and node_az.(proto)_{b_to_a,a_to_b} (propagates both orders)
  - “Propagate both orders” means each node_node direction fans out twice: to az_node with the same direction (az(node1)→node2), and to node_az with the reverse direction (node1→az(node2)). node_az is the same az_node container keyed as (node,az) instead of (az,node); it receives the reversed direction so that end→start semantics remain consistent. See release/generated/ebpf_net/aggregation/containers.inl for the dual updates.
  - az_node.(proto)_{a_to_b,b_to_a} → az_az.(proto)_{a_to_b,b_to_a}
- Root stores use slots 2 (double-buffer), all others slots 1.
  - Yes. agg_root MetricStores are constructed with N_EPOCHS=2 to decouple ingestion (producer slot) from emission (consumer slot). Projections use N_EPOCHS=1. The VirtualClock advances on 1s slots, but metrics emission aligns to 30s boundaries (relative_timeslot>0 gates readiness; metric timestamps are aligned to the end of the 30s slot). While the consumer slot is iterated, the producer slot continues to accept updates.
- Sources:
  - DSL: render/ebpf_net.render:1463-1549 (agg_root), 1551-1657 (node_node), 1612-1683 (az_az, az_node)
  - Runtime propagation: release/generated/ebpf_net/aggregation/containers.inl (foreach functions for agg_root, node_node, az_node, az_az)

Input Messages (ingest → aggregation)
- update_node (sets identity/labels on one side of a flow)
  - Fields: side (0=A, 1=B), id, az, role, role_uid, version, env, ns, node_type, address (ip), process, container, pod_name
  - Field meanings and how matching computes them:
    - id: primary node identifier. If an agent is present, agent_info.id; otherwise the opposite side’s remote IP string (optionally prefixed with AWS id when enriched).
    - az: availability zone. From agent_info.az when present; else from AWS enrichment or GeoIP-derived org for IP-only nodes.
    - role: workload/service name; preference order: Kubernetes pod owner → container.role → service name → process comm → “(internet)”/“(unknown)” for IP-only cases.
    - role_uid: stable workload UID (e.g., Kubernetes owner UID) when known; used for labels only.
    - version, ns, env: from Kubernetes/container/agent info; env defaults to “(no agent)” when missing.
    - node_type: resolution source enum (K8S_CONTAINER/CONTAINER/PROCESS/DNS/IP/AWS/etc.); affects labeling and role key.
    - address (ip), process (comm), container, pod_name: from socket/task/container/Kubernetes enrichment respectively.
  - Behavior:
    - Truncate each string field to span-defined max lengths; increment truncation counters per field when truncated
    - Bind role = by_key(s, version, env, ns, node_type, process, container), then set role.uid = role_uid
    - Bind az = by_key(s, role)
    - Bind node = by_key(id, ip, az); set node.pod_name = pod_name
    - Attach node to agg_root.node1/node2 based on side
    - If any by_key fails (invalid ref), drop update silently
  - Sources: reducer/aggregation/agg_root_span.cc:24-111
- update_tcp_metrics / update_udp_metrics / update_http_metrics / update_dns_metrics (adds metric deltas at agg_root)
  - Fields: direction (A_TO_B or B_TO_A), protocol-specific counters
  - Behavior: call the corresponding agg_root.<proto>_<dir>_update with that point sample
  - Sources: reducer/aggregation/agg_root_span.cc:113-178

Time Bucketing and Iteration
- Window: 30 seconds for all metric stores. Root uses two slots (producer/consumer); projections use one slot.
- Emission trigger: Core’s virtual clock advances timeslots; on_timeslot_complete() calls write_standard_metrics().
- For each protocol (tcp, udp, http, dns), WRITE_METRICS executes in this order:
  1) agg_root.a_to_b.foreach(f); set_reverse(1); agg_root.b_to_a.foreach(f); set_reverse(0)
     - The foreach propagates the current timeslot data down to node_node (no direct output at agg_root)
  2) node_node.a_to_b.foreach(f); set_reverse(1); node_node.b_to_a.foreach(f); set_reverse(0)
     - Emits id_id metrics (if enabled) and optionally flow logs
     - Also propagates to az_node and node_az
  3) az_node.a_to_b.foreach(f); set_reverse(1); az_node.b_to_a.foreach(f); set_reverse(0)
     - Emits az_id or id_az metrics depending on reverse
     - Also propagates to az_az
  4) az_az.a_to_b.foreach(f); az_az.b_to_a.foreach(f)
     - Emits az_az metrics (and feeds percentile latencies if enabled)
  - Note: there is no standalone role→role projection; role spans are used to scope AZ (az.role) and to populate labels (workload.*) on node/az projections.
- Timestamp: For each emitted sample, the timestamp aligns to the end of the corresponding 30s slot.
- Zero injection: For node_node emission only, if metrics.active_sockets > 0 in a sample, the encoder immediately schedules a zero-value update at (t + interval) to ensure a trailing zero is emitted after activity ceases, reducing handle churn and producing terminal zero samples.
  - This zero injection affects the underlying stores; when the future zero timeslot is processed it is emitted through whichever outputs are enabled (Prometheus/JSON and/or OTLP).
- Sources: reducer/aggregation/agg_core.cc:73-164, reducer/aggregation/tsdb_encoder.inl, release/generated/ebpf_net/aggregation/containers.inl

Timeslots, Clocks, and Timestamps
- Clocks and cadence
  - VirtualClock (both matching and aggregation cores) advances in 1s “clock timeslots” derived from message timestamps. When all inputs move past the current 1s slot, on_timeslot_complete() fires.
  - Protocol MetricStores (both cores) bucket at 30s. The mapping from timestamp t→slot uses a 30s divider, independent from the 1s VirtualClock.
- Matching core flush
  - On every 1s clock tick, MatchingCore::on_timeslot_complete() calls send_metrics_to_aggregation(). It computes slot_timestamp = current_timestamp() − timeslot_duration() (i.e., now − 1s) and tries to flush the finished 30s window.
  - Each per-protocol flow store is a MetricStore<..., N_EPOCHS=4> with a 30s divider. A foreach runs only when relative_timeslot(slot_timestamp) > 0, which means the “previous” 30s window has closed. This call then iterates only entries queued for that closed 30s window and advances the store by one window.
  - For each entry, matching sends an update to aggregation with t = slot_timestamp. That t is used by aggregation to place the update into the correct 30s window at the next stage (agg_root).
- Aggregation core flush
  - On every 1s clock tick, AggCore::on_timeslot_complete() calls write_metrics(). It checks readiness on agg_root’s 30s MetricStores; if relative_timeslot(now) > 0, it processes the previous 30s window.
  - The TSDB timestamp used for all outputs in that flush is aligned to the end of the 30s bucket (AggCore::write_standard_metrics aligns to the slot boundary before constructing TsdbEncoder).
  - Within a flush, the pipeline is: agg_root (propagate only) → node_node (emit + propagate) → az_node (emit + propagate) → az_az (emit + feed percentile latencies). After each foreach, the corresponding store.advance() moves to the next 30s epoch.
- Why different epoch counts
  - Matching flow stores use N_EPOCHS=4 to tolerate small delays and to prevent late-but-nearby updates from clobbering the “current” bucket; the ring makes it safe to enqueue into the correct relative bin and flush the exact closed bin.
  - Aggregation: agg_root uses N_EPOCHS=2 (producer/consumer) to decouple ingestion for the next 30s window from iteration of the previous one; downstream projections (node_node/az_node/az_az) use N_EPOCHS=1 as they are fed with the previous-slot timestamp during propagation and then immediately flushed in the same pass.
- Simplifying to single-slot (Rust port guidance)
  - For projections (node_node, az_node, az_az): already single-slot and safe. The pipeline sets their current_timeslot_ via the first update using the previous-slot timestamp, then immediately flushes them (relative_timeslot(now) > 0), and advances.
  - For agg_root: a single-slot store can work provided emission and ingestion are serialized (single-threaded core) and message handling does not interleave with write_standard_metrics(). That is already true in the current design (on_timeslot_complete runs in the event loop between RPC batches). With N_EPOCHS=1:
    - The foreach drains the queue for the finished 30s window and calls store.advance() (which increments current_timeslot_ even if queue index stays 0).
    - New updates for the next 30s window are enqueued only after the flush completes (no concurrent ingestion), so they won’t leak into the just-emitted window.
  - Caveat: if you ever parallelize ingestion with emission, or allow late-arriving updates for a closed window to be enqueued during emission, agg_root must be double-buffered to avoid mixing windows. Matching should keep ≥2 epochs (current uses 4) to tolerate minor timing skew.
- Timestamps seen downstream
  - Updates sent from matching carry t ≈ now−1s (inside the just-closed 30s window); aggregation uses that t purely to select the correct 30s bin.
  - TSDB/OTLP outputs from aggregation carry a timestamp aligned to the end boundary of the 30s window, not the raw message t.
  - Flow logs (OTLP) follow the same aligned 30s timestamp.

Emitted Metrics
- Protocol metric families and formulas (all emitted per projection, unless gated):
  - TCP (reducer/outbound_metrics.h)
    - tcp.bytes = sum_bytes
    - tcp.rtt.num_measurements = active_rtts (disabled by default)
    - tcp.active = active_sockets
    - tcp.rtt.average = (sum_srtt / 8 / 1_000_000) / active_rtts
    - tcp.packets = sum_delivered
    - tcp.retrans = sum_retrans
    - tcp.syn_timeouts = syn_timeouts
    - tcp.new_sockets = new_sockets
    - tcp.resets = tcp_resets
  - UDP
    - udp.bytes, udp.packets, udp.active, udp.drops
  - DNS
    - dns.client.duration.average = (sum_total_time_ns / 1e9) / responses
    - dns.server.duration.average = (sum_processing_time_ns / 1e9) / responses
    - dns.active_sockets, dns.responses, dns.timeouts
  - HTTP
    - http.client.duration.average = (sum_total_time_ns / 1e9) / active_sockets
    - http.server.duration.average = (sum_processing_time_ns / 1e9) / active_sockets
    - http.active_sockets
    - http.status_code with label status_code ∈ {"200","400","500","other"}
- Projection names and gating:
  - id_id (node_node): emitted only if enable_id_id = true
  - az_id and id_az (az_node): emitted only if enable_az_id = true
  - az_az: always emitted (unless the output channel is disabled)
- Percentile latencies (optional): computed over az_az only and exported via Prometheus/JSON (not OTLP)
  - Metrics: <proto>_latency_p90, <proto>_latency_p95, <proto>_latency_p99, <proto>_latency_max where proto ∈ {tcp, dns, http}
  - Window: ~5 minutes (queue size 30, step 10s); each add() feeds a T-Digest per key, producing rolling quantiles and max
  - Label key: FlowLabels on (source.az, dest.az)
  - Sources: reducer/aggregation/percentile_latencies.* and reducer/latency_accumulator.*
- Flow logs (optional): emitted for node_node when enable_flow_logs = true and OTLP is enabled
  - Currently implemented for TCP only (UDP/DNS/HTTP no-ops)
  - Includes a subset of TCP counters (bytes, rtt, active, retrans, syn_timeouts, new_sockets, resets) and labels
  - Source: reducer/write_metrics.h (write_flow_log), reducer/otlp_grpc_formatter.cc (publish_flow_log)

Labels
- NodeLabels fields (per side): id, ip, az, role, role_uid, version, env, ns, type (NodeResolutionType), process, container, pod
  - Derived from the weak refs: Node → Az → Role
  - The NodeResolutionType enum is stringified for label ‘type’
  - If node_ip_field is disabled, the ip label is empty and not emitted
- FlowLabels = {source.<NodeLabels>, dest.<NodeLabels>}
  - Adds az_equal="true|false" when both az labels are non-empty
  - Sources: reducer/aggregation/labels.*
- All outputs append sf_product="network-explorer" label
- For HTTP status metrics, a label status_code is added and removed per emission
- Label/metric name sanitization:
  - Prometheus: dots ‘.’ in metric and label names are replaced with ‘_’
  - JSON/OTLP: dots are preserved as-is
  - Sources: reducer/prometheus_formatter.cc, reducer/json_formatter.cc, reducer/otlp_grpc_formatter.cc

Output Channels and Writer Selection
- Prometheus/JSON (scrape-style) metrics: metric_writers_ vector; disabled if empty
  - Writer sharding: for projections with a span ref, choose writer by span.loc() % metric_writers_.size() to spread load
    - node_node: writer_num = span.loc() % N
    - az_node: writer_num = span.loc() % N
    - az_az: writer_num = span.loc() % N
  - Percentile latency metrics currently use metric_writers_[0] (TODO in code to shard by labels)
- OTLP gRPC (push-style) metrics: single otlp_metric_writer_; disabled if null
  - Metric descriptions can be enabled; otherwise an empty description is used
  - Flow logs are OTLP-only (metrics and logs use the same writer class)
  - Labels for OTLP: adds an "aggregation" label and the union of source.* and dest.* NodeLabels: workload.name, workload.uid, availability_zone, id, ip, resolution_type, image_version, environment, namespace.name, process.name, container.name, pod. Dots are preserved for OTLP; Prometheus replaces '.' with '_'; JSON preserves dots.
- Rollup and timestamp are attached per batch; timestamp is aligned to the end of the 30s slot
- Sources: reducer/aggregation/tsdb_encoder.* and reducer/tsdb_formatter.*

OTLP Metric Types and Descriptions
- All OTLP metric kinds are determined by MetricInfo.type and mapped in OtlpGrpcFormatter to Sum or Gauge.
  - Mapping site: reducer/otlp_grpc_formatter.cc:36 (kind from MetricInfo.type)
- Canonical names, types, units, and descriptions are defined in:
  - reducer/metric_info.h:1 (MetricInfo, MetricType)
  - reducer/metric_info.cc:1 (per-metric info)
- Summary of types by protocol:
  - TCP
    - Sum: tcp.bytes, tcp.packets, tcp.retrans, tcp.syn_timeouts, tcp.new_sockets, tcp.resets
    - Gauge: tcp.rtt_num_measurements, tcp.active, tcp.rtt_average
  - UDP
    - Sum: udp.bytes, udp.packets, udp.drops
    - Gauge: udp.active
  - DNS
    - Sum: dns.responses, dns.timeouts
    - Gauge: dns.active_sockets, dns.client.duration.average, dns.server.duration.average
  - HTTP
    - Sum: http.status_code (with label status_code in {200,400,500,other})
    - Gauge: http.active_sockets, http.client.duration.average, http.server.duration.average
  - Exact text and units in reducer/metric_info.cc:1

Feature Flags and Configuration
- enable_id_id: emit id_id (node-node) metrics
- enable_az_id: emit az_id and id_az (az-node) metrics
- enable_flow_logs: emit flow logs (TCP only) on node-node projection via OTLP
- disable_node_ip_field: omit ip from Node and labels; node keys include blank ip
- enable_percentile_latencies: enable T-Digest accumulators and export p90/p95/p99/max at az_az
- disable_metrics / enable_metrics: per-group and per-metric controls for tcp/udp/dns/http/ebpf_net
  - Default disables tcp.rtt.num_measurements and all ebpf_net internal metrics except a curated set
- scrape_metrics_tsdb_format: Prometheus or JSON; OTLP metrics/logs are independent
- Sources: reducer/reducer_config.h, reducer/disabled_metrics.*

Internal Statistics
- Emitted to logging core via core_stats and agg_core_stats proxy spans
- Truncation counters (per field) for update_node
  - Fields: az, container, env, id, ip, ns, pod_name, process, role, version, role_uid
  - Emitted periodically with module="aggregation" and shard id
- Writer bytes written/failed per Prometheus-style writer
- Code timing metrics (when enabled) are emitted as delta temporal metrics and reset each interval
  - Code timing metrics measure per-site execution time (count/avg/min/max/sum) for instrumented regions (e.g., AggCoreWriteStandardMetrics). Controlled by ENABLE_CODE_TIMING and reset after each interval.
- Sources: reducer/aggregation/agg_core.cc:166-227, reducer/aggregation/stat_counters.h, release/generated/ebpf_net/aggregation/weak_refs.h

Directional Semantics
- All projections are directional; A→B and B→A are emitted separately.
  - There is exactly one agg_root per pair: matching creates agg_root using a canonical ordering of (role1,az1,role2,az2) by sorting (role, az) pairs, so you never get both X→Y and Y→X roots for the same endpoints. Within that single root, both A_TO_B and B_TO_A stores exist and are updated by the sending side.
  - node_node spans are keyed (node1,node2) as assigned by update_node (side A vs side B). A→B and B→A are separate stores on the same span, and TsdbEncoder.reverse swaps label order when emitting. There is no duplicate reversed node_node span created by another root.
- Encoder reverse flag:
  - node_node: reverse toggled to swap label order for A→B vs B→A
  - az_node: reverse selects the projection name:
    - reverse=0 → aggregation="az_id" (source.az → dest.node)
    - reverse=1 → aggregation="id_az" (source.node → dest.az)
  - az_az: direction-specific stores exist; encoder does not swap labels

Zero Injection Policy
- Scope: only applies to node_node projection for all protocols (tcp/udp/http/dns), per direction.
- Trigger: if the aggregated metrics for a node_node timeslot had activity (metrics.active_sockets > 0), the encoder schedules a zero-valued update for the next 30s slot at timestamp (t + interval).
  - Implementation: reducer/aggregation/tsdb_encoder.inl:1 (node_node operator) sets zero_metrics = {} and calls span.A_B_UPDATE or B_A_UPDATE with t + interval when metrics.active_sockets > 0.
  - Effect: ensures exactly one trailing zero timeslot after activity ceases, and keeps handles alive one more interval to reduce churn.
- Not per-metric: the single zero point covers the entire metric family for that label set (all metrics in that family will be zero for that next slot).
- Outputs: zeros are produced by the underlying stores and will be emitted by all enabled outputs (Prometheus/JSON and OTLP). This is not Prometheus-only.
- Projections az_node and az_az do not inject zeros; they will reflect whatever propagated values exist for the slot.

Invariants and Edge Cases
- If required references cannot be allocated (pool exhaustion), update_node drops the update
- node.pod_name is not part of the node key: assumption is “one pod per node”; otherwise the key schema would need to include pod_name
- Zero injection only happens for node_node when there was activity (active_sockets > 0)
- Disabling node IP changes the node key cardinality (IP becomes empty); ensure id is globally unique in practice
- Percentile latency export is Prometheus/JSON only; disabled_metrics does not affect pXX outputs (they are not part of the outbound metric enums)

Relevant Source Files (non-exhaustive)
- reducer/aggregation/agg_core.h
- reducer/aggregation/agg_core.cc
- reducer/aggregation/agg_root_span.h
- reducer/aggregation/agg_root_span.cc
- reducer/aggregation/tsdb_encoder.h
- reducer/aggregation/tsdb_encoder.inl
- reducer/aggregation/tsdb_encoder.cc
- reducer/aggregation/labels.h
- reducer/aggregation/labels.inl
- reducer/aggregation/percentile_latencies.h
- reducer/aggregation/percentile_latencies.cc
- reducer/aggregation/stat_counters.h
- reducer/write_metrics.h
- reducer/tsdb_formatter.h
- reducer/prometheus_formatter.h, reducer/prometheus_formatter.cc
- reducer/json_formatter.h
- reducer/otlp_grpc_formatter.h, reducer/otlp_grpc_formatter.cc
- reducer/disabled_metrics.h, reducer/disabled_metrics.cc
- render/ebpf_net.render (app aggregation)
- release/generated/ebpf_net/aggregation/spans.h
- release/generated/ebpf_net/aggregation/containers.inl
- release/generated/ebpf_net/aggregation/weak_refs.h
- release/generated/ebpf_net/aggregation/index.h

Porting Notes (Rust)
- Message ingestion/parsing can continue to rely on the render-generated serialization (update_node and update_*_metrics messages).
- Reimplement the aggregation graph as explicit Rust structs and maps keyed exactly as above; preserve per-protocol, per-direction stores bucketed at 30s.
- Ensure foreach iteration order and slot advancement semantics match containers.inl behavior; emit and propagate in the same order to avoid behavior regressions.
- Reproduce writer sharding and reverse labeling semantics; maintain aggregation names id_id, az_id, id_az, az_az.
- Preserve zero injection on node_node after activity, and percentile latency accumulation on az_az when enabled.
- Apply DisabledMetrics filtering only at output time; propagation must be unaffected by disabled metrics.
