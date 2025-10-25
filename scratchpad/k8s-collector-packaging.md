K8s Collector Packaging (Rust)

Overview
- Binaries: The k8s-collector Rust port lives under `crates/k8s-collector` with a CLI binary in `crates/k8s-collector-bin` producing a `k8s-collector` executable.
- Role: Connects to the Kubernetes API (Pods/ReplicaSets/Jobs), correlates ownership, and streams encoded events directly to the reducer over TCP.
- Env vars: `EBPF_NET_INTAKE_HOST`, `EBPF_NET_INTAKE_PORT`, plus optional tuning (`K8S_COLLECTOR_DELETE_TTL_SECS`, `K8S_COLLECTOR_DELETE_CAPACITY`, and `K8S_COLLECTOR_WAITING_LIMIT`).

- Container Packaging
- Build system: We integrate with existing CMake packaging used by other collectors. The `cmake/rust_main.cmake` helper builds Rust binaries and provides a stripped variant and artifacts for Docker.
- New image: `k8s-collector` packaged similarly to other agents. The Dockerfile is in `collector/k8s/Dockerfile`, referenced from the k8s-collector CMake via `DOCKERFILE_PATH`.
- Base image: `bitnami/minideb:trixie` for consistency with current images.
- Entrypoint: Runs the binary directly as `/srv/k8s-collector`.
- Transitional shims: The image creates symlinks so it can be used as a drop-in override for existing Helm chart containers without changing chart commands:
  - `/srv/k8s-watcher` → `/srv/k8s-collector`

- Targets and CMake Integration
- All build and packaging rules live in `collector/k8s/CMakeLists.txt`.
- Target: `k8s-collector` (binary) and `k8s-collector-stripped` (stripped binary) via `add_rust_main()` referencing Cargo package `k8s-collector-bin`.
- Docker: `k8s-collector-docker` and `k8s-collector-docker-registry` via `build_custom_docker_image()` using `collector/k8s/Dockerfile`; we copy the stripped binary, `debug-info.conf`, and LICENSE/NOTICE.
- Aggregation: `collectors` depends on `k8s-collector`, and `collectors-docker(-registry)` depend on the docker targets, so `pipeline-docker(-registry)` builds and pushes it automatically.

Helm Chart Changes (opentelemetry-ebpf)
- Current chart: `opentelemetry-helm-charts/charts/opentelemetry-ebpf` deploys two containers in one pod for the Kubernetes collector: `k8s-watcher` and `k8s-relay`.
- Long-term: Update the chart to include an optional single-container mode for `k8s-collector` (Rust) that talks directly to the reducer. Proposed values additions (high-level):
  - `networkExplorer.k8sCollector.mode: [split|unified]` (default `split`)
  - `networkExplorer.k8sCollector.collector.image.{repository,name,tag}` when `mode=unified` to control the single container image.
- Env: Maintain `EBPF_NET_INTAKE_HOST` and `EBPF_NET_INTAKE_PORT` in the container `env` section (these are already used by other agents and by the Rust collector).

Transitional Override (No Chart Changes)
- Goal: Test the Rust collector by overriding only one container in the existing deployment.
- Recommended path: Override the watcher container’s image to the `k8s-collector` image. The relay can remain present but unused; the Rust collector connects directly to the reducer.
  - The `k8s-collector` image provides `/srv/k8s-watcher` as a symlink to `/srv/k8s-collector`, so a chart `command: ["/srv/k8s-watcher"]` still works.
  - Ensure `EBPF_NET_INTAKE_HOST` and `EBPF_NET_INTAKE_PORT` are set (chart already does this for collectors).

Example values override (transitional):
  networkExplorer:
    k8sCollector:
      watcher:
        image:
          repository: localhost:5000
          name: k8s-collector
          tag: latest
      # Optional: quiet the old watcher by switching it to a pause image (advanced)
      # watcher:
      #   image:
      #     repository: gcr.io/google_containers
      #     name: pause-amd64
      #     tag: "3.1"

Apply override for testing:
- helm upgrade --install <release> /workspace/opentelemetry-helm-charts/charts/opentelemetry-ebpf -f values.override.yaml

Alternate override (relay container):
- Not recommended. The Rust collector replaces the relay role entirely; overriding the relay image leaves the Go watcher trying to connect to a non-existent RPC. Prefer overriding the watcher image as shown above.

GitHub Workflows
- Build and Test (`.github/workflows/build-and-test.yaml`):
  - Add a `build-k8s-collector` job mirroring the `build-k8s-relay` job to compile the Rust collector via the existing build container.
- Release (`.github/workflows/build-and-release.yaml`):
  - `pipeline-docker-registry` now builds and pushes `localhost:5000/k8s-collector` as part of `collectors-docker-registry`.
  - Pull `localhost:5000/k8s-collector` after the pipeline build.
  - Include `k8s-collector` in the list of pushed images (with tags matching other images).

What I Changed (Implementation Summary)
- Added container packaging for the Rust collector:
  - Consolidated build rules into `collector/k8s/CMakeLists.txt`; no nested CMakeLists.
  - CMake target `k8s-collector` (via `add_rust_main`) and Docker packaging (`k8s-collector-docker` and `k8s-collector-docker-registry`). Dockerfile lives in `collector/k8s/Dockerfile`.
  - Hooked `k8s-collector` into `collectors` and `collectors-docker` via CMake dependencies.
  - Consolidated `collector/k8s/CMakeLists.txt` to remove the old `k8s-watcher` and `k8s-relay` targets; only the Rust `k8s-collector` is built and packaged now.
- CI:
  - build-and-test: added a job to compile `k8s-collector` like other agents and removed the old `build-k8s-watcher` and `build-k8s-relay` jobs.
  - build-and-release: ensured the image is built, pulled from local registry, and pushed with the other agent images; dropped watcher/relay images from push list.
- Transitional Helm guidance: Provided a values override snippet to swap only the watcher container to the new `k8s-collector` image and notes for relay override if needed.

Notes and Follow-ups
- Chart update proposal: Add a `mode: unified` option for a single Rust collector container and wire up values for its image and env. This allows removing the split watcher/relay deployment once validated.
- Probes and resources: For the Rust collector, consider adding reasonable `readinessProbe`/`livenessProbe` and resource requests/limits in the chart once adopted.
- Image hardening: Optionally switch to `distroless/static` or `gcr.io/distroless/base-nonroot` after confirming dynamic lib requirements; current base matches the rest of the stack and keeps consistency for now.
