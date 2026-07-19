//! Tracing subscriber setup: a compact human-readable stderr layer plus a
//! non-blocking, daily-rolling JSON file sink, both gated by one `RUST_LOG`
//! env-filter. Called by `main` before `dioxus::serve`; the JSON file is the
//! durable log source for the admin log viewer, which reads it back through
//! `omnibus_db::logs` (the single owner of the log-directory resolution).

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
    use tracing_subscriber::{fmt, prelude::*, EnvFilter};

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|err| {
        // try_from_default_env also errs when RUST_LOG is simply unset — only an
        // actually-set-but-unparsable value deserves a warning. eprintln because
        // no subscriber exists yet to carry the event.
        if std::env::var_os("RUST_LOG").is_some() {
            eprintln!("invalid RUST_LOG ({err}); falling back to default log filter");
        }
        EnvFilter::new("info,omnibus=debug")
    });

    // Build the rolling-file JSON layer, or fall back to stderr-only if the
    // directory can't be created. `Option<Layer>` is itself a `Layer` (None =
    // no-op), so the registry wiring is identical either way. The directory is
    // owned by `omnibus_db::logs` so the log viewer reads exactly where we
    // write.
    let dir = omnibus_db::logs::log_dir();
    let (file_layer, guard) = match std::fs::create_dir_all(&dir) {
        Ok(()) => {
            let appender = tracing_appender::rolling::daily(&dir, "omnibus.log");
            let (writer, guard) = tracing_appender::non_blocking(appender);
            let layer = fmt::layer().json().with_writer(writer);
            (Some(layer), Some(guard))
        }
        Err(err) => {
            eprintln!(
                "could not create log dir {}: {err}; on-disk JSON logging disabled",
                dir.display()
            );
            (None, None)
        }
    };

    // try_init over init: a second subscriber (e.g. in tests) is a no-op, not a
    // panic. A single env-filter gates both layers.
    tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_writer(std::io::stderr).compact())
        .with(file_layer)
        .try_init()
        .ok();

    guard
}
