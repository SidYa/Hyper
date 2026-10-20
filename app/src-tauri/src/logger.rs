//! Logging: everything goes to stdout (visible in `tauri dev`) and to a
//! daily-rotated `hyper.log` file in the logs directory.

use std::path::PathBuf;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{EnvFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt};

pub fn init(logs_dir: PathBuf) -> WorkerGuard {
    // two weeks of days, not years of them (logs can hold account names)
    let file_appender = tracing_appender::rolling::RollingFileAppender::builder()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix("hyper")
        .filename_suffix("log")
        .max_log_files(14)
        .build(&logs_dir)
        .unwrap_or_else(|_| tracing_appender::rolling::daily(&logs_dir, "hyper.log"));
    let (file_writer, guard) = tracing_appender::non_blocking(file_appender);

    let filter = EnvFilter::try_from_env("HYPER_LOG")
        .unwrap_or_else(|_| EnvFilter::new("info,hyper_app_lib=debug,gilrs=error"));

    tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_target(true).compact())
        .with(fmt::layer().with_ansi(false).with_writer(file_writer))
        .init();

    guard
}
