# Changelog

## [0.12.0](https://github.com/yonch/opentelemetry-network/compare/v0.11.0...v0.12.0) (2025-11-18)


### ⚠ BREAKING CHANGES

* **kernel-collector:** support CO-RE (compile once run everywhere) by transitioning from bcc to libbpf ([#350](https://github.com/yonch/opentelemetry-network/issues/350))

### Features

* add synk repository scan ([89cac7c](https://github.com/yonch/opentelemetry-network/commit/89cac7cc3244c73c1bba3730f03917243fc8bb48))
* add synk repository scan ([6c8e3a2](https://github.com/yonch/opentelemetry-network/commit/6c8e3a2da8c69452ae2284a17ffd4ca496c18d30))
* **build:** add build environment multi-arch builds ([#400](https://github.com/yonch/opentelemetry-network/issues/400)) ([5ccb104](https://github.com/yonch/opentelemetry-network/commit/5ccb1042704d9c2a677d15cdb0429fc25fa8f571))
* **build:** add devcontainer.json for local builds ([#396](https://github.com/yonch/opentelemetry-network/issues/396)) ([0595e13](https://github.com/yonch/opentelemetry-network/commit/0595e130a2fb79fbf8223d062ad8a1c3f4c34560))
* **build:** refactor version info to cc file to speed up build ([#399](https://github.com/yonch/opentelemetry-network/issues/399)) ([e9cb2b5](https://github.com/yonch/opentelemetry-network/commit/e9cb2b5c82fbf77b2e655a4599d47e71c6e9c6c6))
* **collectors:** remove breakpad crash handlers ([#436](https://github.com/yonch/opentelemetry-network/issues/436)) ([0789b71](https://github.com/yonch/opentelemetry-network/commit/0789b71362434440f4e9dbce799a1e7728d10026))
* **FastDiv:** add remainder computation ([#461](https://github.com/yonch/opentelemetry-network/issues/461)) ([24a417c](https://github.com/yonch/opentelemetry-network/commit/24a417c4b0666e573e30517d70e8f8ccabd10c87))
* **k8s-collector:** add descriptive errors on k8s API connection failure ([6a144aa](https://github.com/yonch/opentelemetry-network/commit/6a144aa23261fcb19613652f95b10505f0d420d1))
* **k8s-collector:** add heartbeat on connection to keep it alive ([905cc57](https://github.com/yonch/opentelemetry-network/commit/905cc5797d6e01b27d0a7d7464ed3db799b65651))
* **k8s-collector:** add info messages on start ([03c4b34](https://github.com/yonch/opentelemetry-network/commit/03c4b34f0b4b7f1a71425f047f91275543191626))
* **k8s-collector:** add Rust k8s-collector to replace k8s-watcher (golang) and k8s-relay (C++) ([f9b5f4f](https://github.com/yonch/opentelemetry-network/commit/f9b5f4fdf769f2ad515e81fab6cf446f94543862))
* **k8s-collector:** add sending of version info message on handshake ([d836d3f](https://github.com/yonch/opentelemetry-network/commit/d836d3f42cc9e5045d7bccd89788574763b042f8))
* **k8s-collector:** flush data to reducer at most 100ms after processing ([66bb58b](https://github.com/yonch/opentelemetry-network/commit/66bb58b4bc9173c2885dc14e5c6537cbfa07be85))
* **k8s-collector:** improve reconnection log messaging ([ef9a05c](https://github.com/yonch/opentelemetry-network/commit/ef9a05c299a9025e2223dff4b84c52163878dd51))
* **kernel-collector:** enhance iovec support for http handlers ([#394](https://github.com/yonch/opentelemetry-network/issues/394)) ([e0d91c7](https://github.com/yonch/opentelemetry-network/commit/e0d91c7d6bae29a084dc5cdf6736e5814f562bf5))
* **kernel-collector:** Support AWS IMDSv2 with IMDSv1 fallback ([#357](https://github.com/yonch/opentelemetry-network/issues/357)) ([e7dadc9](https://github.com/yonch/opentelemetry-network/commit/e7dadc986de6c47da1100c811ec66960c2c3b257)), closes [#356](https://github.com/yonch/opentelemetry-network/issues/356)
* **kernel-collector:** support CO-RE (compile once run everywhere) by transitioning from bcc to libbpf ([#350](https://github.com/yonch/opentelemetry-network/issues/350)) ([a3dabc3](https://github.com/yonch/opentelemetry-network/commit/a3dabc3ebbe47f5012680a69d16cede2bc2608be))
* **kernel-collector:** Use Rust main for kernel-collector ([#420](https://github.com/yonch/opentelemetry-network/issues/420)) ([c9ca1d3](https://github.com/yonch/opentelemetry-network/commit/c9ca1d3f895e4dd8fd28f2d52efd5d7690800df8))
* **reducer:** add aggregation framework and tests ([#462](https://github.com/yonch/opentelemetry-network/issues/462)) ([dfa1c36](https://github.com/yonch/opentelemetry-network/commit/dfa1c3622ed0d3cc003b89bdc68112d2b4febf5a))
* **reducer:** Add skeleton Rust agg core ([#456](https://github.com/yonch/opentelemetry-network/issues/456)) ([6b23665](https://github.com/yonch/opentelemetry-network/commit/6b23665ca59acd31864b948ecdba05d96d13d543))
* **reducer:** Migrate OpenTelemetry exporter to Rust ([#435](https://github.com/yonch/opentelemetry-network/issues/435)) ([d8f0476](https://github.com/yonch/opentelemetry-network/commit/d8f0476fc9aa6c160fc785dce7a5ab75d1a80421))
* **reducer:** Port aggregation core to Rust ([#463](https://github.com/yonch/opentelemetry-network/issues/463)) ([714dbf9](https://github.com/yonch/opentelemetry-network/commit/714dbf9099ea30fc7f5f8c79edbba1be2f849682))
* **reducer:** port timeslot utilities, element_queue, ReducerConfig to Rust and remove signal_handler dependence on parser ([#446](https://github.com/yonch/opentelemetry-network/issues/446)) ([20aee38](https://github.com/yonch/opentelemetry-network/commit/20aee389292ab046d5885aed28204d7a5362e494))
* **reducer:** Switch reducer to Rust main ([#432](https://github.com/yonch/opentelemetry-network/issues/432)) ([aff9acd](https://github.com/yonch/opentelemetry-network/commit/aff9acdc4b234067c78ceea8962b056caf997f4d))
* **render:** add parsed messages and decode code ([#458](https://github.com/yonch/opentelemetry-network/issues/458)) ([320e7cb](https://github.com/yonch/opentelemetry-network/commit/320e7cb20746deed9359c2dadc58caabb58cb80f))
* **render:** add perfect_hash_map crate ([#425](https://github.com/yonch/opentelemetry-network/issues/425)) ([d7f9f1d](https://github.com/yonch/opentelemetry-network/commit/d7f9f1d65ea987d826a9867fe8fcd302b7e07168))
* **render:** Add render_parser library ([#428](https://github.com/yonch/opentelemetry-network/issues/428)) ([876ab02](https://github.com/yonch/opentelemetry-network/commit/876ab02ec0f4162ccadec865475b95e3d313e54f))
* **render:** build Rust libraries from crates/; add render_source_dir target ([#430](https://github.com/yonch/opentelemetry-network/issues/430)) ([fa65668](https://github.com/yonch/opentelemetry-network/commit/fa65668de7da54f41b133da6142a4009089da996))
* **render:** Generate Rust perfect hash ([#423](https://github.com/yonch/opentelemetry-network/issues/423)) ([b81c3d3](https://github.com/yonch/opentelemetry-network/commit/b81c3d355a8a8f414a984e5186993424f9dc2b2e))
* **render:** port message serialization to Rust ([#410](https://github.com/yonch/opentelemetry-network/issues/410)) ([7e65aba](https://github.com/yonch/opentelemetry-network/commit/7e65aba501c93759ff7f9361b135c82daa3756b0))
* start implementing arm64 support ([#313](https://github.com/yonch/opentelemetry-network/issues/313)) ([ae3083a](https://github.com/yonch/opentelemetry-network/commit/ae3083a549d79e2e38c8165cb9e639ebae3a1f0c))


### Bug Fixes

* **benv:** add home link to /install ([#382](https://github.com/yonch/opentelemetry-network/issues/382)) ([540cbd9](https://github.com/yonch/opentelemetry-network/commit/540cbd913458609d2ce0802731d6799f0bd4f19d))
* **build:** patch google depot_tools's insertion of default.xml ([#397](https://github.com/yonch/opentelemetry-network/issues/397)) ([648d147](https://github.com/yonch/opentelemetry-network/commit/648d14746d4acd918a9abf249f299681fb3be52e))
* **ci:** add curl to kernel collector test container ([#387](https://github.com/yonch/opentelemetry-network/issues/387)) ([8db3bd4](https://github.com/yonch/opentelemetry-network/commit/8db3bd4536668ac68bd605c13fce5e7b3ff1cf07))
* **ci:** correctly pull containers from container builds in build-and-test.yaml ([#344](https://github.com/yonch/opentelemetry-network/issues/344)) ([40737cb](https://github.com/yonch/opentelemetry-network/commit/40737cbf8ddc30479b419eabb087a5029bff4463))
* **deps:** update dependency args4j:args4j to v2.37 ([#317](https://github.com/yonch/opentelemetry-network/issues/317)) ([8e9828c](https://github.com/yonch/opentelemetry-network/commit/8e9828c1bb453a40f1a13afff1428e1fa271bba9))
* **deps:** update dependency org.eclipse.emf:org.eclipse.emf.mwe2.launch to v2.23.0 ([#366](https://github.com/yonch/opentelemetry-network/issues/366)) ([c32c326](https://github.com/yonch/opentelemetry-network/commit/c32c3261e735c18831acd31e8a619d2d0f827a31))
* **deps:** update xtextversion to v2.39.0 ([#322](https://github.com/yonch/opentelemetry-network/issues/322)) ([f307564](https://github.com/yonch/opentelemetry-network/commit/f30756428335a12bb920f6e47d1673861405e24c))
* **deps:** update xtextversion to v2.40.0 ([#368](https://github.com/yonch/opentelemetry-network/issues/368)) ([e7b9288](https://github.com/yonch/opentelemetry-network/commit/e7b9288eb172b54f61e0ba65d92b5100c62ffafe))
* GUARDED_BY(mu_) ([59f383d](https://github.com/yonch/opentelemetry-network/commit/59f383de6f3760e24b7b340fb3f49004b0bbf1e4))
* GUARDED_BY(mu_) ([dde5d81](https://github.com/yonch/opentelemetry-network/commit/dde5d8104e1f8da73dafbcba002b1b5d705fad62))
* GUARDED_BY(stats_mu_) ([a6424b5](https://github.com/yonch/opentelemetry-network/commit/a6424b57404a6f8f1b6f69c58deefe7d963228ea))
* json modifier ([7dcaff9](https://github.com/yonch/opentelemetry-network/commit/7dcaff965ec68cd2b8696298f433cc55dc332e04))
* json modifier ([10f79bb](https://github.com/yonch/opentelemetry-network/commit/10f79bbc6576fe0059b93c78d43522990548ef68))
* **k8s-collector:** add connect message to handshake ([544955b](https://github.com/yonch/opentelemetry-network/commit/544955bbe676f02996aba4715520cd7ef80309e5))
* **k8s-collector:** keep separate Stores for ReplicaSet and Job metadata ([b37bd5a](https://github.com/yonch/opentelemetry-network/commit/b37bd5af7711c78190ece6193407aaa70b1acb71))
* **kernel-collector:** fix string comparison in http detection ([#393](https://github.com/yonch/opentelemetry-network/issues/393)) ([e40d331](https://github.com/yonch/opentelemetry-network/commit/e40d331d47373461180e8237cddfa68c90a4aee6))
* **kernel-collector:** improve nat lifecycle hooks ([#377](https://github.com/yonch/opentelemetry-network/issues/377)) ([a3315f8](https://github.com/yonch/opentelemetry-network/commit/a3315f8f92871f05a8d52c1205b43c96fb4f6897))
* **kernel-collector:** make DNS kprobe available ([#378](https://github.com/yonch/opentelemetry-network/issues/378)) ([b2118f1](https://github.com/yonch/opentelemetry-network/commit/b2118f1c45c138564ab25b0d6c7753af085b35f1))
* log modifier ([3c51360](https://github.com/yonch/opentelemetry-network/commit/3c51360ea170168e061b985d3c39997a1ee032c0))
* **nat:** add nat existing kprobe with exported symbol ([#353](https://github.com/yonch/opentelemetry-network/issues/353)) ([2458282](https://github.com/yonch/opentelemetry-network/commit/24582820671d3684fa2edc1a7e97042a0ccd2d4f))
* **tests:** Resolve and work around some ASAN issues ([#408](https://github.com/yonch/opentelemetry-network/issues/408)) ([8e0f38e](https://github.com/yonch/opentelemetry-network/commit/8e0f38edf932281b4256985c8002d5cdae730989))
* update otlp_grpc ([e73ef23](https://github.com/yonch/opentelemetry-network/commit/e73ef235195af7128c5068354d09045e00a7fe86))
* update release github workflow ([#365](https://github.com/yonch/opentelemetry-network/issues/365)) ([5e07f9d](https://github.com/yonch/opentelemetry-network/commit/5e07f9d6875c72ea0e4e2e44a13a83c13b0d17c5))


### CI

* add ccache to build-and-test.yaml ([#386](https://github.com/yonch/opentelemetry-network/issues/386)) ([a5d0b75](https://github.com/yonch/opentelemetry-network/commit/a5d0b750757be8a254abf1e314556df84bc6d0ea))
* add end-to-end test with kernel-collector, reducer, otelcol ([#427](https://github.com/yonch/opentelemetry-network/issues/427)) ([bcdfc95](https://github.com/yonch/opentelemetry-network/commit/bcdfc954ecc6d168e72809424af094d2eafe57f6))
* add release-please for triggering releases ([cab07f4](https://github.com/yonch/opentelemetry-network/commit/cab07f45637a769d46990ffbdcb506c9370c328b))
* increase workload visibility in bpf_log test ([#395](https://github.com/yonch/opentelemetry-network/issues/395)) ([f585685](https://github.com/yonch/opentelemetry-network/commit/f585685ff4d4c08e48ec9f13fc09588b9271fb6d))
* install packages for LVH using cache ([#405](https://github.com/yonch/opentelemetry-network/issues/405)) ([f5b9504](https://github.com/yonch/opentelemetry-network/commit/f5b95044bbf4e02019a5ad87e6647e91f82eeffd))
* reduce ccache sizes in build-and-test ([#407](https://github.com/yonch/opentelemetry-network/issues/407)) ([892690d](https://github.com/yonch/opentelemetry-network/commit/892690d40a8a5e7136ce533ff7b77a9b8b99b0e2))
* remove snyk gha ([#333](https://github.com/yonch/opentelemetry-network/issues/333)) ([30d6d15](https://github.com/yonch/opentelemetry-network/commit/30d6d15ddaf5896b81cead4b7c9ad1017f5b8e76))
* update e2e test to include k8s-collector ([0f583c2](https://github.com/yonch/opentelemetry-network/commit/0f583c2257da43993b8ee57fff2e6dd88e417314))
* update runners to ubuntu-24.04 ([#329](https://github.com/yonch/opentelemetry-network/issues/329)) ([073f9fb](https://github.com/yonch/opentelemetry-network/commit/073f9fb9733f50204aaef08d4b5114e4e776256b))
* upload e2e container logs as artifacts ([deb09d9](https://github.com/yonch/opentelemetry-network/commit/deb09d987b10c3620e53fd56413b807a26052994))
* use build tools with distro packages of curl, openssl, grpc, and abseil ([#338](https://github.com/yonch/opentelemetry-network/issues/338)) ([f80da3d](https://github.com/yonch/opentelemetry-network/commit/f80da3dc858f12e108ad7ea4ae14dd31931ba206))
* use little-vm-helper for kernel tests ([#337](https://github.com/yonch/opentelemetry-network/issues/337)) ([fa0c4fe](https://github.com/yonch/opentelemetry-network/commit/fa0c4fe40d6626985ccbfa1a95587ebce821e28a))

## Changelog

All notable changes to this project will be documented in this file by Release Please.
