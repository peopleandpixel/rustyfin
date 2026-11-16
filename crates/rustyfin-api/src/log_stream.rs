use std::collections::VecDeque;
use std::fmt::Write as _;
use std::sync::{Arc, Mutex};

use chrono::Utc;
use tokio::sync::broadcast;
use tracing::field::{Field, Visit};
use tracing_subscriber::Layer;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::layer::Context;
use tracing_subscriber::registry::LookupSpan;

use rustyfin_model::ws_event::LogMessageData;

/// The lines the live log keeps for a view that connects later, unless `RUSTYFIN_LOG_HISTORY` sets
/// another count.
pub const DEFAULT_LOG_HISTORY: usize = 1000;

/// The most lines `RUSTYFIN_LOG_HISTORY` may keep.
pub const MAX_LOG_HISTORY: usize = 100_000;

/// The longest message a line keeps, in bytes; a longer one is cut at a character boundary and
/// ends in [`TRUNCATED`].
pub const MAX_MESSAGE_BYTES: usize = 8 * 1024;

/// The end of a message cut at [`MAX_MESSAGE_BYTES`].
pub const TRUNCATED: &str = "…(truncated)";

/// The live lines a view may fall behind by before it misses some; it is then told how many.
const LIVE_CAPACITY: usize = 1024;

/// The server's recent log lines and the channel that carries new ones to the live log views.
///
/// It has its own channel, apart from the WebSocket events, so a burst of other events never
/// pushes log lines out of a view's queue.
#[derive(Clone)]
pub struct LogHistory {
    ring: Arc<Mutex<Ring>>,
    capacity: usize,
    tx: broadcast::Sender<LogMessageData>,
}

struct Ring {
    lines: VecDeque<LogMessageData>,
    /// The next line's `Seq`
    next_seq: u64,
}

impl LogHistory {
    /// A history of up to `capacity` lines, at most [`MAX_LOG_HISTORY`].
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(LIVE_CAPACITY);
        Self {
            ring: Arc::new(Mutex::new(Ring {
                lines: VecDeque::new(),
                // Starts at the time in microseconds, so a restarted server's lines still number
                // after the ones a view saw before the restart.
                next_seq: Utc::now().timestamp_micros().max(0) as u64,
            })),
            capacity: capacity.min(MAX_LOG_HISTORY),
            tx,
        }
    }

    /// Numbers `line`, cuts its message to [`MAX_MESSAGE_BYTES`], keeps it in the history and
    /// sends it to the connected views.
    pub fn push(&self, mut line: LogMessageData) {
        truncate_message(&mut line.message);
        let mut ring = self.ring.lock().unwrap_or_else(|e| e.into_inner());
        line.seq = Some(ring.next_seq);
        ring.next_seq += 1;
        if self.capacity > 0 {
            if ring.lines.len() == self.capacity {
                ring.lines.pop_front();
            }
            ring.lines.push_back(line.clone());
        }
        // Sent under the lock, so that `subscribe` sees each line in its history or on its
        // receiver, never in both and never in neither.
        let _ = self.tx.send(line);
    }

    /// The lines kept so far, oldest first, and a receiver for the lines after them.
    pub fn subscribe(&self) -> (Vec<LogMessageData>, broadcast::Receiver<LogMessageData>) {
        let ring = self.ring.lock().unwrap_or_else(|e| e.into_inner());
        (ring.lines.iter().cloned().collect(), self.tx.subscribe())
    }
}

/// Cuts `message` to [`MAX_MESSAGE_BYTES`] at a character boundary and marks the cut.
fn truncate_message(message: &mut String) {
    if message.len() <= MAX_MESSAGE_BYTES {
        return;
    }
    let mut end = MAX_MESSAGE_BYTES;
    while !message.is_char_boundary(end) {
        end -= 1;
    }
    message.truncate(end);
    message.push_str(TRUNCATED);
}

/// The history size `RUSTYFIN_LOG_HISTORY` asks for, clamped to [`MAX_LOG_HISTORY`], and a
/// warning to log when the value is not a number or is clamped.
pub fn history_size(value: Option<&str>) -> (usize, Option<String>) {
    let Some(value) = value else {
        return (DEFAULT_LOG_HISTORY, None);
    };
    match value.trim().parse::<usize>() {
        Ok(n) if n > MAX_LOG_HISTORY => (
            MAX_LOG_HISTORY,
            Some(format!(
                "RUSTYFIN_LOG_HISTORY={n} is more than {MAX_LOG_HISTORY}; keeping {MAX_LOG_HISTORY} log lines"
            )),
        ),
        Ok(n) => (n, None),
        Err(_) => (
            DEFAULT_LOG_HISTORY,
            Some(format!(
                "RUSTYFIN_LOG_HISTORY={value:?} is not a number of lines; keeping {DEFAULT_LOG_HISTORY}"
            )),
        ),
    }
}

impl Default for LogHistory {
    fn default() -> Self {
        Self::new(DEFAULT_LOG_HISTORY)
    }
}

/// The line a view gets in place of the `count` lines it fell too far behind to receive.
pub fn skipped_line(count: u64) -> LogMessageData {
    LogMessageData {
        timestamp: now(),
        level: "WARN".to_string(),
        message: format!("{count} log lines skipped: the live log fell behind"),
        target: Some(module_path!().to_string()),
        seq: None,
    }
}

/// The next line for a view: a new line, or one [`skipped_line`] when it fell behind. `None` once
/// the channel is closed.
pub async fn next_line(rx: &mut broadcast::Receiver<LogMessageData>) -> Option<LogMessageData> {
    match rx.recv().await {
        Ok(line) => Some(line),
        Err(broadcast::error::RecvError::Lagged(count)) => Some(skipped_line(count)),
        Err(broadcast::error::RecvError::Closed) => None,
    }
}

/// The live log's level name for the most verbose level the console filter lets through.
pub fn level_name_for(hint: Option<LevelFilter>) -> &'static str {
    match hint {
        None => "trace",
        Some(LevelFilter::TRACE) => "trace",
        Some(LevelFilter::DEBUG) => "debug",
        Some(LevelFilter::INFO) => "info",
        Some(LevelFilter::WARN) => "warn",
        Some(_) => "error",
    }
}

/// A tracing layer that keeps every log event the server prints in a [`LogHistory`].
///
/// It sits beside the console formatter, under the same `RUST_LOG` filter, so it gets the same
/// events, and it writes them the same way: the span scope with its fields, the message, then
/// each field as `key=value`. The live log view filters by level itself.
pub struct LogStreamLayer {
    history: LogHistory,
}

impl LogStreamLayer {
    pub fn new(history: LogHistory) -> Self {
        Self { history }
    }
}

fn now() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, false)
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

/// Writes fields as tracing-subscriber's default formatter does: the message as is, and every
/// other field as `key=value`, separated by spaces, with strings quoted.
#[derive(Default)]
struct FieldsVisitor {
    message: Option<String>,
    fields: String,
}

impl FieldsVisitor {
    fn field(&mut self, name: &str, value: std::fmt::Arguments<'_>) {
        // The `log` crate's bridge records its own metadata as `log.*` fields.
        if name.starts_with("log.") {
            return;
        }
        if !self.fields.is_empty() {
            self.fields.push(' ');
        }
        let name = name.strip_prefix("r#").unwrap_or(name);
        let _ = write!(self.fields, "{name}={value}");
    }

    /// The message and then the fields.
    fn line(self) -> String {
        match self.message {
            Some(message) if self.fields.is_empty() => message,
            Some(message) => format!("{message} {}", self.fields),
            None => self.fields,
        }
    }
}

impl Visit for FieldsVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message = Some(value.to_string());
        } else {
            self.field(field.name(), format_args!("{value:?}"));
        }
    }

    fn record_error(&mut self, field: &Field, value: &(dyn std::error::Error + 'static)) {
        self.field(field.name(), format_args!("{value}"));
        let mut sources = Vec::new();
        let mut source = value.source();
        while let Some(s) = source {
            sources.push(s.to_string());
            source = s.source();
        }
        if !sources.is_empty() {
            let name = format!("{}.sources", field.name());
            self.field(&name, format_args!("[{}]", sources.join(", ")));
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.message = Some(format!("{value:?}"));
        } else {
            self.field(field.name(), format_args!("{value:?}"));
        }
    }
}

/// A span's fields as written by [`FieldsVisitor`], kept in the span's extensions.
struct SpanFields(String);

impl<S> Layer<S> for LogStreamLayer
where
    S: tracing::Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(
        &self,
        attrs: &tracing::span::Attributes<'_>,
        id: &tracing::span::Id,
        ctx: Context<'_, S>,
    ) {
        let Some(span) = ctx.span(id) else { return };
        let mut visitor = FieldsVisitor::default();
        attrs.record(&mut visitor);
        span.extensions_mut().insert(SpanFields(visitor.line()));
    }

    fn on_record(
        &self,
        id: &tracing::span::Id,
        values: &tracing::span::Record<'_>,
        ctx: Context<'_, S>,
    ) {
        let Some(span) = ctx.span(id) else { return };
        let mut visitor = FieldsVisitor::default();
        values.record(&mut visitor);
        let added = visitor.line();
        if added.is_empty() {
            return;
        }
        let mut extensions = span.extensions_mut();
        match extensions.get_mut::<SpanFields>() {
            Some(SpanFields(fields)) if !fields.is_empty() => {
                fields.push(' ');
                fields.push_str(&added);
            }
            Some(SpanFields(fields)) => *fields = added,
            None => extensions.insert(SpanFields(added)),
        }
    }

    fn on_event(&self, event: &tracing::Event<'_>, ctx: Context<'_, S>) {
        let meta = event.metadata();

        // The span scope as the console writes it: `outer{a=1}:inner: `
        let mut message = String::new();
        if let Some(scope) = ctx.event_scope(event) {
            for span in scope.from_root() {
                message.push_str(span.name());
                if let Some(SpanFields(fields)) = span.extensions().get::<SpanFields>()
                    && !fields.is_empty()
                {
                    let _ = write!(message, "{{{fields}}}");
                }
                message.push(':');
            }
            if !message.is_empty() {
                message.push(' ');
            }
        }

        let mut visitor = FieldsVisitor::default();
        event.record(&mut visitor);
        message.push_str(&visitor.line());

        self.history.push(LogMessageData {
            timestamp: now(),
            level: level_name(meta.level()).to_string(),
            message,
            target: Some(meta.target().to_string()),
            seq: None,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing_subscriber::layer::SubscriberExt;

    /// Runs `f` with a subscriber that has only the layer, and returns the history it filled.
    fn capture(capacity: usize, f: impl FnOnce()) -> LogHistory {
        let history = LogHistory::new(capacity);
        let subscriber = tracing_subscriber::registry().with(LogStreamLayer::new(history.clone()));
        tracing::subscriber::with_default(subscriber, f);
        history
    }

    fn messages(history: &LogHistory) -> Vec<String> {
        history
            .subscribe()
            .0
            .into_iter()
            .map(|l| l.message)
            .collect()
    }

    #[derive(Debug)]
    struct Outer;
    impl std::fmt::Display for Outer {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("disk full")
        }
    }
    impl std::error::Error for Outer {}

    #[test]
    fn r3livelog_event_fields_follow_the_message() {
        let history = capture(10, || {
            let e = Outer;
            tracing::warn!(task = 7, name = "scan", ok = false, error = %e, "could not finish");
        });
        let (lines, _) = history.subscribe();
        assert_eq!(
            lines[0].message,
            r#"could not finish task=7 name="scan" ok=false error=disk full"#
        );
        assert_eq!(lines[0].level, "WARN");
        assert_eq!(lines[0].target.as_deref(), Some(module_path!()));
    }

    #[test]
    fn r3livelog_span_fields_prefix_the_line_as_on_the_console() {
        let history = capture(10, || {
            let outer = tracing::info_span!("scan", library = "Movies");
            let _o = outer.enter();
            let inner = tracing::info_span!("probe", file = tracing::field::Empty);
            inner.record("file", "a.mkv");
            let _i = inner.enter();
            tracing::info!(count = 2, "found");
            let bare = tracing::info_span!("bare");
            let _b = bare.enter();
            tracing::info!("in a bare span");
        });
        assert_eq!(
            messages(&history),
            vec![
                r#"scan{library="Movies"}:probe{file="a.mkv"}: found count=2"#,
                r#"scan{library="Movies"}:probe{file="a.mkv"}:bare: in a bare span"#,
            ]
        );
    }

    #[test]
    fn r3livelog_history_keeps_the_newest_lines_and_replays_them_before_live_ones() {
        let history = capture(3, || {
            for n in 0..5 {
                tracing::info!("line {n}");
            }
        });
        assert_eq!(messages(&history), vec!["line 2", "line 3", "line 4"]);

        let (replay, mut rx) = history.subscribe();
        assert_eq!(replay.len(), 3);
        let subscriber = tracing_subscriber::registry().with(LogStreamLayer::new(history.clone()));
        tracing::subscriber::with_default(subscriber, || tracing::info!("line 5"));
        assert_eq!(rx.try_recv().unwrap().message, "line 5");
        assert!(rx.try_recv().is_err(), "a replayed line is not sent again");
    }

    #[tokio::test]
    async fn r3livelog_a_receiver_that_falls_behind_is_told_how_many_lines_it_missed() {
        let history = LogHistory::new(0);
        let (_, mut rx) = history.subscribe();
        let subscriber = tracing_subscriber::registry().with(LogStreamLayer::new(history.clone()));
        tracing::subscriber::with_default(subscriber, || {
            for n in 0..LIVE_CAPACITY + 5 {
                tracing::info!("line {n}");
            }
        });
        let skipped = next_line(&mut rx).await.unwrap();
        assert_eq!(
            skipped.message,
            "5 log lines skipped: the live log fell behind"
        );
        assert_eq!(skipped.level, "WARN");
        assert_eq!(next_line(&mut rx).await.unwrap().message, "line 5");
    }

    #[test]
    fn r3livelog_default_level_is_the_console_filters_most_verbose_level() {
        assert_eq!(level_name_for(Some(LevelFilter::DEBUG)), "debug");
        assert_eq!(level_name_for(Some(LevelFilter::INFO)), "info");
        assert_eq!(level_name_for(Some(LevelFilter::OFF)), "error");
        assert_eq!(level_name_for(None), "trace");
        let filter = tracing_subscriber::EnvFilter::new("warn,rustyfin_core=debug");
        assert_eq!(level_name_for(filter.max_level_hint()), "debug");
    }

    #[test]
    fn r3livelog_lines_are_numbered_in_order_and_long_messages_cut() {
        let history = LogHistory::new(10);
        let long = "é".repeat(MAX_MESSAGE_BYTES); // two bytes each
        let subscriber = tracing_subscriber::registry().with(LogStreamLayer::new(history.clone()));
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!("short");
            tracing::info!("{long}");
        });
        let (lines, _) = history.subscribe();
        let (first, second) = (lines[0].seq.unwrap(), lines[1].seq.unwrap());
        assert_eq!(second, first + 1);
        assert_eq!(lines[0].message, "short");
        let cut = &lines[1].message;
        assert!(cut.ends_with(TRUNCATED), "{}", &cut[cut.len() - 20..]);
        assert_eq!(cut.len(), MAX_MESSAGE_BYTES + TRUNCATED.len());
        assert!(cut.starts_with("éé"));
    }

    #[test]
    fn r3livelog_history_size_is_clamped_and_a_bad_value_warns() {
        assert_eq!(history_size(None), (DEFAULT_LOG_HISTORY, None));
        assert_eq!(history_size(Some("250")), (250, None));
        assert_eq!(history_size(Some("0")), (0, None));
        let (n, warning) = history_size(Some("5000000"));
        assert_eq!(n, MAX_LOG_HISTORY);
        assert!(warning.unwrap().contains("100000"));
        let (n, warning) = history_size(Some("lots"));
        assert_eq!(n, DEFAULT_LOG_HISTORY);
        assert!(warning.unwrap().contains("\"lots\""));
        assert_eq!(LogHistory::new(usize::MAX).capacity, MAX_LOG_HISTORY);
    }
}
