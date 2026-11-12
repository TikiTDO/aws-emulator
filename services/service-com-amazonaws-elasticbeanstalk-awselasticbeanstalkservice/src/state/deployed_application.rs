// SPDX-License-Identifier: CC0-1.0

//! The application an environment RUNS: the files of the version deployed to it,
//! the log that application's own code writes, and what it answers a request
//! with.
//!
//! `CreateEnvironment$VersionLabel` is "the name of the application version to
//! deploy" and `UpdateEnvironment` "deploys the named application version to the
//! environment", so an environment's deployment has observable consequences: the
//! application's own log lines (which the environment information a
//! `RequestEnvironmentInfo` compiles carries beside this service's activity log)
//! and the content it serves at the environment's own address.
//!
//! Both come from RUNNING the deployed code, never from anything this emulator
//! makes up: the version's `SourceBundle` is downloaded from object storage
//! through the one bounded reader this crate already has
//! ([`super::download_caller_archive`]), the archive is read by the archive
//! reader, and the entry point it carries is evaluated by the embedded engine
//! ([`super::application_runtime`]). A bundle with no entry point this emulator
//! has an engine for leaves the environment serving the FILES the bundle carries
//! and writing no application log at all, rather than output nobody produced.
//!
//! The bundle is read once, when the VERSION is created, and kept on the version
//! record: an environment deploys a version by label, so every environment
//! running that label runs the same files, and the read happens where a create
//! can reach object storage.

use super::*;

/// The archive reader's entry is a `Read`, and the trait has to be in scope for
/// `read_to_end` ([`ApplicationBundle::read`]).
use ::std::io::Read as _;

/// How many entries of one source bundle are kept.
///
/// A bound on CALLER content: the bundle's files are held in this process for as
/// long as the version exists, so the count is capped rather than trusted.
pub const MAX_BUNDLE_FILES: usize = 64;

/// How many bytes of ONE file of the bundle are kept.
pub const MAX_BUNDLE_FILE_BYTES: usize = 512 * 1024;

/// How many bytes of the bundle are kept in total, over every file — the bound on
/// the PRODUCT the count bound cannot give on its own.
pub const MAX_BUNDLE_BYTES: usize = 2 * 1024 * 1024;

/// The file names an Elastic Beanstalk Node.js platform starts a deployed
/// application from, in the order it looks for them — after the `main` of the
/// bundle's `package.json`, which comes first. (Elastic Beanstalk's Node.js
/// platform documentation: the platform runs the `main` a `package.json` names,
/// and otherwise `app.js`.)
const ENTRY_POINT_CANDIDATES: &[&str] = &["app.js", "server.js", "index.js", "main.js"];

/// The file a request for the application's root is served from when the bundle
/// carries no entry point this emulator can run: the document name a web server
/// serves a directory's root from.
const ROOT_DOCUMENT: &str = "index.html";

/// What the archive IS, for the log line a read that learned nothing leaves
/// behind. See [`super::download_caller_archive`].
const BUNDLE_SUBJECT: &str = "the source bundle an environment runs the application from";

/// The configuration-option namespace whose settings are the deployed
/// application's own environment variables: `aws:elasticbeanstalk:application:
/// environment`, which Elastic Beanstalk's configuration-option reference
/// documents as the environment properties passed to the application.
pub const NAMESPACE_APPLICATION_ENVIRONMENT: &str = "aws:elasticbeanstalk:application:environment";

/// The variable Elastic Beanstalk sets to the port its proxy forwards to, which
/// is the port a deployed application listens on.
const PORT_VARIABLE: &str = "PORT";

/// The value of that variable on the platforms this service publishes.
const PORT_VALUE: &str = "8080";

/// The log file an application's standard output goes to on an Elastic Beanstalk
/// instance, as the compiled environment information names it.
pub const APPLICATION_STDOUT_LOG: &str = "/var/log/web.stdout.log";

/// The log file this service's own activity goes to on that instance, which is
/// the other file the compiled environment information carries.
pub const ACTIVITY_LOG: &str = "/var/log/eb-activity.log";

/// The rule the compiled information draws above and below each file name, so a
/// bundle of several log files says which file each section came from.
pub const LOG_SECTION_RULE: &str = "-------------------------------------";

/// How many deployed applications this emulator runs AT ONCE.
///
/// An EMULATOR bound: each run holds the engine's own heap
/// ([`super::application_runtime`]) on a blocking thread, so this is what bounds
/// what those runs can hold and occupy however many requests arrive together.
const MAX_CONCURRENT_RUNS: usize = 4;

/// How long a request waits for one of those run slots before it is answered as
/// an application that produced nothing, rather than queueing behind other
/// callers' code.
const RUN_PERMIT_WAIT: Duration = Duration::from_secs(5);

/// The run slots [`MAX_CONCURRENT_RUNS`] declares, over every environment of this
/// process: the bound is on what the ENGINE may hold at once, which is a property
/// of the process rather than of one environment or one request.
fn application_runs() -> &'static ::tokio::sync::Semaphore {
    static RUNS: ::std::sync::OnceLock<::tokio::sync::Semaphore> = ::std::sync::OnceLock::new();
    RUNS.get_or_init(|| ::tokio::sync::Semaphore::new(MAX_CONCURRENT_RUNS))
}

/// The files one application version's source bundle carries: what an environment
/// that deploys the version runs and serves.
#[derive(Debug, Clone, Default)]
pub struct ApplicationBundle {
    /// The files of the archive, by their path inside it.
    pub files: BTreeMap<String, Vec<u8>>,
    /// The file the platform starts the application from, when the bundle carries
    /// one this emulator has an engine for.
    pub entry_point: Option<String>,
}

impl ApplicationBundle {
    /// The bundle `bytes` carries, inside this module's bounds: a `.zip` is what
    /// an Elastic Beanstalk source bundle is, so the archive FORMAT is read by
    /// the archive reader rather than by this emulator.
    ///
    /// A directory entry, an entry whose name escapes the archive and an entry
    /// past [`MAX_BUNDLE_FILE_BYTES`] are left out: the bundle is the files that
    /// could be read, which is what an environment then runs and serves.
    pub fn read(bytes: &[u8]) -> Self {
        let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        let Ok(mut archive) = ::zip::ZipArchive::new(::std::io::Cursor::new(bytes)) else {
            ::tracing::warn!(
                "an application version's source bundle is not an archive this emulator can \
                 read, so an environment deploying it runs none of its files",
            );
            return Self::default();
        };
        let mut budget = MAX_BUNDLE_BYTES;
        for index in 0..archive.len() {
            if files.len() >= MAX_BUNDLE_FILES || budget == 0 {
                break;
            }
            let Ok(mut entry) = archive.by_index(index) else {
                continue;
            };
            if entry.is_dir() {
                continue;
            }
            // The archive reader reports a name as an `Option`, because a name
            // that escapes the archive is not a path a file may be read into.
            let Some(name) = entry
                .enclosed_name()
                .and_then(|name| name.to_str().map(str::to_string))
            else {
                continue;
            };
            let room = budget.min(MAX_BUNDLE_FILE_BYTES);
            let mut body = Vec::new();
            // One byte past the room left, so a file that does not fit is left
            // out rather than stored truncated.
            let read = ::std::io::Read::take(&mut entry, room as u64 + 1).read_to_end(&mut body);
            if read.is_err() || body.len() > room {
                continue;
            }
            budget -= body.len();
            files.insert(name, body);
        }
        let entry_point = entry_point_of(&files);
        Self { files, entry_point }
    }

    /// The source of the file the platform starts, when the bundle carries one
    /// and it is text.
    pub fn entry_source(&self) -> Option<&str> {
        let entry_point = self.entry_point.as_ref()?;
        let source = self.files.get(entry_point)?;
        ::std::str::from_utf8(source).ok()
    }

    /// Whether this bundle carries nothing at all, which is what a bundle no read
    /// reached and one no archive reader would read both leave.
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

/// The application one environment is running: the version it deploys, the files
/// of that version's bundle and the process environment the environment's
/// configuration gives it.
#[derive(Debug, Clone)]
pub struct DeployedApplication {
    /// The version label the environment reports running.
    pub version_label: String,
    pub bundle: ApplicationBundle,
    /// The environment variables the application reads out of its own process
    /// environment.
    pub environment: Vec<(String, String)>,
}

impl DeployedApplication {
    /// START the application: evaluate its entry point with no request to serve,
    /// which is what writes its start-up log.
    ///
    /// `None` when the bundle carries no entry point this emulator has an engine
    /// for — such an environment's application writes no log of its own.
    pub async fn start(&self) -> Option<GuestRun> {
        self.run(None).await
    }

    /// What the application answers `request` with: the response its own request
    /// handler ends, and otherwise the file of the bundle the request addresses.
    ///
    /// `None` when the application answers nothing at that address and the bundle
    /// carries no file for it either, which the plane then answers as a path
    /// nothing is served at.
    pub async fn serve(&self, request: &GuestRequest) -> Option<GuestResponse> {
        if let Some(run) = self.run(Some(request)).await {
            if let Some(response) = run.response {
                return Some(response);
            }
            if let Some(failure) = run.failure {
                ::tracing::warn!(
                    detail = %failure,
                    path = %request.path,
                    "the deployed application did not answer the request it was handed",
                );
            }
        }
        self.served_file(&request.path)
    }

    /// Run the entry point on a blocking thread, handing it `request` when there
    /// is one: the engine is synchronous, and its interrupt handler is what stops
    /// code with no exit.
    ///
    /// One of [`MAX_CONCURRENT_RUNS`] slots is held for the run, so what the
    /// engine holds at once is bounded however many requests arrive together; a
    /// request that waits [`RUN_PERMIT_WAIT`] for one is answered as an
    /// application that produced nothing rather than queued behind other
    /// callers' code.
    async fn run(&self, request: Option<&GuestRequest>) -> Option<GuestRun> {
        let entry = self.bundle.entry_source()?.to_string();
        let environment = self.environment.clone();
        let request = request.cloned();
        let permit = ::tokio::time::timeout(RUN_PERMIT_WAIT, application_runs().acquire()).await;
        let permit = match permit {
            Ok(Ok(permit)) => permit,
            _ => {
                ::tracing::warn!(
                    "the deployed application was not run: every run slot this emulator holds \
                     was taken",
                );
                return None;
            }
        };
        let run = ::tokio::task::spawn_blocking(move || {
            run_application(&entry, &environment, request.as_ref())
        })
        .await;
        drop(permit);
        match run {
            Ok(run) => Some(run),
            Err(failure) => {
                ::tracing::warn!(
                    detail = %failure,
                    "the thread the deployed application ran on did not finish",
                );
                None
            }
        }
    }

    /// The file of the bundle a request path addresses: the path with its leading
    /// separator and any query string removed, and the root document for a
    /// request for the application's root.
    fn served_file(&self, path: &str) -> Option<GuestResponse> {
        let path = path.split(['?', '#']).next().unwrap_or(path);
        let name = path.trim_start_matches('/');
        let name = if name.is_empty() { ROOT_DOCUMENT } else { name };
        let body = self.bundle.files.get(name)?;
        Some(GuestResponse {
            status: 200,
            headers: vec![(
                "content-type".to_string(),
                document_media_type(name).to_string(),
            )],
            body: body.clone(),
        })
    }
}

/// The lines one run of the application wrote to its own log: its standard output
/// then its standard error, with the empty trailing line a text stream ends in
/// dropped — byte for byte as the application wrote them, so non-ASCII text
/// survives unchanged.
pub fn application_log_lines(run: &GuestRun) -> Vec<&str> {
    [run.stdout.as_str(), run.stderr.as_str()]
        .into_iter()
        .flat_map(|stream| stream.split_inclusive('\n'))
        .map(|line| line.strip_suffix('\n').unwrap_or(line))
        .filter(|line| !line.is_empty())
        .collect()
}

/// The environment variables the deployed application reads: every setting of the
/// environment-properties namespace, plus the port the platform tells the
/// application to listen on.
pub fn application_environment(
    option_settings: &[ConfigurationOptionSetting],
) -> Vec<(String, String)> {
    let mut environment: Vec<(String, String)> =
        vec![(PORT_VARIABLE.to_string(), PORT_VALUE.to_string())];
    for setting in option_settings {
        if setting.namespace.as_deref() != Some(NAMESPACE_APPLICATION_ENVIRONMENT) {
            continue;
        }
        let (Some(name), Some(value)) = (setting.option_name.as_deref(), setting.value.as_deref())
        else {
            continue;
        };
        environment.push((name.to_string(), value.to_string()));
    }
    environment
}

/// Read the source bundle at `location` out of object storage, in process: the
/// files an environment that deploys this version runs.
///
/// `None` when there is nothing to read — no object the read reached, or an
/// archive the archive reader will not read. A version then carries no files, and
/// an environment deploying it runs no application of its own rather than one
/// whose output was invented.
pub async fn read_application_bundle(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    location: &S3Location,
) -> Option<ApplicationBundle> {
    let bytes = match download_caller_archive(ctx, region, location, BUNDLE_SUBJECT).await {
        DownloadedArchive::Bytes(bytes) => bytes,
        DownloadedArchive::NotReadable | DownloadedArchive::NotRead => return None,
    };
    let bundle = ApplicationBundle::read(&bytes);
    (!bundle.is_empty()).then_some(bundle)
}

/// The media type a bundle file is served under, by the extension it carries —
/// the handful a deployed web application's own documents use. Anything else is
/// served as the bytes it is.
fn document_media_type(name: &str) -> &'static str {
    match name.rsplit_once('.').map(|(_, extension)| extension) {
        Some("html" | "htm") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("json") => "application/json",
        Some("txt" | "log") => "text/plain; charset=utf-8",
        Some("svg") => "image/svg+xml",
        _ => "application/octet-stream",
    }
}

/// The file the platform starts the application from: the `main` the bundle's
/// `package.json` names, when the bundle carries that file, and otherwise the
/// first of [`ENTRY_POINT_CANDIDATES`] it carries.
///
/// Only a JavaScript file is an entry point here: JavaScript is what the embedded
/// engine runs, so a bundle whose entry point is anything else is one this
/// emulator starts no application from.
fn entry_point_of(files: &BTreeMap<String, Vec<u8>>) -> Option<String> {
    let main = files
        .get("package.json")
        .and_then(|manifest| ::std::str::from_utf8(manifest).ok())
        .and_then(|manifest| ::serde_json::from_str::<::serde_json::Value>(manifest).ok())
        .and_then(|manifest| {
            manifest
                .get("main")
                .and_then(::serde_json::Value::as_str)
                .map(str::to_string)
        })
        .map(|main| main.trim_start_matches("./").to_string())
        .filter(|main| files.contains_key(main));
    main.into_iter()
        .chain(
            ENTRY_POINT_CANDIDATES
                .iter()
                .filter(|candidate| files.contains_key(**candidate))
                .map(|candidate| (*candidate).to_string()),
        )
        .find(|name| name.ends_with(".js"))
}

/// The content the environment information compiled from one of an environment's
/// instances carries: the log FILES that instance holds.
///
/// "Compiles the last lines from the application server log files of every Amazon
/// EC2 instance in your environment" — so the content is this service's own
/// activity log and the standard output of the application the environment runs,
/// each introduced by the path it has on the instance, the way a compiled bundle
/// names the file each section came from.
///
/// An environment running no application of its own has one log file, and the
/// content is that file: there is no second section to name, so nothing is
/// written about a file that does not exist. `max_lines` has already been applied
/// to `activity`; it is applied to the application's lines here, since it is the
/// tail of each log that a `tail` compiles.
pub fn compiled_environment_information(
    activity: &str,
    application: &[&str],
    max_lines: Option<usize>,
) -> String {
    if application.is_empty() {
        return activity.to_string();
    }
    let dropped = application
        .len()
        .saturating_sub(max_lines.unwrap_or(application.len()));
    let application = application[dropped..].join("\n");
    format!(
        "{rule}\n{ACTIVITY_LOG}\n{rule}\n{activity}\
         {rule}\n{APPLICATION_STDOUT_LOG}\n{rule}\n{application}\n",
        rule = LOG_SECTION_RULE,
    )
}
