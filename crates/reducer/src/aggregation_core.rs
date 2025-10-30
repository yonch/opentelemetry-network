use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use element_queue::ElementQueue;
use render_parser::Parser;
use timeslot::virtual_clock::VirtualClock;

// Render-generated perfect hash and metadata for aggregation
use encoder_ebpf_net_aggregation::hash::{aggregation_hash, AGGREGATION_HASH_SIZE};
use encoder_ebpf_net_aggregation::wire_messages;
use encoder_ebpf_net_aggregation::parsed_message;

// Keep batch size reasonable to avoid starving other work
const K_MAX_RPC_BATCH_PER_QUEUE: usize = 10_000;

type Handler = fn(u32, usize, u64, &[u8]);


pub struct AggregationCore {
    queues: Vec<ElementQueue>,
    clock: VirtualClock,
    parser: Parser<Handler, fn(u32) -> u32>,
    shard: u32,
    stop: Arc<AtomicBool>,
}

impl AggregationCore {
    pub fn new(eq_views: &[(usize, u32, u32)], shard: u32) -> Self {
        // Build queues from contiguous storage descriptors
        let mut queues = Vec::with_capacity(eq_views.len());
        for (data, n_elems, buf_len) in eq_views.iter().cloned() {
            let ptr = data as *mut u8;
            let q = unsafe { ElementQueue::new_from_contiguous(n_elems, buf_len, ptr) }
                .expect("failed to create ElementQueue from contiguous storage");
            queues.push(q);
        }

        // Build parser with render-provided perfect hash
        let hash_size = AGGREGATION_HASH_SIZE as usize;
        let mut parser: Parser<Handler, fn(u32) -> u32> = Parser::new(hash_size, aggregation_hash);
        // Register each render-generated wire message with its handler.
        let _ = parser.add_message(
            wire_messages::jb_aggregation__agg_root_start::metadata(),
            AggregationCore::handle_agg_root_start,
        );
        let _ = parser.add_message(
            wire_messages::jb_aggregation__agg_root_end::metadata(),
            AggregationCore::handle_agg_root_end,
        );
        let _ = parser.add_message(
            wire_messages::jb_aggregation__update_node::metadata(),
            AggregationCore::handle_update_node,
        );
        let _ = parser.add_message(
            wire_messages::jb_aggregation__update_tcp_metrics::metadata(),
            AggregationCore::handle_update_tcp_metrics,
        );
        let _ = parser.add_message(
            wire_messages::jb_aggregation__update_udp_metrics::metadata(),
            AggregationCore::handle_update_udp_metrics,
        );
        let _ = parser.add_message(
            wire_messages::jb_aggregation__update_http_metrics::metadata(),
            AggregationCore::handle_update_http_metrics,
        );
        let _ = parser.add_message(
            wire_messages::jb_aggregation__update_dns_metrics::metadata(),
            AggregationCore::handle_update_dns_metrics,
        );
        let _ = parser.add_message(
            wire_messages::jb_aggregation__pulse::metadata(),
            AggregationCore::handle_pulse,
        );

        // Virtual clock with as many inputs as queues
        let mut clock = VirtualClock::default();
        clock.add_inputs(queues.len());

        Self {
            queues,
            clock,
            parser,
            shard,
            stop: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    pub fn run(&mut self) {
        if self.queues.is_empty() {
            return;
        }
        let mut next_idx: usize = 0;
        // Soft time budget to avoid long stalls; not strictly needed
        let _time_budget = Duration::from_millis(20);

        while !self.stop.load(Ordering::Relaxed) {
            let start_cycle = Instant::now();

            for _ in 0..self.queues.len() {
                let i = next_idx;
                next_idx = (next_idx + 1) % self.queues.len();

                if !self.clock.can_update(i) {
                    continue;
                }

                // RAII read guard
                let rb = self.queues[i].start_read();
                let mut handled_in_queue = 0usize;

                while handled_in_queue < K_MAX_RPC_BATCH_PER_QUEUE
                    && self.clock.can_update(i)
                    && rb.peek_len().is_ok()
                {
                    // Peek timestamp (native-endian u64 at start of element)
                    let ts = match rb.peek_value::<u64>() {
                        Ok(v) => v,
                        Err(_e) => {
                            // Drain malformed element and continue
                            let _ = rb.read();
                            continue;
                        }
                    };

                    // Update clock for this input
                    match self.clock.update(i, ts) {
                        Ok(()) => {}
                        Err(timeslot::virtual_clock::UpdateError::PastTimeslot) => {
                            // Drain and continue
                            let _ = rb.read();
                            continue;
                        }
                        Err(timeslot::virtual_clock::UpdateError::NotPermitted) => {
                            break;
                        }
                    }

                    if self.clock.is_current(i) {
                        match rb.read() {
                            Ok(bytes) => {
                                // Parser expects the full buffer: ts(8) + message
                                match self.parser.handle(bytes) {
                                    Ok(ok) => {
                                        (ok.value)(self.shard, i, ok.timestamp, ok.message);
                                    }
                                    Err(e) => {
                                        println!(
                                            "agg[shard={}] eq={} ts={} parse_error={:?}",
                                            self.shard, i, ts, e
                                        );
                                    }
                                }
                            }
                            Err(_e) => break,
                        }
                        handled_in_queue += 1;
                    }

                    if start_cycle.elapsed() >= _time_budget {
                        break; // yield and rotate queues
                    }
                }

                // Publish read heads
                let _ = rb.finish();
            }

            if self.clock.advance() {
                println!("agg[shard={}] timeslot_advanced", self.shard);
            }
        }
    }

    fn handle_agg_root_start(shard: u32, queue_idx: usize, timestamp: u64, buf: &[u8]) {
        match parsed_message::agg_root_start::decode(buf) {
            Ok(msg) => {
                println!(
                    "agg[shard={}] eq={} ts={} rpc_id={} agg_root_start ref={}",
                    shard, queue_idx, timestamp, msg._rpc_id, msg._ref
                );
            }
            Err(e) => Self::log_decode_error(shard, "agg_root_start", queue_idx, timestamp, e),
        }
    }

    fn handle_agg_root_end(shard: u32, queue_idx: usize, timestamp: u64, buf: &[u8]) {
        match parsed_message::agg_root_end::decode(buf) {
            Ok(msg) => {
                println!(
                    "agg[shard={}] eq={} ts={} rpc_id={} agg_root_end ref={}",
                    shard, queue_idx, timestamp, msg._rpc_id, msg._ref
                );
            }
            Err(e) => Self::log_decode_error(shard, "agg_root_end", queue_idx, timestamp, e),
        }
    }

    fn handle_update_node(shard: u32, queue_idx: usize, timestamp: u64, buf: &[u8]) {
        match parsed_message::update_node::decode(buf) {
            Ok(msg) => {
                println!(
                    "agg[shard={}] eq={} ts={} rpc_id={} update_node ref={} id=\"{}\" az=\"{}\" role=\"{}\" version=\"{}\" env=\"{}\" ns=\"{}\" address=\"{}\" process=\"{}\" container=\"{}\" pod_name=\"{}\" side={} node_type={}",
                    shard,
                    queue_idx,
                    timestamp,
                    msg._rpc_id,
                    msg._ref,
                    msg.id,
                    msg.az,
                    msg.role,
                    msg.version,
                    msg.env,
                    msg.ns,
                    msg.address,
                    msg.process,
                    msg.container,
                    msg.pod_name,
                    msg.side,
                    msg.node_type
                );
            }
            Err(e) => Self::log_decode_error(shard, "update_node", queue_idx, timestamp, e),
        }
    }

    fn handle_update_tcp_metrics(shard: u32, queue_idx: usize, timestamp: u64, buf: &[u8]) {
        match parsed_message::update_tcp_metrics::decode(buf) {
            Ok(msg) => {
                println!(
                    "agg[shard={}] eq={} ts={} rpc_id={} update_tcp_metrics direction={} active_sockets={} sum_bytes={} sum_srtt={} sum_delivered={} ref={} sum_retrans={} active_rtts={} syn_timeouts={} new_sockets={} tcp_resets={}",
                    shard,
                    queue_idx,
                    timestamp,
                    msg._rpc_id,
                    msg.direction,
                    msg.active_sockets,
                    msg.sum_bytes,
                    msg.sum_srtt,
                    msg.sum_delivered,
                    msg._ref,
                    msg.sum_retrans,
                    msg.active_rtts,
                    msg.syn_timeouts,
                    msg.new_sockets,
                    msg.tcp_resets
                );
            }
            Err(e) => Self::log_decode_error(shard, "update_tcp_metrics", queue_idx, timestamp, e),
        }
    }

    fn handle_update_udp_metrics(shard: u32, queue_idx: usize, timestamp: u64, buf: &[u8]) {
        match parsed_message::update_udp_metrics::decode(buf) {
            Ok(msg) => {
                println!(
                    "agg[shard={}] eq={} ts={} rpc_id={} update_udp_metrics direction={} active_sockets={} bytes={} ref={} addr_changes={} packets={} drops={}",
                    shard,
                    queue_idx,
                    timestamp,
                    msg._rpc_id,
                    msg.direction,
                    msg.active_sockets,
                    msg.bytes,
                    msg._ref,
                    msg.addr_changes,
                    msg.packets,
                    msg.drops
                );
            }
            Err(e) => Self::log_decode_error(shard, "update_udp_metrics", queue_idx, timestamp, e),
        }
    }

    fn handle_update_http_metrics(shard: u32, queue_idx: usize, timestamp: u64, buf: &[u8]) {
        match parsed_message::update_http_metrics::decode(buf) {
            Ok(msg) => {
                println!(
                    "agg[shard={}] eq={} ts={} rpc_id={} update_http_metrics direction={} active_sockets={} sum_total_time_ns={} sum_processing_time_ns={} ref={} sum_code_200={} sum_code_400={} sum_code_500={} sum_code_other={}",
                    shard,
                    queue_idx,
                    timestamp,
                    msg._rpc_id,
                    msg.direction,
                    msg.active_sockets,
                    msg.sum_total_time_ns,
                    msg.sum_processing_time_ns,
                    msg._ref,
                    msg.sum_code_200,
                    msg.sum_code_400,
                    msg.sum_code_500,
                    msg.sum_code_other
                );
            }
            Err(e) => Self::log_decode_error(shard, "update_http_metrics", queue_idx, timestamp, e),
        }
    }

    fn handle_update_dns_metrics(shard: u32, queue_idx: usize, timestamp: u64, buf: &[u8]) {
        match parsed_message::update_dns_metrics::decode(buf) {
            Ok(msg) => {
                println!(
                    "agg[shard={}] eq={} ts={} rpc_id={} update_dns_metrics direction={} active_sockets={} sum_total_time_ns={} sum_processing_time_ns={} ref={} requests_a={} requests_aaaa={} responses={} timeouts={}",
                    shard,
                    queue_idx,
                    timestamp,
                    msg._rpc_id,
                    msg.direction,
                    msg.active_sockets,
                    msg.sum_total_time_ns,
                    msg.sum_processing_time_ns,
                    msg._ref,
                    msg.requests_a,
                    msg.requests_aaaa,
                    msg.responses,
                    msg.timeouts
                );
            }
            Err(e) => Self::log_decode_error(shard, "update_dns_metrics", queue_idx, timestamp, e),
        }
    }

    fn handle_pulse(shard: u32, queue_idx: usize, timestamp: u64, buf: &[u8]) {
        match parsed_message::pulse::decode(buf) {
            Ok(msg) => {
                println!(
                    "agg[shard={}] eq={} ts={} rpc_id={} pulse",
                    shard, queue_idx, timestamp, msg._rpc_id
                );
            }
            Err(e) => Self::log_decode_error(shard, "pulse", queue_idx, timestamp, e),
        }
    }

    fn log_decode_error(
        shard: u32,
        message_name: &str,
        queue_idx: usize,
        timestamp: u64,
        err: parsed_message::DecodeError,
    ) {
        println!(
            "agg[shard={}] eq={} ts={} message={} decode_error={:?}",
            shard, queue_idx, timestamp, message_name, err
        );
    }

}
