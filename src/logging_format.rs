use std::fmt;
use tracing::{Event, Subscriber};
use tracing_subscriber::fmt::{format::Writer, FmtContext, FormatEvent, FormatFields};
use tracing_subscriber::registry::LookupSpan;

pub struct LspGatewayFormat;

impl<S, N> FormatEvent<S, N> for LspGatewayFormat
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        // Write timestamp
        let now = chrono::Local::now();
        write!(
            writer,
            "\x1b[2m{}\x1b[0m ",
            now.format("%Y-%m-%dT%H:%M:%S%.6fZ")
        )?;

        // Get the metadata
        let metadata = event.metadata();

        // Write level with ANSI color codes
        let level = metadata.level();
        let (color_code, level_str) = match *level {
            tracing::Level::ERROR => ("\x1b[1;31m", "ERROR"), // Bold Red
            tracing::Level::WARN => ("\x1b[1;33m", "WARN"),   // Bold Yellow
            tracing::Level::INFO => ("\x1b[1;32m", "INFO"),   // Bold Green
            tracing::Level::DEBUG => ("\x1b[34m", "DEBUG"),   // Blue
            tracing::Level::TRACE => ("\x1b[35m", "TRACE"),   // Magenta
        };
        write!(writer, "{}{}\x1b[0m ", color_code, level_str)?;

        // Write the [LSP-GATEWAY] prefix
        write!(writer, "\x1b[1m[LSP-GATEWAY]\x1b[0m ")?;

        // Write span hierarchy
        if let Some(scope) = ctx.event_scope() {
            let mut span_names = Vec::new();
            for span in scope.from_root() {
                span_names.push(span.name());
            }
            if !span_names.is_empty() {
                write!(writer, "\x1b[1m{}\x1b[0m: ", span_names.join(":"))?;
            }
        }

        // Write target (module path), file and line if available
        let target = metadata.target();
        if let Some(file) = metadata.file() {
            if let Some(line) = metadata.line() {
                write!(writer, "\x1b[2m{} {}:{}:\x1b[0m ", target, file, line)?;
            }
        } else if !target.is_empty() {
            write!(writer, "\x1b[2m{}:\x1b[0m ", target)?;
        }

        // Write the message and fields
        ctx.field_format().format_fields(writer.by_ref(), event)?;

        // Write span fields
        if let Some(scope) = ctx.event_scope() {
            for span in scope.from_root() {
                let extensions = span.extensions();
                if let Some(fields) =
                    extensions.get::<tracing_subscriber::fmt::FormattedFields<N>>()
                {
                    let fields_str = fields.as_str();
                    if !fields_str.is_empty() {
                        write!(writer, " {}", fields_str)?;
                    }
                }
            }
        }

        writeln!(writer)
    }
}
