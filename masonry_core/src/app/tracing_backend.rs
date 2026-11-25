// Copyright 2024 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

//! Configures a suitable default [`tracing`] implementation for a Masonry application.
//!
//! This uses a custom log format specialised for GUI applications,
//! and will write all logs to a temporary file in debug mode.
//! This also uses a default filter, which can be overwritten using `RUST_LOG`.
//! This will include all [`DEBUG`](tracing::Level::DEBUG) messages in debug mode,
//! and all [`INFO`](tracing::Level::INFO) level messages in release mode.
//!
//! If a `tracing` backend is already configured, this will not overwrite that.

// TODO - Move this code out of masonry.

use std::error::Error;
use std::fmt;
#[cfg(not(target_arch = "wasm32"))]
use std::fs::File;
#[cfg(not(target_arch = "wasm32"))]
use std::time::UNIX_EPOCH;

#[cfg(not(target_arch = "wasm32"))]
use time::macros::format_description;
use tracing::Subscriber;
#[cfg(not(target_arch = "wasm32"))]
use tracing_subscriber::EnvFilter;
use tracing_subscriber::filter::LevelFilter;
#[cfg(not(target_arch = "wasm32"))]
use tracing_subscriber::fmt::time::UtcTime;
use tracing_subscriber::prelude::*;

#[cfg(not(target_arch = "wasm32"))]
/// Get the tracing subscriber we wish to set-up for a non-web platform with the given `default_level`.
///
/// Returns the subscriber and any recoverable configuration errors.
fn default_tracing_subscriber_native(
    default_level: LevelFilter,
) -> (impl Subscriber, Vec<Box<dyn Error>>) {
    // Use EnvFilter to allow the user to override the log level without recompiling.
    let env_filter_builder = EnvFilter::builder()
        .with_default_directive(default_level.into())
        .with_env_var("RUST_LOG");
    let mut errors: Vec<Box<dyn Error>> = env_filter_builder
        .from_env()
        .err()
        .map(|err| format!("failed to parse RUST_LOG environment variable: {err:#}").into())
        .into_iter()
        .collect();
    let env_filter = env_filter_builder.from_env_lossy();

    // This format is more concise than even the 'Compact' default:
    // - We print the time without the date (GUI apps usually run for very short periods).
    // - We print the time with millisecond instead of microsecond precision.
    // - We skip the target. In app code, the target is almost always visual noise. By
    //   default, it only gives you the module a log was defined in. This is rarely useful;
    //   the log message is much more helpful for finding a log's location.
    let timer = UtcTime::new(format_description!(
        // We append a `Z` here to indicate clearly that this is a UTC time
        "[hour repr:24]:[minute]:[second].[subsecond digits:3]Z"
    ));
    // If modifying, also update the module level docs
    let console_layer = tracing_subscriber::fmt::layer()
        .with_timer(timer.clone())
        .with_target(false)
        .with_filter(env_filter);

    // We skip the layer which stores to a file in `--release` mode for performance.
    let log_file_layer = if cfg!(debug_assertions) {
        // TODO - Replace with a more targeted subscriber.
        // See https://github.com/linebender/xilem/issues/1556

        let open_log = || -> Result<_, Box<dyn Error>> {
            let id = std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)?
                .as_nanos();
            let pid = std::process::id();
            let path = std::env::temp_dir().join(format!("masonry-{pid}-{id:032x}-dense.log"));
            let file = File::create_new(&path)?;
            Ok((path, file))
        };
        match open_log() {
            Ok((path, file)) => {
                #[expect(
                    clippy::print_stderr,
                    reason = "Report the debug log path before the tracing subscriber is installed"
                )]
                {
                    eprintln!("Writing full logs to {}", path.display());
                }
                Some(
                    tracing_subscriber::fmt::layer()
                        .with_timer(timer)
                        .with_writer(file)
                        .with_ansi(false),
                )
            }
            Err(error) => {
                errors.push(format!("failed to open debug log: {error}").into());
                None
            }
        }
    } else {
        None
    };

    #[cfg(target_os = "android")]
    let android_trace_layer = tracing_android_trace::AndroidTraceLayer::new();

    let registry = tracing_subscriber::registry()
        .with(console_layer)
        .with(log_file_layer);

    #[cfg(target_os = "android")]
    let registry = registry.with(android_trace_layer);

    // After the above line because of https://github.com/linebender/android_trace/pull/17
    #[cfg(feature = "tracy")]
    let registry = registry.with(tracing_tracy::TracyLayer::default());

    (registry, errors)
}

#[cfg(target_arch = "wasm32")]
/// Initialise tracing for the web with the given `max_level`.
fn default_tracing_subscriber_wasm(
    max_level: LevelFilter,
) -> (impl Subscriber, Vec<Box<dyn Error>>) {
    // Note - tracing-wasm might not work in headless Node.js. Probably doesn't matter anyway,
    // because this is a GUI framework, so wasm targets will virtually always be browsers.

    // Ignored if the panic hook is already set
    console_error_panic_hook::set_once();

    let layer = max_level.into_level().map(|level| {
        let config = tracing_wasm::WASMLayerConfigBuilder::new()
            .set_max_level(level)
            .build();
        tracing_wasm::WASMLayer::new(config)
    });
    (
        tracing_subscriber::Registry::default().with(layer),
        Vec::new(),
    )
}

/// Constructs a default tracing subscriber with a given `max_level` filter.
///
/// Recoverable configuration errors are returned alongside the subscriber.
#[must_use = "Install or inspect the constructed tracing subscriber."]
pub fn default_tracing_subscriber(
    max_level: LevelFilter,
) -> (impl Subscriber, Vec<Box<dyn Error>>) {
    #[cfg(not(target_arch = "wasm32"))]
    {
        default_tracing_subscriber_native(max_level)
    }

    #[cfg(target_arch = "wasm32")]
    {
        default_tracing_subscriber_wasm(max_level)
    }
}

/// An Error indicating that a tracing subscriber has been set before.
#[derive(Debug)]
pub struct TracingSubscriberHasBeenSetError;

impl fmt::Display for TracingSubscriberHasBeenSetError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.pad("A tracing subscriber has been set before.")
    }
}

impl Error for TracingSubscriberHasBeenSetError {}

/// Verify that a tracing subscriber has not been set before or return with an error.
fn verify_subscriber_has_not_been_set() -> Result<(), TracingSubscriberHasBeenSetError> {
    // The tracing_core::dispatcher::has_been_set function is doc(hidden).
    // However, it is guaranteed to remain for the entire tracing_core 1.0 series,
    // as tracing depends on it, and it isn't documented as unsupported.
    if tracing_core::dispatcher::has_been_set() {
        return Err(TracingSubscriberHasBeenSetError);
    }
    Ok(())
}

fn install_default_tracing(level: LevelFilter) -> Result<(), TracingSubscriberHasBeenSetError> {
    verify_subscriber_has_not_been_set()?;
    let (subscriber, errors) = default_tracing_subscriber(level);
    tracing::subscriber::set_global_default(subscriber)
        .map_err(|_| TracingSubscriberHasBeenSetError)?;
    for error in errors {
        tracing::error!("Initialising logging encountered recoverable error: {error}");
    }
    Ok(())
}

/// Initialise tracing with a default subscriber for a unit test.
///
/// # Errors
///
/// Returns an error if a global subscriber has already been installed.
pub fn try_init_test_tracing() -> Result<(), TracingSubscriberHasBeenSetError> {
    install_default_tracing(LevelFilter::WARN)
}

/// Initialise tracing with a default subscriber for an end-user application.
///
/// # Errors
///
/// Returns an error if a global subscriber has already been installed.
pub fn try_init_tracing() -> Result<(), TracingSubscriberHasBeenSetError> {
    let default_level = if cfg!(debug_assertions) {
        LevelFilter::DEBUG
    } else {
        LevelFilter::INFO
    };
    install_default_tracing(default_level)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multiple_init_test_tracing_errors() {
        let _first_result = try_init_test_tracing();
        let second_result = try_init_test_tracing();
        assert!(second_result.is_err());
    }
}
