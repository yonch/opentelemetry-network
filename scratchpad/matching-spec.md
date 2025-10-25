Title: Matching Core (C++) — Behavioral Specification for Rust Port

Scope

- This document specifies the behavior of the reducer/matching C++ implementation to guide a functionally equivalent Rust implementation.
- Covered components are limited to reducer/matching:
  - MatchingCore
  - FlowSpan
  - K8sPodSpan
  - K8sContainerSpan
  - AwsEnrichmentSpan
- External, generated types (Index, Protocol, Connection, TransformBuilder, containers, spans, keys) are treated as APIs and referenced by behavior rather than implementation.

Key Dependencies and Concepts

- Generated matching runtime:
  - Index: ebpf_net::matching::Index (span storage and lookups)
  - Protocol: ebpf_net::matching::Protocol (message decoder/dispatcher)
  - Connection: ebpf_net::matching::Connection (per-connection stats)
  - TransformBuilder: ebpf_net::matching::TransformBuilder (JIT transforms)
  - Spans: ebpf_net::matching::spans::{flow, agg_root, k8s_pod, k8s_container, aws_enrichment, logger}
  - Containers: ebpf_net::matching::containers::flow (per-flow metric stores and foreach iteration)
- Common enums and constants (reducer/constants.h):
  - NodeResolutionType: NONE, IP, DNS, AWS, INSTANCE_METADATA, PROCESS, LOCALHOST, K8S_CONTAINER, CONTAINER, NOMAD
  - FlowSide: SIDE_A, SIDE_B. Operators: +side (index cast), ~side (other side).
  - UpdateDirection: NONE, A_TO_B, B_TO_A
  - Strings: kUnknown="(unknown)", kCommKubelet="kubelet", kNoAgentEnvironmentName="(no agent)"
  - kPortDNS=53
- GeoIP database (geoip::database): optional; when present builds AS org strings from IPs.
- Timeslot/virtual clock (Core): per-message timestamping drives timeslot advancement; matching flushes work on timeslot boundaries.

High-Level Responsibilities

- Ingests flow- and enrichment-related messages from ingest cores.
- Resolves node identities for each flow side (K8s > Container > Process > AWS > DNS > IP), with special-case overrides.
- Creates/maintains agg_root handles keyed by role/az pairs (with ordering) and attaches both nodes.
- Tracks and emits per-protocol, per-direction metric updates to aggregation on timeslot ticks.
- Exposes internal metrics and logs for observability.

MatchingCore

- Construction
  - Parameters: ingest→matching queues, matching→aggregation queues, matching→logging queues, optional GeoIP DB path, shard_num, initial_timestamp.
  - Initializes:
    - CoreBase with matching→aggregation and matching→logging writers and app_name="matching".
    - GeoIP database (an_db): no-op DB if path not provided.
    - Rpc stats trackers for ingest→matching, matching→aggregation, matching→logging.
    - auto_handles::core_stats and auto_handles::logger for internal metrics/logging.
    - Adds RPC clients for each ingest queue.
- Static toggles
  - enable_aws_enrichment(bool): forwards to FlowSpan::enable_aws_enrichment.
  - set_autonomous_system_ip_enabled(bool), autonomous_system_ip_enabled(): controls whether AS-classified IP nodes collapse address/id to "AS".
- Logger accessor
  - logger(): returns a weak_refs::logger handle for child spans (e.g., K8sContainerSpan) to emit structured logs to logging core.
- on_timeslot_complete()
  - send_metrics_to_aggregation() (flush previous timeslot’s metrics; see FlowSpan send logic below)
  - matching_to_aggregation_stats_.check_utilization() and matching_to_logging_stats_.check_utilization()
  - index_.send_pulse() (heartbeat/pulse to generated runtime)
- send_metrics_to_aggregation()
  - slot_timestamp = current_timestamp() − timeslot_duration().
  - If index_.flow.tcp_a_to_b_ready(slot_timestamp) is true, delegate to FlowSpan::send_metrics_to_aggregation(index_.flow, slot_timestamp).
    - Note: readiness gating lives in generated containers; the call flushes finished slot data.
- write_internal_stats()
  - Emits common span utilization and connection message stats (CoreBase::write_common_stats_to_logging_core) and per-RPC-lane latencies to logging core.
  - Periodically dumps internal index state (IndexDumper) for diagnostics.

FlowSpan

- Role: Enriches flows with node identity and relays metrics. Implements message handlers for per-side state accumulation and supplies derived NodeData to agg_root during metric flushes.

- Per-side state (arrays indexed by +FlowSide)
  - AgentInfo { id, az, env, role, ns }
  - TaskInfo { comm, cgroup_name }
  - SocketInfo { local_addr:IPv6, local_port:u16, remote_addr:IPv6, remote_port:u16, is_connector:u8, remote_dns_name:string }
  - K8sInfo { pod_uid_suffix:[u8;64], pod_uid_hash:u64 }
  - ContainerInfo { name, pod, role, version, ns, type:NodeResolutionType (default CONTAINER; sanitized) }
  - ServiceInfo { name }
  - metrics_update_side_: Option<FlowSide>; chosen on first metrics update to avoid double-counting across sides for the entire flow life.
  - n_received_info_messages_: u32; incremented on each info message.
  - message_count_on_last_update_: u32; last count when update_nodes() ran (initially 0xFFFFFFFF to force first update on first flush).
  - aws_enrichment_enabled_: static bool.

- Derived node representation
  - NodeData { id, az, role, role_uid, version, env, ns, node_type, address, comm, container_name, pod_name }
  - Address is the printable IP string of the chosen address for the side; comm derived from TaskInfo.

- Info message handlers (mutate per-side state and bump n_received_info_messages_)
  - agent_info(flow, t, msg): fills AgentInfo; logs trace.
  - task_info(flow, t, msg): fills TaskInfo; logs trace.
  - socket_info(flow, t, msg): fills SocketInfo; logs trace; stores remote_dns_name.
  - k8s_info(flow, t, msg): fills K8sInfo from {pod_uid_suffix[64], pod_uid_hash}.
  - container_info(flow, t, msg): fills ContainerInfo and sanitizes msg->node_type to valid NodeResolutionType or falls back to CONTAINER.
  - service_info(flow, t, msg): fills ServiceInfo.name.

- Node resolution and updates
  - update_nodes_if_required(flow):
    - Determine need_to_update if any new info messages arrived since last update OR if should_attempt_k8s_enrichment(side) for either side returns true.
    - If update needed: resolve_node(flow, SIDE_A) and resolve_node(flow, SIDE_B), then update_nodes(flow, a, b), then message_count_on_last_update_ = n_received_info_messages_.
  - should_attempt_k8s_enrichment(flow, side):
    - Returns true if the flow does not yet have k8s_pod handle set for that side and K8sInfo for that side exists.
  - update_nodes(flow, node_a, node_b):
    - create_agg_root(flow, node_a, node_b) if needed.
    - If flow.agg_root() is valid, call update_node(agg_root, side, node) for both sides.
  - update_node(agg_root, side, n): emits agg_root.update_node with fields:
    - side:u8, id, az, role, version, env, ns, node_type:u8, address, comm, container_name, pod_name, role_uid.

- Agg root creation and sharding
  - create_agg_root(flow, node_a, node_b):
    - If either node’s role is empty, do nothing (insufficient identity).
    - Determine sharding key fields role1/role2 and optionally az1/az2:
      - HACK: include AZs only if at least one node is NodeResolutionType::IP; otherwise AZs are not used for sharding.
      - Order normalization: compute (role_a, az_a) and (role_b, az_b). If tie-break says (a,az_a) <= (b,az_b), set role1=a, role2=b, az1=az_a, az2=az_b; else swap.
    - If flow has no agg_root or keys changed, allocate a new agg_root handle: index.agg_root.alloc(role1, az1, role2, az2) and attach to flow.

- Node resolution algorithm (resolve_node)
  - Inputs: span_ref (flow), side.
  - Precondition: an address/port must be available from either this side’s SocketInfo (local) or the other side’s SocketInfo (remote). If neither present: log debug_state("address/port missing") and return default NodeData (empty identity) — upstream callers will skip updates when agg_root invalid.
  - Compute textual IP address: chosen from get_addr_port(side).addr.tidy_string().
  - Compute (id, az, is_autonomous_system):
    - If AgentInfo exists on this side: id=agent.id, az=agent.az, is_autonomous_system=false.
    - Else: use the other side’s SocketInfo.remote_addr for id; if GeoIP DB is present, look up that IPv6 and set az to the mmdb field "autonomous_system_organization" if available; is_autonomous_system = true on successful AS lookup, else false.
  - env: AgentInfo.env if present; else "(no agent)".
  - ns: AgentInfo.ns if present; else retain or use container.ns if later.
  - pod_name: from ContainerInfo.pod (may be empty).
  - Sanity checks (logging only): agent_info presence should match task_info and socket_info; log debug_state if mismatched.
  - Enrichment precedence (stop at first that yields identity):
    1) Kubernetes (get_k8s_pod):
       - If K8sInfo exists, build k8s_pod key from pod_uid_suffix[64] and pod_uid_hash; look up index.k8s_pod.by_key; require pod.valid() and pod.owner_name() non-empty.
       - Else, if TaskInfo exists, parse cgroup_name with CGroupParser to extract container_id; build k8s_container key (UID key scheme) and look up index.k8s_container.by_key; if valid and has pod, use its pod().
       - On success: attach pod to flow via flow.modify().k8s_pod1/2(pod) for the side.
       - Set node_type=K8S_CONTAINER; role=pod.owner_name(); role_uid=pod.owner_uid(); ns=pod.ns(); version=pod.version();
       - If TaskInfo exists and container lookup by cgroup container_id succeeds, set container_name=k8s_container.name(); override version if k8s_container.version() non-empty.
    2) ContainerInfo (direct): node_type=sanitized(container.type) else CONTAINER; role=container.role; version=container.version; ns=container.ns.
    3) Process (Agent present): node_type=PROCESS; role is ServiceInfo.name if present, else TaskInfo.comm if present, else AgentInfo.role; ns=AgentInfo.ns.
    4) No agent on this side, use flip side SocketInfo (remote):
       - AWS enrichment (if enabled): lookup span index.aws_enrichment.by_key({remote_ipv6.as_int()}, allow_create=false); if found and .impl().info() has role and az non-empty, set node_type=AWS; role=aws.role; az=aws.az; if aws.id is non-empty, prefix id with "aws.id/".
       - Else DNS: if remote_dns_name non-empty, node_type=DNS; role=remote_dns_name.
       - Else IP: node_type=IP; role="(internet)" if is_autonomous_system else "(unknown)".
  - Special-case overrides after base resolution:
    - If get_comm(side) == "kubelet" → role="kubelet" (node_type unchanged).
    - Else if addr_port->port == 53 → role="DNS" (node_type unchanged).
    - Else if address == IPv6-mapped 169.254.169.254 (instance metadata) → role="instance metadata"; node_type=INSTANCE_METADATA; if the other side has AgentInfo, override id and az with the other side’s agent.id and agent.az.
  - Autonomous system policy: if is_autonomous_system && node_type==IP && !MatchingCore::autonomous_system_ip_enabled(): set id = "AS" and address = "AS".
  - If container_name is still empty, default to ContainerInfo.name.
  - Return NodeData with: id, az, role, role_uid, version, env, ns, node_type, address, comm=get_comm(side), container_name, pod_name.

- Helper getters
  - get_comm(side): TaskInfo.comm or empty.
  - get_addr_port(side): Prefer this side’s SocketInfo.local_{addr,port}; else other side’s SocketInfo.remote_{addr,port}; else None.
  - get_id_az(side): if AgentInfo exists, (agent.id, agent.az, false); else derive from flip side SocketInfo.remote_addr and GeoIP (see above) returning (ip_string, az, is_autonomous_system).
  - get_k8s_pod(side, index): try by K8sInfo key; else by parsed cgroup container_id via k8s_container→pod; require valid with identity; returns auto_handle or invalid handle if not found.

- Metrics ingestion (per message)
  - A per-flow, single side is chosen lazily to send full metrics: metrics_update_side_ is set upon the first metrics message observed for the flow; the other side returns UpdateDirection::NONE for full metrics to avoid double counting. This applies across protocols for the life of the flow.
  - UpdateDirection mapping:
    - If side==SIDE_A: is_rx==0 → A_TO_B; is_rx==1 → B_TO_A.
    - If side==SIDE_B: is_rx==0 → B_TO_A; is_rx==1 → A_TO_B.
    - If force_both_sides=true (used to bypass gating for non-duplicated subfields), mapping is computed without side gating.
  - tcp_update(flow, t, msg):
    - Copy msg into ::ebpf_net::metrics::tcp_metrics_point.
    - If msg.is_rx is true, zero out RTT fields (sum_srtt, active_rtts) to avoid duplicating RX RTTs.
    - Emit full metrics to span_ref.tcp_{dir}_update for UpdateDirection dir if dir!=NONE.
    - Additionally, when not skipping RTTs (i.e., on TX): send an RTT-only point (sum_srtt, active_rtts) to the opposite direction; if dir==NONE, send RTT-only to both directions.
  - udp_update(flow, t, msg): copy and emit ::ebpf_net::metrics::udp_metrics_point to the chosen direction if dir!=NONE.
  - http_update(flow, t, msg):
    - Interpret is_rx by client_server flag (SC_SERVER==rx; SC_CLIENT==tx).
    - Copy msg into ::ebpf_net::metrics::http_metrics_point, then enforce side-correctness: client never gets processing time (zero sum_processing_time_ns); server never gets total time (zero sum_total_time_ns).
    - If dir==NONE (both sides present): reduce to a single-sided point containing only non-duplicated latency aggregates (sum_total_time_ns or sum_processing_time_ns), and recompute dir with force_both_sides=true to emit that reduced point on the correct side.
  - dns_update(flow, t, msg): identical duplicate-handling pattern to http_update, acting on ::ebpf_net::metrics::dns_metrics_point and fields sum_total_time_ns vs sum_processing_time_ns.

- Metrics flush to aggregation (timeslot-driven)
  - FlowSpan::send_metrics_to_aggregation(flows, ts):
    - For each protocol/direction pair, use generated foreach to iterate entries for timestamp ts:
      - tcp_a_to_b_foreach(ts, send_tcp_metrics<A_TO_B>), tcp_b_to_a_foreach(ts, send_tcp_metrics<B_TO_A>)
      - udp_a_to_b_foreach, udp_b_to_a_foreach
      - dns_a_to_b_foreach, dns_b_to_a_foreach
      - http_a_to_b_foreach, http_b_to_a_foreach
    - Each send_* helper does:
      1) span_ref.impl().update_nodes_if_required(span_ref)
      2) If span_ref.agg_root invalid, return (skip)
      3) agg_root.update_{proto}_metrics((u8)Dir, …aggregates from the store…)
    - Note: Node updates occur here, just before metrics are forwarded, so that aggregation observes up-to-date identities.

Kubernetes Spans

- K8sPodSpan
  - set_pod_detail(pod, t, msg): populate owner_name, owner_uid, pod_name, ns, version using short-string truncation rules from generated modifiers; used by FlowSpan during K8s resolution.

- K8sContainerSpan
  - set_container_pod(container, t, msg):
    - Build k8s_pod key from msg.pod_uid_hash and msg.pod_uid_suffix[64], look up index.k8s_pod.by_key().
    - If not found: log trace and emit logger().k8s_container_pod_not_found(pod_uid_suffix, pod_uid_hash) via MatchingCore::logger(); return.
    - Derive name and version: name is truncated msg->name; version extracted from msg->image (DockerImageMetadata(msg->image).version()).
    - Set container.pod(k8s_pod), .name(name), .version(version).

AWS Enrichment Span

- AwsEnrichmentSpan maintains std::optional<AwsEnrichmentInfo> info_ per span.
- aws_enrichment(span, t, msg): assign msg.role, msg.az, msg.id into info_ (create if missing). Logs at NodeResolutionType::AWS with ClientType::cloud.
- FlowSpan consults this span (by key: remote IPv6 address as u128) only when aws_enrichment is enabled and this side lacks AgentInfo:
  - If info exists and role+az non-empty: adopt node_type=AWS, role=info.role, az=info.az, id possibly prefixed by info.id.

Special Cases and Policies

- Instance metadata address: IPv6 0000:…:ffff:169.254.169.254 is recognized. Node is labeled "instance metadata" with node_type=INSTANCE_METADATA; id/az inherited from the other side’s AgentInfo if available.
- Autonomous System handling: when GeoIP identifies the remote IP’s AS organization, set is_autonomous_system=true and az to that org string. If MatchingCore::autonomous_system_ip_enabled() is false and node_type==IP, collapse both id and address to the literal "AS".
- Duplicate metrics avoidance: one side per flow emits full protocol metrics. HTTP and DNS still emit single-sided latency aggregates to place totals on the client and processing on the server when both sides report.

Internal Metrics and Logging

- MatchingCore periodically writes:
  - Span utilization and connection message stats via logger core span.
  - RPC latencies per queue (ingest→matching, matching→aggregation, matching→logging).
  - A heartbeat/pulse and index dump snapshots at a controlled cadence.
- FlowSpan::debug_state(reason): detailed dump of cached per-side info, gated under NodeResolutionType::NONE in log severity routing. Used for diagnosis when preconditions for resolution are violated.

Inputs and Outputs (contract)

- Inputs from ingest (complete list of fields; names follow jsrv_matching__* messages as used). Types mirror `crates/render/ebpf_net/matching/src/parsed_message.rs`:
  - flow_start:
    - addr1: IPv6 (u128)
    - port1: u16
    - addr2: IPv6 (u128)
    - port2: u16
  - flow_end: —
  - agent_info:
    - side: u8
    - id: string
    - az: string
    - env: string
    - role: string
    - ns: string
  - task_info:
    - side: u8
    - comm: string
    - cgroup_name: string
  - socket_info:
    - side: u8
    - local_addr: IPv6 (16B)
    - local_port: u16
    - remote_addr: IPv6 (16B)
    - remote_port: u16
    - is_connector: u8 (boolean semantics)
    - remote_dns_name: string
  - k8s_info:
    - side: u8
    - pod_uid_suffix: [u8; 64]
    - pod_uid_hash: u64
  - tcp_update:
    - side: u8
    - is_rx: u8
    - active_sockets: u32
    - sum_retrans: u32
    - sum_bytes: u64
    - sum_srtt: u64
    - sum_delivered: u64
    - active_rtts: u32
    - syn_timeouts: u32
    - new_sockets: u32
    - tcp_resets: u32
  - udp_update:
    - side: u8
    - is_rx: u8
    - active_sockets: u32
    - addr_changes: u32
    - packets: u32
    - bytes: u64
    - drops: u32
  - http_update:
    - side: u8
    - client_server: u8 (client=0/server=1)
    - active_sockets: u32
    - sum_code_200: u32
    - sum_code_400: u32
    - sum_code_500: u32
    - sum_code_other: u32
    - sum_total_time_ns: u64
    - sum_processing_time_ns: u64
  - dns_update:
    - side: u8
    - client_server: u8 (client=0/server=1)
    - active_sockets: u32
    - requests_a: u32
    - requests_aaaa: u32
    - responses: u32
    - timeouts: u32
    - sum_total_time_ns: u64
    - sum_processing_time_ns: u64
  - container_info:
    - side: u8
    - name: string
    - pod: string
    - role: string
    - version: string
    - ns: string
    - node_type: NodeResolutionType (u8; sanitized, default CONTAINER)
  - service_info:
    - side: u8
    - name: string
  - aws_enrichment_start:
    - ip: IPv6 (u128)
  - aws_enrichment_end: —
  - aws_enrichment (span keyed by IPv6 address):
    - role: string
    - az: string
    - id: string
  - k8s_pod_start:
    - uid_suffix: [u8; 64]
    - uid_hash: u64
  - k8s_pod_end: —
  - set_pod_detail:
    - owner_name: string
    - pod_name: string
    - ns: string
    - version: string
    - owner_uid: string
  - k8s_container_start:
    - uid_suffix: [u8; 64]
    - uid_hash: u64
  - k8s_container_end: —
  - set_container_pod:
    - pod_uid_suffix: [u8; 64]
    - pod_uid_hash: u64
    - name: string
    - image: string
- Outputs to aggregation:
  - update_node(side, id, az, role, version, env, ns, node_type, address, comm, container_name, pod_name, role_uid) on agg_root
  - update_{tcp,udp,http,dns}_metrics(direction, …aggregates…) on agg_root
- Outputs to logging:
  - core_stats: span utilization, connection message stats, status
  - logger.k8s_container_pod_not_found when K8sContainerSpan cannot resolve a pod

Rust Porting Notes (non-normative; implementation guidance)

- State model: Mirror per-flow, per-side cached state and counters as described. Maintain a metrics_update_side_ Option to gate double counting across the flow lifetime.
- Generated API equivalents: Replace generated C++ handles/containers with Rust abstractions that provide:
  - Index lookups by keys, per-span handles with .valid()/get()/modify() semantics.
  - Flow container foreach over finished timeslots per protocol/direction.
  - agg_root update_* methods and logger/core_stats emitters.
- String handling: C++ uses truncating short-string wrappers for some fields (via modifiers). Ensure identical truncation/length constraints where applicable.
- Ordering and sharding: Implement the lexical ordering and AZ-inclusion policy exactly:
  - AZ included in sharding key only if either node_type==IP; else shard on roles only.
  - Order (role1,az1),(role2,az2) lexicographically to ensure canonical pairing.
- GeoIP: Provide an optional GeoIP lookup to set autonomous_system_organization; expose the autonomous_system_ip_enabled toggle with the same semantics (collapse to "AS" when disabled).
- Timestamps/clock: The flush API should receive a timestamp for the finished slot (now − timeslot_duration). Timeslot duration and advancement come from the virtual clock; the port should maintain the same behavior.
- Logging/metrics: Preserve message categories and cardinality of logs/metrics. Functional parity is higher priority than exact formatting.

Edge Cases and Observed Invariants

- Agent presence invariants are only asserted via debug logs (not enforced): agent_info presence should match task_info and socket_info.
- If no address is available from either side, FlowSpan returns empty NodeData and no agg_root is created; metric propagation skips due to invalid agg_root handle.
- metrics_update_side_ is set on first metrics message observed and not reset thereafter; thus the side chosen may depend on arrival order and applies across all protocols.
- K8s resolution requires non-empty owner_name for k8s_pod to be considered valid.

Cross-File References

- MatchingCore interface: reducer/matching/matching_core.h
- MatchingCore behavior: reducer/matching/matching_core.cc
- FlowSpan interface: reducer/matching/flow_span.h
- FlowSpan behavior: reducer/matching/flow_span.cc
- K8sPodSpan: reducer/matching/k8s_pod_span.{h,cc}
- K8sContainerSpan: reducer/matching/k8s_container_span.{h,cc}
- AwsEnrichmentSpan: reducer/matching/aws_enrichment_span.{h,cc}
- Constants and enums: reducer/constants.{h,inl}
- GeoIP helper: geoip/geoip.h

Non-Goals

- This spec does not duplicate the generated type layouts or codegen DSL; instead it defines behavioral contracts against those APIs.
- Aggregation core behavior is out of scope except where MatchingCore invokes it.
