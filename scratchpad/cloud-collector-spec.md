Cloud Metadata Collector (current C++ agent)
===========================================

Purpose
- Discover cloud metadata that is not available from the kernel collector alone, and publish it into the ingest pipeline.
- Specifically:
  - Identify the cloud platform and node identity (AWS, GCP, or unknown).
  - Provide node-level metadata (AZ/zone, role, instance ID/type, IP mappings).
  - Periodically enumerate AWS EC2 network interfaces across all regions and expose per-IP metadata for enrichment.
- Maintain a continuous connection to the ingest server, with heartbeats, resource-usage reports, and health signals.

High-level architecture
- Single binary `cloud-collector` that:
  - Parses configuration and command-line flags.
  - Connects to the ingest server (the “reducer”) using the common intake protocol.
  - Performs an initial metadata handshake so the server can identify this agent and its environment.
  - Periodically queries AWS EC2 for network interface information and streams that metadata.
  - Reports its own health and resource usage.
- Downstream, the ingest/reducer side:
  - Maintains an `aws_network_interface` span keyed by IP address.
  - Uses `network_interface_info` messages from this agent to annotate those spans with AWS metadata, which is then used to enrich flows.

Scope of this spec
- Focuses on **externally observable behavior and protocol**, not C++ implementation details.
- Intended as a parity spec for a Rust reimplementation; behavior that is visible to:
  - The ingest server (wire messages, ordering, frequencies).
  - AWS/GCP APIs (which endpoints are called and how often).
  - Operators configuring and running the collector.

Configuration and inputs
------------------------

CLI arguments (entrypoint)
- `--config-file=<path>` (optional)
  - YAML config file containing:
    - `labels`: key/value string labels attached to the node (e.g., environment, role).
    - `intake.host`, `intake.port`: ingest server address.
  - If omitted, defaults are used and no labels are set.
  - Invalid or missing file:
    - If the file cannot be read and `FailMode::exception` is used (current default), the collector logs and exits.
    - If the path is empty, the file is ignored and defaults apply.
- `--intake-host`, `--intake-port`, `--intake-encoder`
  - Standard intake options (from `config::IntakeConfig::ArgsHandler`).
  - These override both config file and environment for host/port/encoder.
- `--ec2-poll-interval-ms=<millis>`
  - Interval between AWS enumeration cycles.
  - Default: 1000ms (1s).
  - Must be non-zero; if set to `0`, the collector logs an error and exits with failure.
- `--aws-timeout=<millis>`
  - Timeout for fetching **instance metadata** (AWS and GCP) during connection metadata handshake.
  - Default: 1000ms.
  - Does **not** affect AWS EC2 API calls for network interfaces; those use SDK defaults.
- Standard logging whitelist flags
  - Flags are registered for log filtering by component (e.g., `channel`, `cloud-platform`, `utility`).
  - Affect only logs, not behavior.

Environment variables
- Intake configuration:
  - `EBPF_NET_INTAKE_HOST`, `EBPF_NET_INTAKE_PORT`
    - If set, override config file defaults and are used unless overridden by CLI.
  - `EBPF_NET_INTAKE_ENCODER`
    - Chooses intake encoder; current C++ cloud collector only uses the default encoder.
  - `EBPF_NET_RECORD_INTAKE_OUTPUT_PATH`
    - If set, writer will mirror outgoing traffic to the given path (via the generic intake machinery).
- AWS authentication and region:
  - Uses standard AWS SDK resolution (environment variables, shared config, instance profiles, etc.).
  - In particular, the README recommends:
    - `AWS_ACCESS_KEY_ID`
    - `AWS_SECRET_ACCESS_KEY`
  - Region is resolved via the AWS SDK; there is no collector-specific region flag.
- Debugging:
  - Container entrypoint shell script supports `EBPF_NET_RUN_UNDER_GDB` and related vars for debugging, but this is not part of the functional protocol.

Static defaults and constants
- Heartbeat:
  - Interval: 2 seconds (`HEARTBEAT_INTERVAL`).
  - Heartbeat messages are sent only while connected.
- Ingest write buffer:
  - Size: 16 KiB (`WRITE_BUFFER_SIZE`) for batched output to the upstream TCP/TLS connection.

Runtime lifecycle
-----------------

Startup
- Initialize a single event loop and basic signal handling:
  - Disable core dumps.
  - Ignore `SIGPIPE` for robust I/O.
- Parse CLI arguments and environment:
  - Apply config-file values, then environment overrides, then CLI overrides for intake settings.
  - Validate `--ec2-poll-interval-ms != 0`.
- Resolve host name:
  - Attempt to get the local host name via `uname`.
  - On failure, use `"(unknown)"`.
- Generate a random agent ID string (for logs only); not transmitted on the wire.
- Log startup information:
  - Release version (from `versions::release`).
  - Build mode.
  - Hostname.
  - Agent ID.

Ingest connection establishment
- Construct an intake connection using the resolved `IntakeConfig`:
  - Connects to the configured host and port over TCP/TLS (per shared intake configuration).
  - Uses a reconnecting channel that:
    - Attempts to connect immediately on startup.
    - Maintains a persistent connection, reconnecting after failures with backoff.
  - Uses an `ebpf_net::ingest::Writer` to emit messages.
- Once the underlying TCP/TLS connection is established:
  - The ingest connection notifies a generic “connection caretaker” component (see next section).
  - On the **first** successful connect (and on each reconnect), the caretaker performs a metadata handshake and starts heartbeats.
  - Only after the caretaker signals “connected” does the collector start periodic AWS enumeration.

Graceful termination
- Signal handling:
  - `SIGINT` or `SIGTERM`:
    - Logs that a signal was caught.
    - Stops listening to further signals.
    - Exits the process with a negative exit code derived from the signal number.
  - No special logic to flush pending AWS enumerations; exit is immediate once the signal is processed.
- Normal shutdown path:
  - Stops the event loop.
  - Shuts down the AWS SDK.
  - Closes the UV loop structure.

Upstream connection & metadata handshake
----------------------------------------

Cloud platform detection
- On each successful ingest connection:
  - The caretaker tries to fetch instance metadata from:
    - AWS Instance Metadata Service (IMDS).
    - GCP instance metadata service.
  - The configured `--aws-timeout` is applied as an overall timeout budget for each metadata fetch.
  - Outcomes:
    - If AWS metadata fetch succeeds:
      - Cloud platform is `aws`.
      - AWS account, AZ, role, instance ID/type and network interfaces are available.
    - Else if GCP metadata fetch succeeds:
      - Cloud platform is `gcp`.
      - Cluster name/location, AZ/zone, role, instance ID/type and network interfaces are available.
    - Else:
      - Cloud platform is `unknown`.
      - Only hostname and config labels are known.
  - Fetch errors are logged but do not prevent the connection from being used.

Metadata handshake message sequence
- For each ingest connection, once the TCP/TLS link is up:
  - Compression is **temporarily disabled** so the first version message is uncompressed.
  - Then the collector sends, in order:
    1. `version_info`
       - Agent version: `major`, `minor`, `patch` from the embedded release version.
       - Purpose: allows the ingest server to track client versions.
    2. Compression is re-enabled (if allowed by the intake configuration).
    3. `connect`
       - Fields (logical):
         - `client_type = cloud`.
         - `hostname` = resolved host name (from `uname` or `"(unknown)"`).
       - Purpose: identifies the logical role of the agent and its host to ingest.
    4. `report_cpu_cores`
       - Fields:
         - Number of CPU cores, from `std::thread::hardware_concurrency()`.
       - Purpose: informs ingest about the hardware footprint for resource metrics.
    5. `set_config_label` (one per configured label)
       - For each label from the config file:
         - Key/value strings (subject to length limits enforced at config-load time).
       - Purpose: attach logical labels (e.g. env, role) to this node/agent.
    6. Cloud platform and node information:
       - If AWS metadata is available:
         - `cloud_platform`
           - Value: `aws`.
         - Optional `cloud_platform_account_info`
           - Sends the AWS account ID, if available.
         - `set_node_info`
           - Fields (conceptual):
             - Availability zone (e.g. `us-east-1a`).
             - IAM role name.
             - Instance ID (with the `i-` prefix stripped).
             - Instance type (e.g. `m5.large`).
         - Network interface messages based on AWS metadata:
           - For each network interface on the local instance:
             - For each private IPv4 address:
               - `private_ipv4_addr`
                 - Fields:
                   - IPv4 address (network order).
                   - VPC ID as a short string.
             - For each IPv6 address:
               - `ipv6_addr`
                 - Fields:
                   - IPv6 address bytes.
                   - VPC ID.
             - For each mapping *(public IPv4 → private IPv4)*:
               - `public_to_private_ipv4`
                 - Fields:
                   - Public IPv4.
                   - Private IPv4.
                   - VPC ID.
       - Else if GCP metadata is available:
         - `cloud_platform`
           - Value: `gcp`.
         - (Account ID is currently **not** sent even if derivable.)
         - `set_node_info`
           - Fields:
             - Availability zone (derived from the instance `zone`).
             - Role (typically service account or a name derived from metadata).
             - Hostname.
             - Machine type.
         - Network interface messages based on GCP metadata:
           - For each interface:
             - If private address is IPv4:
               - `private_ipv4_addr` + `public_to_private_ipv4` for each associated public IP.
             - If private address is IPv6:
               - `ipv6_addr` with VPC ID.
       - Else (no cloud metadata):
         - `cloud_platform`
           - Value: `unknown`.
         - `set_node_info`
           - Fields:
             - Empty AZ.
             - Empty role.
             - Hostname.
             - Empty instance type.
         - No network interface messages are sent in this case.
    7. `metadata_complete`
       - Indicates that metadata handshake for this connection is complete.
       - Downstream ingest can treat this as “node/agent identity is fully described”.
  - After sending the above:
    - The caretaker calls the `on_connected` callback used by the cloud collector.
    - This is the trigger for the collector to start periodic AWS enumeration.

Heartbeats
- While connected:
  - At fixed interval (`HEARTBEAT_INTERVAL = 2s`), the caretaker sends:
    - `heartbeat`
      - Identifies the agent connection as live.
  - Heartbeat messages are flushed immediately after being enqueued.
  - If the underlying writer is not writable (e.g., connection lost), no heartbeat is sent; connection recovery is handled by the reconnecting channel.

Periodic AWS enumeration
------------------------

Overview
- Once the metadata handshake is complete for a connection:
  - The cloud collector starts a repeating job (“enumeration cycle”) at the configured interval (`ec2-poll-interval-ms`).
  - Each cycle:
    - Reports current process resource usage.
    - Enumerates AWS regions.
    - For each region, enumerates EC2 network interfaces and emits IP-scoped metadata.
    - Updates its in-memory set of “live” AWS network-interface spans.
    - Reports collector health based on success/failure of AWS API calls.
  - On connection errors, enumeration is stopped and any previously held spans are released.

Frequency and backoff
- Base interval:
  - `base_interval = ec2-poll-interval-ms` (default 1s).
  - The first cycle runs roughly `base_interval` after metadata-complete.
- Job follow-up behavior:
  - If the cycle completes with **no AWS API errors**:
    - Next run scheduled after `base_interval`.
    - Any previous backoff is reset.
  - If the cycle encounters **AWS API errors** (non-throttling):
    - The job requests backoff.
    - The scheduler multiplies the interval by a factor of 2 each time a backoff result is returned:
      - `base_interval, 2×, 4×, 8×, …` until a subsequent successful cycle returns `ok`, which resets the backoff.
  - Throttling responses:
    - AWS `THROTTLING` errors are logged but **do not** contribute to `collector_health` updates in the current implementation.
    - The cycle continues, and backoff behavior is determined by other errors encountered in the same run.

Resource usage reporting
- At the beginning of each enumeration cycle:
  - The collector samples its own resource usage and emits:
    - `agent_resource_usage`
      - Fields (conceptual):
        - User-mode CPU time (since process start).
        - Kernel-mode CPU time.
        - Max RSS.
        - Minor/major page faults.
        - Block I/O counts.
        - Voluntary/involuntary context switches.
        - CPU utilization (per-mille) and one reserved field (currently 0).
  - This call is **best-effort**:
    - If resource usage cannot be obtained, an error is logged and **no message is emitted** for that cycle.
    - Enumeration continues regardless of resource reporting success.

AWS region enumeration
- The collector uses AWS EC2 APIs in two tiers:
  1. Global region listing
     - Call: `DescribeRegions` with an empty request (no filters).
     - Uses the default EC2 client (region determined by AWS SDK).
     - If this call fails:
       - Logs the error.
       - Emits a `collector_health` message with status `aws_describe_regions_error` and detail equal to the HTTP status code (for non-throttling errors).
       - Returns `backoff` for this cycle; no per-region network-interface calls are attempted.
  2. Per-region interface listing
     - For each region in the successful `DescribeRegions` result:
       - Create an EC2 client configured for that region.
       - Call `DescribeNetworkInterfaces` with an empty request (no filters).
       - If the call fails for a region:
         - Logs the error.
         - Emits a `collector_health` message with status `aws_describe_network_interfaces_error` and detail equal to the HTTP status code (for non-throttling errors).
         - Marks the overall cycle result as `backoff` but continues to other regions.
       - If the call succeeds:
         - Retrieves all network interfaces from the response (pagination is handled by the AWS SDK).
         - For each interface, transforms it into one or more IP-scoped span updates as described below.

Network interface → spans mapping
- Keyed by IP:
  - The ingest-side `aws_network_interface` span is keyed by an IP address stored as a 128-bit number.
  - For compatibility with the rest of the pipeline:
    - IPv4 addresses are converted into an IPv6 representation (using the same IPv4→IPv6 embedding that the rest of the system uses).
    - IPv6 addresses are used directly.
  - Contract for Rust port:
    - The IP-to-key mapping must be identical to the existing pipeline convention so that lookups for flow IPs match these spans.
- Per-interface data extraction:
  - For each `NetworkInterface` returned by AWS:
    - Read:
      - `Attachment`:
        - `InstanceId`
        - `InstanceOwnerId`
      - `Association`:
        - `IpOwnerId`
        - `PublicDnsName`
        - `PublicIp` (may be absent)
      - Interface attributes:
        - `VpcId`
        - `AvailabilityZone`
        - `NetworkInterfaceId`
        - `InterfaceType` (enum, cast to `u16`)
        - `PrivateDnsName`
        - `Description`
      - Private IPs:
        - `PrivateIpAddresses[*].PrivateIpAddress`
      - IPv6 addresses:
        - `Ipv6Addresses[*].Ipv6Address`
- Per-IP emission rules:
  - For each interface:
    - Build a **logical “entry”** per IP address associated with that interface:
      1. If `Association.PublicIp` is set and parses as IPv4:
         - Treat it as a public IPv4 address for the interface.
         - Create one entry for that public IP.
      2. For each `PrivateIpAddresses[*].PrivateIpAddress` that parses as IPv4:
         - Create one entry for that private IPv4.
      3. For each `Ipv6Addresses[*].Ipv6Address` that parses as IPv6:
         - Create one entry for that IPv6.
    - Each entry:
      - Looks up or allocates a span in the `aws_network_interface` index keyed by the IP.
      - Emits a `network_interface_info` message with the following fields:
        - `ip_owner_id`
        - `vpc_id`
        - `az`
        - `interface_id`
        - `interface_type` (numeric enum value).
        - `instance_id`
        - `instance_owner_id`
        - `public_dns_name`
        - `private_dns_name`
        - `interface_description`
      - The entry is kept as a local handle so that its span lifetime can be controlled across cycles.

Span lifetime across enumeration cycles
- The enumerator maintains a vector of handles to `aws_network_interface` spans corresponding to the most recent enumeration:
  - Before storing new handles:
    - It **releases all previous handles**, which decrements refcounts and may cause outdated spans to be deallocated on the ingest side.
  - After enumerating interfaces across all regions:
    - It replaces the internal handle set with the handles from the latest cycle.
  - Effectively:
    - The ingest side always sees a live set of `aws_network_interface` spans corresponding to the **most recent** successful enumeration.
    - IPs that disappear from AWS responses between cycles are dropped when their handles are released.
- On connection error (see below), the collector proactively frees all handles before sleeping and reconnecting:
  - This removes all AWS interface spans associated with this agent on the ingest side.

Cloud collector health reporting
--------------------------------

Status values
- The collector uses a shared `collector_health` metric encoded as:
  - `status`: enum `CollectorStatus` (u16):
    - `healthy = 0`
    - `unknown = 1` (not currently used by the cloud collector in normal flow).
    - `aws_describe_regions_error = 2`
    - `aws_describe_network_interfaces_error = 3`
  - `detail`: additional numeric detail, interpreted as HTTP status code on errors.

When health is reported
- On successful enumeration cycles:
  - At the end of a cycle in which **no** AWS API errors were encountered:
    - Emit `collector_health(status = healthy, detail = 0)`.
  - This effectively declares the cloud collector healthy after the last cycle.
- On AWS API failures:
  - If `DescribeRegions` fails:
    - Emit `collector_health(status = aws_describe_regions_error, detail = <HTTP status>)`.
    - Do not enumerate per-region interfaces in that cycle.
  - If `DescribeNetworkInterfaces` fails for a region:
    - Emit `collector_health(status = aws_describe_network_interfaces_error, detail = <HTTP status>)`.
    - Continue to other regions, but mark the overall cycle as `backoff`.
  - Throttling (`EC2Errors::THROTTLING`):
    - Recognized as a special case:
      - Logged as “AWS API call throttled”.
      - Does **not** emit a `collector_health` message.
      - The cycle still completes and may be treated as successful unless other errors are present.

Ingest connection error handling & reconnection
-----------------------------------------------

Error propagation
- The ingest connection is observed by the cloud collector through a callback interface:
  - On ingest connection error:
    - The cloud collector is notified with an error code.
  - On ingest connection close:
    - The reconnecting channel will put the connection into a backoff state and attempt reconnection.

Collector-level reaction to errors
- On ingest connection error callback:
  - Stop the periodic enumeration scheduler:
    - No further AWS enumeration cycles are scheduled until reconnection.
  - Free all stored `aws_network_interface` handles:
    - This removes all AWS network interface spans on the ingest side associated with this agent.
  - Sleep for a base reconnect delay with jitter:
    - Base: 5 seconds.
    - Jitter: ±1 second (uniform).
    - This sleep is independent of the reconnecting channel’s own reconnection timers and serves as a “cooldown” at the collector level.
- On ingest connection close:
  - The reconnecting channel transitions to a backoff state and eventually attempts to reconnect.
- On ingest connection re-establishment:
  - The caretaker:
    - Sends metadata handshake messages (as above).
    - Starts heartbeats.
  - The caretaker invokes the collector’s “connected” callback.
  - The collector then restarts the enumeration scheduler with the configured base interval.

Signal handling behavior
------------------------

- On startup:
  - The collector installs minimal signal handling:
    - Disables core dumps (`RLIMIT_CORE = 0`).
    - Sets `SIGPIPE` to be ignored.
- During runtime:
  - The collector can register explicit handlers for:
    - `SIGINT`, `SIGTERM`.
  - On these signals:
    - Logs the caught signal.
    - Clears all registered signal handlers to avoid re-entry.
    - Exits the process with status `-signal_number`.
- There is no protocol-level signal indicating graceful shutdown; ingest observes the connection closing.

External observable behavior summary
------------------------------------

From the ingest server’s perspective, a correctly functioning cloud collector:
- On connect:
  - Sends:
    - `version_info`
    - `connect(client_type = cloud, hostname)`
    - `report_cpu_cores`
    - `set_config_label` messages for all configured labels.
    - `cloud_platform` and optional `cloud_platform_account_info`.
    - `set_node_info` and network-interface IP mapping messages (`private_ipv4_addr`, `ipv6_addr`, `public_to_private_ipv4`) based on cloud metadata, when available.
    - `metadata_complete`.
- While connected:
  - Sends `heartbeat` every ~2 seconds.
  - Performs an enumeration cycle every `ec2-poll-interval-ms` (with exponential backoff on AWS API errors), where each cycle:
    - Emits `agent_resource_usage` once.
    - Emits one `network_interface_info` message per IP address attached to each AWS EC2 network interface across all regions.
    - Emits `collector_health` reporting healthy or specific AWS error codes.
  - Maintains a live set of `aws_network_interface` spans keyed by IP, corresponding to the latest snapshot.
- On ingest connection errors:
  - Stops enumeration and heartbeats (as connection is down).
  - Drops all `aws_network_interface` spans by releasing handles.
  - After pause + reconnection, repeats the metadata handshake and then resumes enumeration.
- On process termination:
  - Connection drops; ingest eventually times out the agent as inactive based on its own logic.

Porting considerations for Rust implementation
----------------------------------------------

Behavioral parity requirements
- Message semantics and ordering:
  - Preserve the handshake sequence:
    - `version_info` → `connect` → `report_cpu_cores` → labels → `cloud_platform`/`cloud_platform_account_info` → `set_node_info` + interface IP mappings → `metadata_complete`.
    - It is acceptable if compression details differ, but the sequence and content should be preserved.
  - Maintain the same message types and field semantics:
    - Use the existing `ebpf_net::ingest` and `ebpf_net::cloud_collector` schemas (or their Rust equivalents) so that the reducer behavior remains unchanged.
- Enumeration behavior:
  - Continue to:
    - Enumerate **all** AWS regions via `DescribeRegions` + per-region `DescribeNetworkInterfaces`.
    - Treat throttling differently from hard errors (log vs. health).
    - Emit one `network_interface_info` per IP per interface, with the same field sources.
  - Maintain snapshot semantics:
    - Each run replaces the previous set of live `aws_network_interface` spans.
    - Removing spans for IPs no longer present is important for correctness.
- Scheduling and backoff:
  - Respect configurable base interval and non-zero constraint.
  - Implement a geometric backoff when AWS calls fail, resetting on success.
- Health reporting:
  - Use the same `CollectorStatus` codes and mapping to HTTP response codes.
  - Do not treat throttling as a change in collector health, unless intentionally changing semantics.
- Cloud metadata behavior:
  - On each new ingest connection, attempt AWS metadata, then GCP metadata, then fall back to unknown:
    - Send `cloud_platform` and `set_node_info` accordingly.
    - Populate IP mapping messages from cloud metadata when available.
  - Apply a configurable timeout to metadata fetches (`--aws-timeout` equivalent).
- IP key encoding:
  - Preserve the IP → key mapping used for `aws_network_interface` spans:
    - IPv4 embedded into IPv6 in the same way as the rest of the system.
    - IPv6 passed through unchanged.

Non-goals / permissible changes
- The Rust implementation may:
  - Use different internal concurrency models or runtime (e.g., tokio instead of libuv) as long as observable behavior is equivalent.
  - Adjust internal logging, as long as message protocol and frequencies are preserved.
  - Improve robustness (e.g., refreshing cloud metadata periodically) if it **does not** break assumptions about message types and core semantics.
  - Extend configuration options, provided defaults preserve current behavior.

