use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use render_parser::Parser;

use crate::matching_message_handler::MatchingMessageHandler;
use crate::queue_handler::QueueHandler;
use encoder_ebpf_net_matching::hash::{matching_hash, MATCHING_HASH_SIZE};

type Handler = Box<dyn Fn(u32, usize, u64, &[u8]) + 'static>;

pub struct MatchingCore {
    queue_handler: QueueHandler,
    parser: Parser<Handler, fn(u32) -> u32>,
    shard: u32,
    stop: Arc<AtomicBool>,
    _handler: std::rc::Rc<MatchingMessageHandler>,
}

impl MatchingCore {
    pub fn new(eq_views: &[(usize, u32, u32)], shard: u32) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let queue_handler = QueueHandler::new_from_views(eq_views, stop.clone());

        let hash_size = MATCHING_HASH_SIZE as usize;
        let mut parser: Parser<Handler, fn(u32) -> u32> = Parser::new(hash_size, matching_hash);
        let handler = MatchingMessageHandler::new(&mut parser);

        Self {
            queue_handler,
            parser,
            shard,
            stop,
            _handler: handler,
        }
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    pub fn run(&mut self) {
        if self.queue_handler.is_empty() {
            return;
        }

        let shard = self.shard;
        let parser = &self.parser;

        self.queue_handler.run(
            move |queue_idx, bytes| match parser.handle(bytes) {
                Ok(ok) => (ok.value)(shard, queue_idx, ok.timestamp, ok.message),
                Err(e) => {
                    println!(
                        "match[shard={}] eq={} parse_error={:?}",
                        shard, queue_idx, e
                    );
                }
            },
            move |_window_end_ns| {
                // Matching skeleton: nothing to flush; pulses already printed.
            },
        );
    }
}
