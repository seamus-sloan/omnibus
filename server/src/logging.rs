//! Tracing subscriber setup: a compact human-readable stderr layer, a
//! non-blocking daily-rolling JSON file sink, and the in-memory error ring
//! buffer layer ([`error_ring_layer`]), all gated by one `RUST_LOG`
//! env-filter. Called by `main` before `dioxus::serve`; the JSON file is the
//! durable log source read back through `omnibus_db::logs`.

mod error_ring_layer;

#[cfg(test)]
mod tests;

use std::path::Path;

use tracing_appender::non_blocking::{NonBlocking, WorkerGuard};
use tracing_subscriber::EnvFilter;

use error_ring_layer::ErrorRingLayer;

/// Filter used when `RUST_LOG` is unset or unparsable: omnibus events stay
/// visible without the dependency noise a bare `debug` would let through.
const DEFAULT_FILTER: &str = "info,omnibus=debug";

/// The one env-filter every layer shares. A set-and-parsable `RUST_LOG` wins;
/// anything else falls back to [`DEFAULT_FILTER`].
fn env_filter() -> EnvFilter {
    EnvFilter::try_from_default_env().unwrap_or_else(|err| {
        // try_from_default_env also errs when RUST_LOG is simply unset — only an
        // actually-set-but-unparsable value deserves a warning. eprintln because
        // no subscriber exists yet to carry the event.
        if std::env::var_os("RUST_LOG").is_some() {
            eprintln!("invalid RUST_LOG ({err}); falling back to default log filter");
        }
        EnvFilter::new(DEFAULT_FILTER)
    })
}

/// Create `dir` and open the daily-rolling appender inside it, returning its
/// non-blocking writer and flush guard. `None` when the directory can't be
/// created, so stderr logging still comes up on a read-only volume.
fn rolling_writer(dir: &Path) -> Option<(NonBlocking, WorkerGuard)> {
    if let Err(err) = std::fs::create_dir_all(dir) {
        eprintln!(
            "could not create log dir {}: {err}; on-disk JSON logging disabled",
            dir.display()
        );
        return None;
    }
    Some(tracing_appender::non_blocking(
        tracing_appender::rolling::daily(dir, "omnibus.log"),
    ))
}

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
pub fn init_tracing() -> Option<WorkerGuard> {
    use tracing_subscriber::{fmt, prelude::*};

    // Build the rolling-file JSON layer, or fall back to stderr-only if the
    // directory can't be created. `Option<Layer>` is itself a `Layer` (None =
    // no-op), so the registry wiring is identical either way. The directory is
    // owned by `omnibus_db::logs` so the log viewer reads exactly where we
    // write.
    let (file_layer, guard) = match rolling_writer(&omnibus_db::logs::log_dir()) {
        Some((writer, guard)) => (Some(fmt::layer().json().with_writer(writer)), Some(guard)),
        None => (None, None),
    };

    // try_init over init: a second subscriber (e.g. in tests) is a no-op, not a
    // panic. A single env-filter gates every layer, including the ring buffer
    // — a DEBUG-only deployment shouldn't have ERROR events silently vanish
    // from the buffer just because the filter dropped them upstream, but nor
    // should the buffer see events the operator asked to suppress.
    tracing_subscriber::registry()
        .with(env_filter())
        .with(fmt::layer().with_writer(std::io::stderr).compact())
        .with(file_layer)
        .with(ErrorRingLayer)
        .try_init()
        .ok();

    guard
}
