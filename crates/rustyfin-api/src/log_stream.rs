use std::sync::Arc;

use chrono::Utc;
use tokio::sync::broadcast;
use tracing_subscriber::Layer;

use rustyfin_model::ws_event::{LogMessageData, WsEvent};

/// A tracing layer that forwards log events to a WebSocket broadcast channel.
///
/// The minimum log level is configurable at runtime via a watch channel.
pub struct LogStreamLayerInner {
    tx: broadcast::Sender<WsEvent>,
    min_level: Arc<tokio::sync::watch::Receiver<String>>,
}

pub type LogStreamLayer = LogStreamLayerInner;

impl LogStreamLayer {
    pub fn new(
        tx: broadcast::Sender<WsEvent>,
        min_level: Arc<tokio::sync::watch::Receiver<String>>,
    ) -> Self {
        Self { tx, min_level }
    }

    fn should_record(&self, level: tracing::Level) -> bool {
        let min = self.min_level.borrow().clone();
        let min_level = parse_level(&min);
        let event_level = level_to_u8(&level);
        event_level <= min_level
    }
}

fn parse_level(s: &str) -> u8 {
    match s.to_lowercase().as_str() {
        "trace" => 1,
        "debug" => 2,
        "info" => 3,
        "warn" => 4,
        "error" => 5,
        _ => 3, // default: info
    }
}

fn level_to_u8(level: &tracing::Level) -> u8 {
    if *level == tracing::Level::TRACE {
        1
    } else if *level == tracing::Level::DEBUG {
        2
    } else if *level == tracing::Level::INFO {
        3
    } else if *level == tracing::Level::WARN {
        4
    } else {
        5
    }
}

fn level_name(level: &tracing::Level) -> &'static str {
    if *level == tracing::Level::ERROR {
        "ERROR"
    } else if *level == tracing::Level::WARN {
        "WARN"
    } else if *level == tracing::Level::INFO {
        "INFO"
    } else if *level == tracing::Level::DEBUG {
        "DEBUG"
    } else {
        "TRACE"
    }
}

struct MessageVisitor {
    message: Option<String>,
}

impl tracing_core::field::Visit for MessageVisitor {
    fn record_f64(&mut self, field: &tracing_core::field::Field, value: f64) {
        if field.name() == "message" {
            self.message = Some(format!("{:.2}", value));
        }
    }

    fn record_i64(&mut self, field: &tracing_core::field::Field, value: i64) {
        if field.name() == "message" {
            self.message = Some(value.to_string());
        }
    }

    fn record_u64(&mut self, field: &tracing_core::field::Field, value: u64) {
        if field.name() == "message" {
            self.message = Some(value.to_string());
        }
    }

    fn record_bool(&mut self, field: &tracing_core::field::Field, value: bool) {
        if field.name() == "message" {
            self.message = Some(value.to_string());
        }
    }

    fn record_str(&mut self, field: &tracing_core::field::Field, value: &str) {
        if field.name() == "message" {
            self.message = Some(value.to_string());
        }
    }

    fn record_error(&mut self, field: &tracing_core::field::Field, value: &dyn std::error::Error) {
        if field.name() == "message" {
            self.message = Some(value.to_string());
        }
    }

    fn record_debug(&mut self, field: &tracing_core::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.message = Some(format!("{:?}", value));
        }
    }
}

impl<S> Layer<S> for LogStreamLayer
where
    S: tracing::Subscriber,
{
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let meta = event.metadata();
        if !self.should_record(*meta.level()) {
            return;
        }

        let mut visitor = MessageVisitor { message: None };
        event.record(&mut visitor);

        let message = visitor.message.unwrap_or_else(|| meta.target().to_string());
        let target = Some(meta.target().to_string());

        let log_data = LogMessageData {
            timestamp: Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, false),
            level: level_name(meta.level()).to_string(),
            message,
            target,
        };

        let _ = self.tx.send(WsEvent::LogMessage { data: log_data });
    }
}
