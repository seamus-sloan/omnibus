//! Tracing subscriber setup: a compact human-readable stderr layer, a
//! non-blocking daily-rolling JSON file sink, and the in-memory error ring
//! buffer layer ([`error_ring_layer`]), all gated by one `RUST_LOG`
//! env-filter. Called by `main` before `dioxus::serve`; the JSON file is the
//! durable log source read back through `omnibus_db::logs`.

mod error_ring_layer;

#[cfg(test)]
mod tests;

use error_ring_layer::ErrorRingLayer;

/// Install the global tracing subscriber. Must run before `dioxus::serve`,
/// which otherwise installs dioxus-logger's default subscriber with a fixed
/// filter that ignores `RUST_LOG`. `RUST_LOG` wins when set; the fallback keeps
/// omnibus events visible without dependency noise.
///
/// Two sinks share one env-filter: a compact human-readable layer to stderr for
/// local dev, and a non-blocking rolling-file layer emitting one JSON record per
/// event for durable, machine-parseable logs. Returns the file writer's
/// [`WorkerGuard`](tracing_appender::non_blocking::WorkerGuard); the caller must
/// hold it for the process lifetime so buffered records flush on shutdown.
/// `None` when the log directory can't be created — stderr logging still comes
/// up so the server isn't blocked on a read-only volume.
pub fn init_tracing() -> Option<tracing_appender::non_blocking::WorkerGuard> {
    use tracing_subscriber::{fmt, prelude::*};

    let filter = resolve_env_filter();

    // Build the rolling-file JSON layer, or fall back to stderr-only if the
    // directory can't be created. `Option<Layer>` is itself a `Layer` (None =
    // no-op), so the registry wiring is identical either way. The directory is
    // owned by `omnibus_db::logs` so the log viewer reads exactly where we
    // write.
    let dir = omnibus_db::logs::log_dir();
    let (file_layer, guard) = match build_file_writer(&dir) {
        Some((writer, guard)) => (Some(fmt::layer().json().with_writer(writer)), Some(guard)),
        None => (None, None),
    };

    // try_init over init: a second subscriber (e.g. in tests) is a no-op, not a
    // panic. A single env-filter gates every layer, including the ring buffer
    // — a DEBUG-only deployment shouldn't have ERROR events silently vanish
    // from the buffer just because the filter dropped them upstream, but nor
    // should the buffer see events the operator asked to suppress.
    tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_writer(std::io::stderr).compact())
        .with(file_layer)
        .with(ErrorRingLayer)
        .try_init()
        .ok();

    guard
}

/// Resolve `RUST_LOG` into an `EnvFilter`, falling back to
/// `"info,omnibus=debug"` when unset or unparsable (a warning is only
/// printed for the unparsable case — an unset var is the expected default
/// path). Split out from `init_tracing` so the fallback logic is testable
/// without installing a global subscriber.
fn resolve_env_filter() -> tracing_subscriber::EnvFilter {
    tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|err| {
        // eprintln because no subscriber exists yet to carry the event.
        if std::env::var_os("RUST_LOG").is_some() {
            eprintln!("invalid RUST_LOG ({err}); falling back to default log filter");
        }
        tracing_subscriber::EnvFilter::new("info,omnibus=debug")
    })
}

/// Build the daily-rolling JSON file writer for `dir`, or `None` if the
/// directory can't be created (e.g. a read-only volume). Split out from
/// `init_tracing` so the dir-creation failure branch is testable without
/// installing a global subscriber.
fn build_file_writer(
    dir: &std::path::Path,
) -> Option<(
    tracing_appender::non_blocking::NonBlocking,
    tracing_appender::non_blocking::WorkerGuard,
)> {
    match std::fs::create_dir_all(dir) {
        Ok(()) => {
            let appender = tracing_appender::rolling::daily(dir, "omnibus.log");
            Some(tracing_appender::non_blocking(appender))
        }
        Err(err) => {
            eprintln!(
                "could not create log dir {}: {err}; on-disk JSON logging disabled",
                dir.display()
            );
            None
        }
    }
}
