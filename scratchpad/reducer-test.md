**Goal**
- Port the legacy C++ `reducer_test` to Rust so it runs as a Rust test, validating that the reducer emits internal metrics (via OTLP gRPC or Prometheus) under a minimal configuration.

**What The Existing C++ Test Does**
- File: `reducer/reducer_test.cc:1`
- Starts a small OTLP gRPC test server (`otlp_test_server::OtlpGrpcTestServer`) listening on `localhost:4317`.
- Builds a `reducer::ReducerConfig` with 1 shard for each stage, minimal features, and either:
  - OTLP internal metrics enabled (and Prometheus disabled), or
  - Prometheus internal metrics enabled (and OTLP disabled), binding internal Prom to `localhost:7000`.
- Starts the reducer directly (constructs `reducer::Reducer(loop, config)` and calls `startup()`), and uses a libuv timer to periodically check "stop conditions":
  - For OTLP: at least one OTLP metrics request received by the test server.
  - For Prometheus: curl the internal-prom endpoint and check the body contains known metric names (e.g., `ebpf_net_` / `tcp.bytes`).
- On success or timeout it calls `shutdown()` on the reducer. Due to known C++ shutdown memory issues in tests, the fixture calls `exit(0)` in `TearDown()` after checking for failures; most tests were disabled to avoid those teardown crashes.

**What Changed With The Rust Port**
- The reducer configuration and CLI moved to Rust: `crates/reducer/src/lib.rs:1` builds a final FFI config and calls the C++ entrypoint `otn_reducer_main_with_config`.
- The C++ entrypoint (`reducer/entrypoint.cc:1`) now takes the final config, sets up signal handlers, constructs `Reducer`, and calls `startup()` (which runs the libuv loop and blocks until shutdown via signal).
- There is no direct Rust API to create/hold a `Reducer` instance or to call `shutdown()`; the only control point is the entrypoint that blocks until signaled.

**Implications For Tests**
- We can’t link the legacy GoogleTest fixture anymore. Instead, we should write Rust integration tests that:
  - Spawn the reducer binary (`crates/reducer-bin`) as a child process with appropriate flags.
  - Stand up any external test servers (e.g., a minimal OTLP gRPC server) in the test process.
  - Poll endpoints until conditions are met (or timeout), then terminate the reducer child.
  - Use ephemeral/random ports per test to avoid conflicts and allow parallel test runs, or run tests serially.

**Proposed Test Coverage (Parity With C++ Test)**
1) Prometheus Internal Metrics (fast, no external deps)
   - Configure the reducer to export internal metrics via Prometheus.
   - Bind `internal_prom_bind` to `127.0.0.1:<free_port>`.
   - Disable external Prom metrics (`--disable-prometheus-metrics`) to keep a single HTTP server.
   - Set `--metrics-tsdb-format=prometheus` for scraped metrics (matches C++ test).
   - Start reducer; poll `http://127.0.0.1:<port>/` and assert the body contains a known metric prefix, e.g. `ebpf_net_`.
   - Kill the child process to avoid shutdown issues.

2) OTLP gRPC Internal Metrics (optional; may be gated)
   - Start a small in-process OTLP gRPC test server that implements `ExportMetricsService` and counts requests.
   - Choose a free OTLP port and start the server on `127.0.0.1:<port>`.
   - Start reducer with `--enable-otlp-grpc-metrics --otlp-grpc-metrics-host=127.0.0.1 --otlp-grpc-metrics-port=<port> --otlp-grpc-batch-size=1000` and `--disable-prometheus-metrics`.
   - Wait until the server observes at least one request (or timeout), then kill the child.
   - This test restores the exact semantics of the C++ test’s OTLP path.

Notes:
- The OTLP server can be implemented with `tonic` using `opentelemetry-proto` (already used by `crates/otlp_export`). If CI/network constraints make fetching new crates undesirable, gate this test behind a Cargo feature (e.g. `--features otlp-test`) and mark it `#[ignore]` by default.
- The Prometheus test requires no new dependencies (use `std::net::TcpStream` to issue a simple HTTP GET and read the response).

**Test Layout And Responsibilities**
- Location: `crates/reducer/tests/reducer_integration.rs`
- Helpers:
  - `pick_free_port() -> u16`: bind `TcpListener` to `127.0.0.1:0`, read the assigned port, drop listener.
  - `spawn_reducer(args: &[&str]) -> std::process::Child`: run the reducer binary with the provided CLI args; capture stdout/stderr for debugging.
  - `http_get(addr: &str) -> Result<String, _>`: minimal HTTP 1.1 GET using `TcpStream`; request `/` and return body as `String`.
  - `wait_for(predicate, timeout)`: poll with backoff until predicate returns true or timeout.
- Tests:
  - `prometheus_internal_metrics_smoke()`
    - Ports: one free port for internal-prom (e.g., 7000+ random).
    - Also pick a free telemetry port for the ingest listener to avoid collisions.
    - Args (example):
      - `--print-config` is not required in test, but can be used locally to debug.
      - `--num-ingest-shards=1 --num-matching-shards=1 --num-aggregation-shards=1 --partitions-per-shard=1`
      - `--enable-id-id --enable-az-id` (mirrors the C++ test)
      - `--port=<free_telemetry_port>`
      - `--disable-prometheus-metrics` (to avoid the external metrics server)
      - `--internal-prom=127.0.0.1:<port>`
      - `--metrics-tsdb-format=prometheus`
    - Wait up to 60s for `http_get("127.0.0.1:<port>")` to contain `ebpf_net_`.
    - Tear down: `child.kill()` (or SIGTERM if graceful stop desired).
  - `otlp_internal_metrics_smoke()` (optional, behind feature `otlp-test`)
    - Start tonic-based server; atomically track `requests_observed`.
    - Start reducer with OTLP flags pointing at the server.
    - Wait up to 60s for `requests_observed >= 1`.
    - Tear down: kill child and stop server.

**Sample Skeleton (Prometheus Test)**
```rust
// crates/reducer/tests/reducer_integration.rs
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Command, Stdio, Child};
use std::time::{Duration, Instant};

fn pick_free_port() -> u16 {
    TcpListener::bind(("127.0.0.1", 0)).unwrap().local_addr().unwrap().port()
}

fn spawn_reducer(args: &[&str]) -> Child {
    // Use the built binary; `cargo test` will build it as part of the workspace.
    let bin = std::env::var("CARGO_BIN_EXE_reducer").expect("reducer binary path");
    Command::new(bin)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("failed to spawn reducer")
}

fn http_get(addr: &str) -> std::io::Result<String> {
    let mut stream = TcpStream::connect(addr)?;
    // Minimal HTTP/1.1 GET for "/"
    let req = format!("GET / HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n", addr);
    stream.write_all(req.as_bytes())?;
    let mut buf = String::new();
    stream.read_to_string(&mut buf)?;
    Ok(buf)
}

fn wait_for<F: Fn() -> bool>(deadline: Instant, f: F) -> bool {
    while Instant::now() < deadline {
        if f() { return true; }
        std::thread::sleep(Duration::from_millis(250));
    }
    false
}

#[test]
fn prometheus_internal_metrics_smoke() {
    let prom_port = pick_free_port();
    let telemetry_port = pick_free_port();
    let addr = format!("127.0.0.1:{}", prom_port);

    let args = [
        "--num-ingest-shards=1",
        "--num-matching-shards=1",
        "--num-aggregation-shards=1",
        "--partitions-per-shard=1",
        "--enable-id-id",
        "--enable-az-id",
        &format!("--port={}", telemetry_port),
        "--disable-prometheus-metrics",
        &format!("--internal-prom={}", addr),
        "--metrics-tsdb-format=prometheus",
    ];
    let mut child = spawn_reducer(&args);

    let ok = wait_for(Instant::now() + Duration::from_secs(60), || {
        if let Ok(resp) = http_get(&addr) {
            resp.contains("ebpf_net_")
        } else { false }
    });

    let _ = child.kill();
    assert!(ok, "did not observe internal prom metrics in time");
}
```

Implementation notes:
- To avoid adding dependencies in the repo, the snippet above uses only std. For nicer binary path resolution, you can optionally use `assert_cmd` in dev-dependencies; otherwise resolve the binary path via `CARGO_BIN_EXE_reducer`.
- If test flakiness appears in parallel runs (port races), either:
  - use a more robust free-port allocator that holds the listener until the reducer is spawned, or
  - serialize tests with `RUST_TEST_THREADS=1` or the `serial_test` crate.

**Step-by-Step Migration Plan**
1) Add `crates/reducer/tests/reducer_integration.rs` with the Prometheus test as above.
2) Ensure the test spawns `reducer-bin` (binary name `reducer`). Verify locally with `cargo test -p reducer -- --nocapture` or workspace test.
3) Put the OTLP internal metrics test behind an opt-in Cargo feature `otlp-test`:
   - Add dev-deps on `tonic`, `tokio`, and `opentelemetry-proto` gated by the feature.
   - Implement a minimal `ExportMetricsService` server that increments an `AtomicU64` on each `export` call.
   - Mark the test `#[cfg(feature = "otlp-test")]` and `#[ignore]` by default; document enabling with `cargo test -p reducer --features otlp-test -- --ignored`.
4) Remove or deprecate the legacy C++ `reducer_test.cc` from CI, since it no longer links; keep it as a reference until parity is validated.
5) Optionally, add small utilities to make tests less fragile:
   - SIGTERM-based shutdown for the child (graceful) vs. `kill()` (fast).
   - Capture and print child stderr on failure for easier diagnosis.

**Success Criteria**
- Prometheus test: reliably observes internal metrics within 60s and completes without reducer teardown crashes (child is killed by the test).
- OTLP test (when enabled): observes at least one OTLP `ExportMetrics` request on the test server.
- Tests run via `cargo test` without needing to link or run C++ test harnesses.

**Open Questions / Follow-ups**
- If we want clean shutdown semantics inside the same process (instead of a child), expose an additional FFI function to request shutdown (e.g., an async signal or a dedicated control pipe) and a non-blocking `startup`. For now, the child-process model is simpler and robust.
- If CI cannot fetch new crates, keep the OTLP test opt-in and rely on the Prometheus test for coverage until we vendor the protos/server or reuse the existing C++ test server through a thin test-only bridge.
