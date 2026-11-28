//! Fan-out of received syslog messages to reaction streams.

use std::{
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use remoc::rch::mpsc::TrySendError;

use crate::{
    config::{Framing, PluginOptions, TimestampMode},
    lines::to_text,
    log::LogThrottle,
    message,
    plugin::Sink,
};

#[derive(Clone)]
pub struct Dispatcher {
    options: PluginOptions,
    sinks: Arc<Vec<Sink>>,
    warn: Arc<LogThrottle>,
}

impl Dispatcher {
    pub fn new(options: PluginOptions, sinks: Vec<Sink>) -> Self {
        Self {
            options,
            sinks: Arc::new(sinks),
            warn: Arc::new(LogThrottle::default()),
        }
    }

    pub fn max_datagram_bytes(&self) -> usize {
        self.options.max_datagram_bytes
    }

    pub fn max_line_bytes(&self) -> usize {
        self.options.max_line_bytes
    }

    pub fn reconnect_delay(&self) -> Duration {
        self.options.reconnect_delay
    }

    pub fn framing(&self) -> Framing {
        self.options.framing
    }

    pub fn max_tcp_connections(&self) -> usize {
        self.options.max_tcp_connections
    }

    pub fn tcp_idle_timeout(&self) -> Duration {
        self.options.tcp_idle_timeout
    }

    pub fn warn_throttle(&self) -> Arc<LogThrottle> {
        Arc::clone(&self.warn)
    }

    pub fn dispatch_bytes(&self, data: &[u8], source_addr: Option<String>) {
        if data.len() > self.options.max_datagram_bytes {
            log_warn_throttled!(
                self.warn,
                "oversize_datagram",
                "datagram of {} bytes exceeds max_datagram_bytes, dropping",
                data.len()
            );
            return;
        }
        self.dispatch_tcp_frame(data, source_addr);
    }

    /// Dispatch one reassembled TCP frame. TCP lines are bounded by
    /// `max_line_bytes` only, not by the UDP datagram buffer size.
    pub fn dispatch_tcp_frame(&self, data: &[u8], source_addr: Option<String>) {
        self.dispatch_text(to_text(data), source_addr);
    }

    pub fn dispatch_text(&self, text: String, source_addr: Option<String>) {
        if text.len() > self.options.max_line_bytes {
            log_warn_throttled!(
                self.warn,
                "oversize_line",
                "line of {} bytes exceeds max_line_bytes, dropping",
                text.len()
            );
            return;
        }
        let parsed = message::parse(&text, self.options.parse);
        let mut meta = parsed.meta;
        meta.source_addr = source_addr;
        let receive = now();
        let time = match self.options.timestamp {
            TimestampMode::Receive => receive,
            TimestampMode::Message => {
                match message_time(parsed.timestamp, receive, self.options.max_timestamp_skew) {
                    Some(timestamp) => timestamp,
                    None => {
                        if parsed.timestamp.is_some() {
                            log_warn_throttled!(
                                self.warn,
                                "timestamp_skew",
                                "message timestamp is outside the allowed skew, using receive time"
                            );
                        }
                        receive
                    }
                }
            }
        };

        for sink in self.sinks.iter() {
            if !sink.options.select.matches(&meta) {
                continue;
            }
            let line = format!("{}{}", sink.template.render(&meta), parsed.text);
            if line.len() > self.options.max_line_bytes {
                log_warn_throttled!(
                    self.warn,
                    "oversize_rendered_line",
                    "stream {}: rendered line exceeds max_line_bytes, dropping",
                    sink.stream_name
                );
                continue;
            }
            match sink.tx.try_send((line, time)) {
                Ok(_) => {}
                Err(TrySendError::Full(_)) => {
                    log_warn_throttled!(
                        self.warn,
                        "channel_full",
                        "stream {}: channel full, dropping line",
                        sink.stream_name
                    );
                }
                Err(TrySendError::Closed(_)) => {
                    log_error_throttled!(
                        self.warn,
                        "channel_closed",
                        "stream {}: channel closed",
                        sink.stream_name
                    );
                }
                Err(err) => {
                    log_error_throttled!(
                        self.warn,
                        "send_error",
                        "stream {}: cannot send line: {err}",
                        sink.stream_name
                    );
                }
            }
        }
    }
}

fn message_time(
    parsed: Option<Duration>,
    receive: Duration,
    max_skew: Duration,
) -> Option<Duration> {
    parsed.filter(|timestamp| {
        *timestamp >= receive.saturating_sub(max_skew)
            && *timestamp <= receive.saturating_add(max_skew)
    })
}

fn now() -> Duration {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_time_accepts_only_nearby_timestamps() {
        let receive = Duration::from_secs(1000);
        let skew = Duration::from_secs(5);
        assert_eq!(
            message_time(Some(Duration::from_secs(1003)), receive, skew),
            Some(Duration::from_secs(1003))
        );
        assert_eq!(
            message_time(Some(Duration::from_secs(1010)), receive, skew),
            None
        );
        assert_eq!(
            message_time(Some(Duration::from_secs(990)), receive, skew),
            None
        );
        assert_eq!(message_time(None, receive, skew), None);
    }
}
