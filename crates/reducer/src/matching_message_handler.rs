//! Matching message decoding that prints all fields for visibility during the port.

use std::rc::Rc;

use render_parser::Parser;

use encoder_ebpf_net_matching::parsed_message;
use encoder_ebpf_net_matching::wire_messages;

type Handler = Box<dyn Fn(u32, usize, u64, &[u8]) + 'static>;

#[derive(Clone)]
pub struct MatchingMessageHandler;

impl MatchingMessageHandler {
    pub fn new(parser: &mut Parser<Handler, fn(u32) -> u32>) -> Rc<Self> {
        let rc = Rc::new(Self);

        // Register all matching messages; each closure decodes and prints fields
        {
            let h = rc.clone();
            let _ = parser.add_message(
                wire_messages::jb_matching__flow_start::metadata(),
                Box::new(move |shard, q, ts, buf| h.on_flow_start(shard, q, ts, buf)),
            );
        }
        {
            let h = rc.clone();
            let _ = parser.add_message(
                wire_messages::jb_matching__flow_end::metadata(),
                Box::new(move |shard, q, ts, buf| h.on_flow_end(shard, q, ts, buf)),
            );
        }
        {
            let h = rc.clone();
            let _ = parser.add_message(
                wire_messages::jb_matching__agent_info::metadata(),
                Box::new(move |shard, q, ts, buf| h.on_agent_info(shard, q, ts, buf)),
            );
        }
        {
            let h = rc.clone();
            let _ = parser.add_message(
                wire_messages::jb_matching__task_info::metadata(),
                Box::new(move |shard, q, ts, buf| h.on_task_info(shard, q, ts, buf)),
            );
        }
        {
            let h = rc.clone();
            let _ = parser.add_message(
                wire_messages::jb_matching__socket_info::metadata(),
                Box::new(move |shard, q, ts, buf| h.on_socket_info(shard, q, ts, buf)),
            );
        }
        {
            let h = rc.clone();
            let _ = parser.add_message(
                wire_messages::jb_matching__k8s_info::metadata(),
                Box::new(move |shard, q, ts, buf| h.on_k8s_info(shard, q, ts, buf)),
            );
        }
        {
            let h = rc.clone();
            let _ = parser.add_message(
                wire_messages::jb_matching__tcp_update::metadata(),
                Box::new(move |shard, q, ts, buf| h.on_tcp_update(shard, q, ts, buf)),
            );
        }
        {
            let h = rc.clone();
            let _ = parser.add_message(
                wire_messages::jb_matching__udp_update::metadata(),
                Box::new(move |shard, q, ts, buf| h.on_udp_update(shard, q, ts, buf)),
            );
        }
        {
            let h = rc.clone();
            let _ = parser.add_message(
                wire_messages::jb_matching__http_update::metadata(),
                Box::new(move |shard, q, ts, buf| h.on_http_update(shard, q, ts, buf)),
            );
        }
        {
            let h = rc.clone();
            let _ = parser.add_message(
                wire_messages::jb_matching__dns_update::metadata(),
                Box::new(move |shard, q, ts, buf| h.on_dns_update(shard, q, ts, buf)),
            );
        }
        {
            let h = rc.clone();
            let _ = parser.add_message(
                wire_messages::jb_matching__container_info::metadata(),
                Box::new(move |shard, q, ts, buf| h.on_container_info(shard, q, ts, buf)),
            );
        }
        {
            let h = rc.clone();
            let _ = parser.add_message(
                wire_messages::jb_matching__service_info::metadata(),
                Box::new(move |shard, q, ts, buf| h.on_service_info(shard, q, ts, buf)),
            );
        }
        {
            let h = rc.clone();
            let _ = parser.add_message(
                wire_messages::jb_matching__aws_enrichment_start::metadata(),
                Box::new(move |shard, q, ts, buf| h.on_aws_enrichment_start(shard, q, ts, buf)),
            );
        }
        {
            let h = rc.clone();
            let _ = parser.add_message(
                wire_messages::jb_matching__aws_enrichment_end::metadata(),
                Box::new(move |shard, q, ts, buf| h.on_aws_enrichment_end(shard, q, ts, buf)),
            );
        }
        {
            let h = rc.clone();
            let _ = parser.add_message(
                wire_messages::jb_matching__aws_enrichment::metadata(),
                Box::new(move |shard, q, ts, buf| h.on_aws_enrichment(shard, q, ts, buf)),
            );
        }
        {
            let h = rc.clone();
            let _ = parser.add_message(
                wire_messages::jb_matching__k8s_pod_start::metadata(),
                Box::new(move |shard, q, ts, buf| h.on_k8s_pod_start(shard, q, ts, buf)),
            );
        }
        {
            let h = rc.clone();
            let _ = parser.add_message(
                wire_messages::jb_matching__k8s_pod_end::metadata(),
                Box::new(move |shard, q, ts, buf| h.on_k8s_pod_end(shard, q, ts, buf)),
            );
        }
        {
            let h = rc.clone();
            let _ = parser.add_message(
                wire_messages::jb_matching__set_pod_detail::metadata(),
                Box::new(move |shard, q, ts, buf| h.on_set_pod_detail(shard, q, ts, buf)),
            );
        }
        {
            let h = rc.clone();
            let _ = parser.add_message(
                wire_messages::jb_matching__k8s_container_start::metadata(),
                Box::new(move |shard, q, ts, buf| h.on_k8s_container_start(shard, q, ts, buf)),
            );
        }
        {
            let h = rc.clone();
            let _ = parser.add_message(
                wire_messages::jb_matching__k8s_container_end::metadata(),
                Box::new(move |shard, q, ts, buf| h.on_k8s_container_end(shard, q, ts, buf)),
            );
        }
        {
            let h = rc.clone();
            let _ = parser.add_message(
                wire_messages::jb_matching__set_container_pod::metadata(),
                Box::new(move |shard, q, ts, buf| h.on_set_container_pod(shard, q, ts, buf)),
            );
        }
        {
            let h = rc.clone();
            let _ = parser.add_message(
                wire_messages::jb_matching__pulse::metadata(),
                Box::new(move |shard, q, ts, buf| h.on_pulse(shard, q, ts, buf)),
            );
        }

        rc
    }

    fn fmt_ipv6_bytes(bytes: &[u8; 16]) -> String {
        // Minimal, codec-agnostic; avoid external deps. Hex string.
        bytes
            .iter()
            .map(|b| format!("{:02x}", b))
            .collect::<Vec<_>>()
            .join(":")
    }

    fn fmt_u128_hex(v: u128) -> String {
        format!("{:032x}", v)
    }

    fn on_flow_start(&self, shard: u32, queue_idx: usize, ts: u64, buf: &[u8]) {
        match parsed_message::flow_start::decode(buf) {
            Ok(m) => println!(
                "match[shard={}] q={} ts={} flow_start ref={} addr1={} port1={} addr2={} port2={}",
                shard,
                queue_idx,
                ts,
                m._ref,
                Self::fmt_u128_hex(m.addr1),
                m.port1,
                Self::fmt_u128_hex(m.addr2),
                m.port2
            ),
            Err(e) => eprintln!("match decode_error flow_start: {:?}", e),
        }
    }

    fn on_flow_end(&self, shard: u32, queue_idx: usize, ts: u64, buf: &[u8]) {
        match parsed_message::flow_end::decode(buf) {
            Ok(m) => println!(
                "match[shard={}] q={} ts={} flow_end ref={}",
                shard, queue_idx, ts, m._ref
            ),
            Err(e) => eprintln!("match decode_error flow_end: {:?}", e),
        }
    }

    fn on_agent_info(&self, shard: u32, queue_idx: usize, ts: u64, buf: &[u8]) {
        match parsed_message::agent_info::decode(buf) {
            Ok(m) => println!(
                "match[shard={}] q={} ts={} agent_info side={} id='{}' az='{}' env='{}' role='{}' ns='{}'",
                shard, queue_idx, ts, m.side, m.id, m.az, m.env, m.role, m.ns
            ),
            Err(e) => eprintln!("match decode_error agent_info: {:?}", e),
        }
    }

    fn on_task_info(&self, shard: u32, queue_idx: usize, ts: u64, buf: &[u8]) {
        match parsed_message::task_info::decode(buf) {
            Ok(m) => println!(
                "match[shard={}] q={} ts={} task_info side={} comm='{}' cgroup='{}'",
                shard, queue_idx, ts, m.side, m.comm, m.cgroup_name
            ),
            Err(e) => eprintln!("match decode_error task_info: {:?}", e),
        }
    }

    fn on_socket_info(&self, shard: u32, queue_idx: usize, ts: u64, buf: &[u8]) {
        match parsed_message::socket_info::decode(buf) {
            Ok(m) => println!(
                "match[shard={}] q={} ts={} socket_info side={} local={}:{}, remote={}:{}, is_connector={}, remote_dns='{}'",
                shard,
                queue_idx,
                ts,
                m.side,
                Self::fmt_ipv6_bytes(&m.local_addr),
                m.local_port,
                Self::fmt_ipv6_bytes(&m.remote_addr),
                m.remote_port,
                m.is_connector,
                m.remote_dns_name
            ),
            Err(e) => eprintln!("match decode_error socket_info: {:?}", e),
        }
    }

    fn on_k8s_info(&self, shard: u32, queue_idx: usize, ts: u64, buf: &[u8]) {
        match parsed_message::k8s_info::decode(buf) {
            Ok(m) => {
                let suffix = m
                    .pod_uid_suffix
                    .iter()
                    .map(|b| format!("{:02x}", b))
                    .collect::<Vec<_>>()
                    .join("");
                println!(
                    "match[shard={}] q={} ts={} k8s_info side={} pod_uid_suffix={} pod_uid_hash={}",
                    shard, queue_idx, ts, m.side, suffix, m.pod_uid_hash
                );
            }
            Err(e) => eprintln!("match decode_error k8s_info: {:?}", e),
        }
    }

    fn on_tcp_update(&self, shard: u32, queue_idx: usize, ts: u64, buf: &[u8]) {
        match parsed_message::tcp_update::decode(buf) {
            Ok(m) => println!(
                "match[shard={}] q={} ts={} tcp_update side={} is_rx={} act={} bytes={} srtt={} delivered={} rtts={} syn_timeouts={} new_sockets={} resets={} retrans={}",
                shard,
                queue_idx,
                ts,
                m.side,
                m.is_rx,
                m.active_sockets,
                m.sum_bytes,
                m.sum_srtt,
                m.sum_delivered,
                m.active_rtts,
                m.syn_timeouts,
                m.new_sockets,
                m.tcp_resets,
                m.sum_retrans
            ),
            Err(e) => eprintln!("match decode_error tcp_update: {:?}", e),
        }
    }

    fn on_udp_update(&self, shard: u32, queue_idx: usize, ts: u64, buf: &[u8]) {
        match parsed_message::udp_update::decode(buf) {
            Ok(m) => println!(
                "match[shard={}] q={} ts={} udp_update side={} is_rx={} act={} addr_changes={} packets={} bytes={} drops={}",
                shard, queue_idx, ts, m.side, m.is_rx, m.active_sockets, m.addr_changes, m.packets, m.bytes, m.drops
            ),
            Err(e) => eprintln!("match decode_error udp_update: {:?}", e),
        }
    }

    fn on_http_update(&self, shard: u32, queue_idx: usize, ts: u64, buf: &[u8]) {
        match parsed_message::http_update::decode(buf) {
            Ok(m) => println!(
                "match[shard={}] q={} ts={} http_update side={} client_server={} act={} 200={} 400={} 500={} other={} total_ns={} proc_ns={}",
                shard,
                queue_idx,
                ts,
                m.side,
                m.client_server,
                m.active_sockets,
                m.sum_code_200,
                m.sum_code_400,
                m.sum_code_500,
                m.sum_code_other,
                m.sum_total_time_ns,
                m.sum_processing_time_ns
            ),
            Err(e) => eprintln!("match decode_error http_update: {:?}", e),
        }
    }

    fn on_dns_update(&self, shard: u32, queue_idx: usize, ts: u64, buf: &[u8]) {
        match parsed_message::dns_update::decode(buf) {
            Ok(m) => println!(
                "match[shard={}] q={} ts={} dns_update side={} client_server={} act={} req_a={} req_aaaa={} resp={} timeouts={} total_ns={} proc_ns={}",
                shard,
                queue_idx,
                ts,
                m.side,
                m.client_server,
                m.active_sockets,
                m.requests_a,
                m.requests_aaaa,
                m.responses,
                m.timeouts,
                m.sum_total_time_ns,
                m.sum_processing_time_ns
            ),
            Err(e) => eprintln!("match decode_error dns_update: {:?}", e),
        }
    }

    fn on_container_info(&self, shard: u32, queue_idx: usize, ts: u64, buf: &[u8]) {
        match parsed_message::container_info::decode(buf) {
            Ok(m) => println!(
                "match[shard={}] q={} ts={} container_info side={} name='{}' pod='{}' role='{}' version='{}' ns='{}' node_type={} ref={}",
                shard,
                queue_idx,
                ts,
                m.side,
                m.name,
                m.pod,
                m.role,
                m.version,
                m.ns,
                m.node_type,
                m._ref
            ),
            Err(e) => eprintln!("match decode_error container_info: {:?}", e),
        }
    }

    fn on_service_info(&self, shard: u32, queue_idx: usize, ts: u64, buf: &[u8]) {
        match parsed_message::service_info::decode(buf) {
            Ok(m) => println!(
                "match[shard={}] q={} ts={} service_info side={} name='{}' ref={}",
                shard, queue_idx, ts, m.side, m.name, m._ref
            ),
            Err(e) => eprintln!("match decode_error service_info: {:?}", e),
        }
    }

    fn on_aws_enrichment_start(&self, shard: u32, queue_idx: usize, ts: u64, buf: &[u8]) {
        match parsed_message::aws_enrichment_start::decode(buf) {
            Ok(m) => println!(
                "match[shard={}] q={} ts={} aws_enrichment_start ip={}",
                shard,
                queue_idx,
                ts,
                Self::fmt_u128_hex(m.ip)
            ),
            Err(e) => eprintln!("match decode_error aws_enrichment_start: {:?}", e),
        }
    }

    fn on_aws_enrichment_end(&self, shard: u32, queue_idx: usize, ts: u64, buf: &[u8]) {
        match parsed_message::aws_enrichment_end::decode(buf) {
            Ok(m) => println!(
                "match[shard={}] q={} ts={} aws_enrichment_end ref={}",
                shard, queue_idx, ts, m._ref
            ),
            Err(e) => eprintln!("match decode_error aws_enrichment_end: {:?}", e),
        }
    }

    fn on_aws_enrichment(&self, shard: u32, queue_idx: usize, ts: u64, buf: &[u8]) {
        match parsed_message::aws_enrichment::decode(buf) {
            Ok(m) => println!(
                "match[shard={}] q={} ts={} aws_enrichment role='{}' az='{}' id='{}' ref={}",
                shard, queue_idx, ts, m.role, m.az, m.id, m._ref
            ),
            Err(e) => eprintln!("match decode_error aws_enrichment: {:?}", e),
        }
    }

    fn on_k8s_pod_start(&self, shard: u32, queue_idx: usize, ts: u64, buf: &[u8]) {
        match parsed_message::k8s_pod_start::decode(buf) {
            Ok(m) => println!(
                "match[shard={}] q={} ts={} k8s_pod_start uid_hash={} ref={}",
                shard, queue_idx, ts, m.uid_hash, m._ref
            ),
            Err(e) => eprintln!("match decode_error k8s_pod_start: {:?}", e),
        }
    }

    fn on_k8s_pod_end(&self, shard: u32, queue_idx: usize, ts: u64, buf: &[u8]) {
        match parsed_message::k8s_pod_end::decode(buf) {
            Ok(m) => println!(
                "match[shard={}] q={} ts={} k8s_pod_end ref={}",
                shard, queue_idx, ts, m._ref
            ),
            Err(e) => eprintln!("match decode_error k8s_pod_end: {:?}", e),
        }
    }

    fn on_set_pod_detail(&self, shard: u32, queue_idx: usize, ts: u64, buf: &[u8]) {
        match parsed_message::set_pod_detail::decode(buf) {
            Ok(m) => println!(
                "match[shard={}] q={} ts={} set_pod_detail owner='{}' pod='{}' ns='{}' version='{}' ref={}",
                shard, queue_idx, ts, m.owner_name, m.pod_name, m.ns, m.version, m._ref
            ),
            Err(e) => eprintln!("match decode_error set_pod_detail: {:?}", e),
        }
    }

    fn on_k8s_container_start(&self, shard: u32, queue_idx: usize, ts: u64, buf: &[u8]) {
        match parsed_message::k8s_container_start::decode(buf) {
            Ok(m) => println!(
                "match[shard={}] q={} ts={} k8s_container_start uid_hash={} ref={}",
                shard, queue_idx, ts, m.uid_hash, m._ref
            ),
            Err(e) => eprintln!("match decode_error k8s_container_start: {:?}", e),
        }
    }

    fn on_k8s_container_end(&self, shard: u32, queue_idx: usize, ts: u64, buf: &[u8]) {
        match parsed_message::k8s_container_end::decode(buf) {
            Ok(m) => println!(
                "match[shard={}] q={} ts={} k8s_container_end ref={}",
                shard, queue_idx, ts, m._ref
            ),
            Err(e) => eprintln!("match decode_error k8s_container_end: {:?}", e),
        }
    }

    fn on_set_container_pod(&self, shard: u32, queue_idx: usize, ts: u64, buf: &[u8]) {
        match parsed_message::set_container_pod::decode(buf) {
            Ok(m) => println!(
                "match[shard={}] q={} ts={} set_container_pod name='{}' pod_uid_hash={} ref={}",
                shard, queue_idx, ts, m.name, m.pod_uid_hash, m._ref
            ),
            Err(e) => eprintln!("match decode_error set_container_pod: {:?}", e),
        }
    }

    fn on_pulse(&self, shard: u32, queue_idx: usize, ts: u64, buf: &[u8]) {
        let _ = buf; // unused content
        println!("match[shard={}] q={} ts={} pulse", shard, queue_idx, ts);
    }
}
