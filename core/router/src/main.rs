// SPDX-License-Identifier: CC0-1.0

#![forbid(unsafe_code)]
//! Emulator server for aws-emulator.
//!
//! Serves the multi-service emulator and `GET /__admin/health` on self-signed HTTPS `:9297`.
//! `--help` lists the command line (`-b`, `-p`, `-v`, `-a on|off`, `-t on|off`, `--idle-timeout`).
//! Reports aggregate usage telemetry through the vendored `emulator-telemetry` crate; the
//! workspace README's "Telemetry" section says what is collected and how to turn
//! it off.
//!
//! # Do not replace this with `#[tokio::main]`
//!
//! Assembling the router costs stack in proportion to the largest `mount_chunk_NN`
//! plus the largest single service's `ops::register_all`. Three things keep that
//! within the stack, and removing any one of them makes the process abort before
//! either listener binds — the emulator then looks like one that was never started,
//! and no request gets an answer:
//!
//! * the emulator runs on a thread created with an explicit
//!   [`EMULATOR_STACK_SIZE`], not on the process's default-sized main thread;
//! * the tokio runtime is built by hand so its WORKER threads also get
//!   [`WORKER_STACK_SIZE`] — request handling, and so the deepest generated
//!   deserializer, runs on a worker rather than on the thread above; and
//! * `build_router` is a plain `fn` calling `#[inline(never)]` chunk helpers (see
//!   `router/src/lib.rs`), so no single frame holds the whole workspace.
//!
//! This file is framework code, emitted ONCE — the scaffold does not re-render
//! it. Do not edit it. (`router/src/lib.rs` IS re-rendered on every service add.)

use emulator_server::cli::{BuildInfo, Command, RunOptions};
use engine::InMemoryEngine;
use std::io::Write;
use std::sync::Arc;

/// The version `--version` prints and telemetry reports: the release version the
/// release build stamps as `EMULATOR_VERSION`, and the package version with `-dev`
/// appended for any other build.
const PROJECT_VERSION: &str = match option_env!("EMULATOR_VERSION") {
    Some(version) => version,
    None => concat!(env!("CARGO_PKG_VERSION"), "-dev"),
};

/// What `--version` prints; the release build stamps the date and commit.
const BUILD_INFO: BuildInfo = BuildInfo {
    version: Some(PROJECT_VERSION),
    release_date: option_env!("EMULATOR_RELEASE_DATE"),
    commit: option_env!("EMULATOR_COMMIT"),
};

/// Stack for the thread that assembles the router and serves.
const EMULATOR_STACK_SIZE: usize = 64 * 1024 * 1024;

/// Advisory floor for [`STACK_SIZE_ENV`], not a clamp: a smaller value is honoured and
/// warned about, because a boot below this may overflow the stack while assembling the
/// router.
const MINIMUM_STACK_SIZE: usize = 32 * 1024 * 1024;

/// Stack for each tokio WORKER thread, which is where request handling — and so the
/// deepest generated deserializer — actually runs.
const WORKER_STACK_SIZE: usize = 16 * 1024 * 1024;

/// The project id this emulator reports to telemetry.
const TELEMETRY_PROJECT_ID: &str = "proj-a85d506b35244f71850223e92be29acb";

/// The emulator's folder in the user's home (config file, telemetry state, `--auth` keys).
const EMULATOR_HOME_DIR: &str = ".aws/emulator";

/// The telemetry configuration endpoint. `None` leaves the client inert.
const TELEMETRY_CONFIG_ENDPOINT: Option<&str> = Some("https://aws-emulator-telemetry.us-west-2.api.aws/v1/config");

/// Environment variable overriding [`EMULATOR_STACK_SIZE`], in BYTES, for a host that
/// refuses a large thread stack. A malformed or too-small value falls back to the
/// default rather than failing the boot.
const STACK_SIZE_ENV: &str = "EMULATOR_STACK_BYTES";

fn emulator_stack_size() -> usize {
    let Some(raw) = std::env::var(STACK_SIZE_ENV).ok() else {
        return EMULATOR_STACK_SIZE;
    };
    match raw.trim().parse::<usize>() {
        Ok(bytes) if bytes > 0 => {
            if bytes < MINIMUM_STACK_SIZE {
                // Honoured, not clamped — but say so, because the likely outcome is a
                // stack overflow during `build_router` that looks like "not running".
                eprintln!(
                    "{STACK_SIZE_ENV}={bytes} is below the {MINIMUM_STACK_SIZE}-byte \
                     advisory floor; honouring it, but the emulator may overflow its \
                     stack while assembling the router"
                );
            }
            bytes
        }
        _ => {
            eprintln!(
                "{STACK_SIZE_ENV}={raw:?} is not a positive byte count; using the \
                 {EMULATOR_STACK_SIZE}-byte default"
            );
            EMULATOR_STACK_SIZE
        }
    }
}

fn main() {
    if let Err(error) = emulator_server::set_emulator_home_dir(EMULATOR_HOME_DIR) {
        eprintln!("{error}");
        std::process::exit(2);
    }
    let options = match emulator_server::cli::parse_command_line(std::env::args().skip(1), BUILD_INFO) {
        Ok(Command::Run(options)) => options,
        Ok(Command::Print(text)) => {
            let _ = writeln!(std::io::stdout(), "{text}"); // closed stdout (`-v | head -1`) is fine
            return;
        }
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(2);
        }
    };
    emulator_server::set_authentication(options.authentication);
    emulator_server::cli::listen_on(options.bind_address, options.port);
    let stack_bytes = emulator_stack_size();
    let thread_options = options.clone();
    let emulator = match std::thread::Builder::new()
        .name("emulator-main".to_string())
        .stack_size(stack_bytes)
        .spawn(move || run(thread_options))
    {
        Ok(handle) => handle,
        Err(error) => {
            // If the sized thread cannot be spawned, serve on the main thread instead:
            // a small workspace still boots. Written straight to stderr because the
            // tracing subscriber is installed later, inside `run`.
            eprintln!(
                "could not spawn the emulator thread with a {stack_bytes}-byte stack \
                 ({error}); trying to serve on the main thread instead"
            );
            run(options);
            return;
        }
    };
    // A panic on that thread must exit the process non-zero, or a crash is reported as
    // a clean shutdown.
    if let Err(panic) = emulator.join() {
        let detail = panic
            .downcast_ref::<&str>()
            .map(|message| (*message).to_string())
            .or_else(|| panic.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "unknown panic payload".to_string());
        // Straight to stderr, not `tracing`: the subscriber is installed inside `run`, so
        // a panic before that point (building the runtime, for one) would otherwise exit
        // silently with no cause recorded.
        eprintln!("emulator thread panicked: {detail}");
        std::process::exit(1);
    }
}

/// Build the runtime `#[tokio::main]` would have built, plus a roomier worker stack,
/// and drive the emulator on THIS thread.
fn run(options: RunOptions) {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(WORKER_STACK_SIZE)
        .build()
        .expect("build the emulator tokio runtime");
    runtime.block_on(build_and_serve(options));
}

async fn build_and_serve(options: RunOptions) {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .json()
        .init();

    tracing::info!("aws-emulator emulator starting");

    let telemetry = start_telemetry(options.telemetry);
    if let Some(idle_timeout) = options.idle_timeout {
        emulator_server::cli::stop_after_idle(idle_timeout);
    }

    let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());

    // The state directory is selected ONCE, here: root and namespace validated and then fixed.
    // The same value goes to everything that persists (service snapshots now; the payload store
    // and the emulator's private identity are installed from it on the line above
    // `Persistence::all`), so no writer re-reads the environment or picks a different place.
    let state_dir = match service_common::persist::StateDir::from_env() {
        Ok(state_dir) => state_dir,
        Err(error) => {
            eprintln!("emulator state directory refused: {error}");
            std::process::exit(1);
        }
    };
    // With a state directory, every persisted service's state comes back BEFORE the router is
    // built or a listener opens, and a snapshot that cannot be read honestly stops the boot.
    let persistence = match state_dir {
        Some(state_dir) => {
            match aws_emulator_router::persistence::Persistence::all(engine.clone(), state_dir) {
                Ok(persistence) => Some(Arc::new(persistence)),
                Err(error) => {
                    eprintln!("emulator state directory refused: {error}");
                    std::process::exit(1);
                }
            }
        }
        None => None,
    };
    if let Some(persistence) = &persistence {
        match persistence.restore() {
            Ok(outcome) => tracing::info!(
                restored = ?outcome.restored,
                started_empty = ?outcome.empty,
                "persisted state restored"
            ),
            Err(error) => {
                eprintln!("emulator state could not be restored: {error}");
                std::process::exit(1);
            }
        }
        match persistence.rearm().await {
            Ok(rearmed) => tracing::info!(?rearmed, "restored workloads brought back"),
            Err(error) => {
                eprintln!("emulator state could not be brought back: {error}");
                std::process::exit(1);
            }
        }
    }

    let router = aws_emulator_router::build_router(engine);

    let outcome = match &persistence {
        Some(persistence) => {
            // Work that changes state after its response (a container a task launched, a clock
            // move) has no request behind it to write it; this writes it within two seconds.
            let periodic = persistence.clone();
            tokio::spawn(async move {
                let mut tick = tokio::time::interval(std::time::Duration::from_secs(2));
                loop {
                    tick.tick().await;
                    // A failure is logged by save_changed and retried on the next tick.
                    let _ = periodic.save_changed().await;
                }
            });
            let served = emulator_server::serve_emulator_until_shutdown(
                aws_emulator_router::persistence::PersistingRouter::new(router, persistence.clone()),
            )
            .await;
            if let Err(not_durable) = persistence.save_changed().await {
                eprintln!("emulator state not written at shutdown: {:?}", not_durable.failed);
            }
            served
        }
        None => emulator_server::serve_emulator_until_shutdown(router).await,
    };
    // Only a clean end may send the final batch: a session without one is how an
    // abnormal end is recognised, so a failed serve flushes without marking it final.
    match outcome {
        Ok(()) => {
            if let Some(telemetry) = telemetry {
                telemetry.shutdown(emulator_telemetry::policy::SHUTDOWN_FLUSH_BUDGET);
            }
        }
        Err(error) => {
            if let Some(telemetry) = telemetry {
                telemetry.flush_blocking(emulator_telemetry::policy::SHUTDOWN_FLUSH_BUDGET);
            }
            eprintln!("emulator serve failed: {error}");
            std::process::exit(1);
        }
    }
}

/// Start the telemetry client. Never fails the boot: an unusable value leaves
/// telemetry off and says why. `-t off` turns it off; `-t on` does not override
/// `EMULATOR_TELEMETRY=off` or an off value in the config file.
fn start_telemetry(switch: Option<bool>) -> Option<&'static emulator_telemetry::TelemetryClient> {
    let project_id = match emulator_telemetry::ProjectId::new(TELEMETRY_PROJECT_ID) {
        Ok(project_id) => project_id,
        Err(error) => {
            tracing::warn!(%error, "telemetry off: invalid project id");
            return None;
        }
    };
    let product_version = match emulator_telemetry::ProductVersion::new(PROJECT_VERSION) {
        Ok(product_version) => product_version,
        Err(error) => {
            tracing::warn!(%error, "telemetry off: invalid package version");
            return None;
        }
    };
    let mut options = match TELEMETRY_CONFIG_ENDPOINT.map(emulator_telemetry::Endpoint::parse) {
        Some(Ok(endpoint)) => {
            emulator_telemetry::TelemetryOptions::new(project_id, product_version, endpoint)
        }
        Some(Err(error)) => {
            tracing::warn!(%error, "telemetry inert: invalid config endpoint");
            emulator_telemetry::TelemetryOptions::without_endpoint(project_id, product_version)
        }
        None => emulator_telemetry::TelemetryOptions::without_endpoint(project_id, product_version),
    };
    if switch == Some(false) {
        options.embedding_opt_out = true;
    }
    let telemetry = emulator_telemetry::init_global(emulator_telemetry::TelemetryClient::start(options));
    telemetry.install_panic_hook();
    Some(telemetry)
}
