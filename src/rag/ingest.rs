//! Drop-to-ingest (feature `grobid`): PDFs and URLs become library files and
//! bibliography entries.
//!
//! This module delegates the orchestration to the shared
//! [`grobid_bibtex::prepass`] pipeline (see `UPSTREAM.md`, US-2); the manifest
//! and rename policy are the crate's single source of truth. The pipeline
//! semantics:
//!
//! 1. **Fingerprint** — the sidecar manifest `.ragrig_grobid.json` in the
//!    library folder records mtime/size and the parsed record of every
//!    processed PDF, so unchanged files are never parsed twice and the two
//!    tools share what has already been done.
//! 2. **Extract** — new or changed PDFs are parsed by a running GROBID server
//!    (`/api/processHeaderDocument`).
//! 3. **Complete** — parsed headers are completed against OpenAlex (DOI first,
//!    then a title search); failures keep the parsed header.
//! 4. **Rename** — files are renamed to `Key - Full authors - Full title - Year`,
//!    with collisions numbered on the year.
//! 5. **Merge** — records are appended to the `.bib` with duplicate detection
//!    and a `file` link to the renamed PDF.
//!
//! On top of the shared pipeline this module adds what a GUI needs: dropped
//! files are copied into the library folder (a file dropped from outside must
//! not be renamed in place), URLs are downloaded first, and every stage
//! reports progress and honours cancellation.
//!
//! The index update after a merge is *not* this module's job — the worker runs
//! an incremental sync once the run succeeded.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use grobid_bibtex::prepass::{self, PrepassObserver, PrepassProgress, PrepassStage};
use grobid_bibtex::{Collection, DEFAULT_GROBID_URL};
use ragrig::{Cancel, CancellationToken, Cancelled};

use super::status::{IngestProgress, IngestStage};

/// Mirror of ragrig's `DEFAULT_MAX_DOWNLOAD_BYTES`.
const MAX_DOWNLOAD_BYTES: u64 = 50 * 1024 * 1024;

/// Per-URL timeouts for the download (connect and read).
const DOWNLOAD_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const DOWNLOAD_READ_TIMEOUT: Duration = Duration::from_secs(120);

/// One dropped item to ingest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IngestSource {
    /// A local PDF (anywhere; it is copied into the library folder).
    File(PathBuf),
    /// An `http(s)` URL of a PDF; downloaded into the library folder first.
    Url(String),
}

impl IngestSource {
    /// Whether a run would actually process this source.
    ///
    /// PDFs and URLs are processed; folders and other files are skipped with
    /// a note, so a drop that contains only those stays with the editor.
    pub fn is_ingestible(&self) -> bool {
        match self {
            Self::Url(_) => true,
            Self::File(path) => path.is_file() && is_pdf_path(path),
        }
    }
}

/// Classify dropped paths: URL-like paths become URLs (some platforms deliver
/// web drops as a path-like string), everything else stays a file.
pub fn sources_from_paths(paths: &[PathBuf]) -> Vec<IngestSource> {
    paths
        .iter()
        .map(|path| {
            let text = path.to_string_lossy();
            if url_like(&text) {
                IngestSource::Url(text.trim().to_string())
            } else {
                IngestSource::File(path.clone())
            }
        })
        .collect()
}

/// The sources an ingest run should take from a drop of `paths`;
/// `None` when the drop stays with the editor.
///
/// The routing is deliberately position-free: no window backend reports
/// *where* a drag is (winit parses the X11 XdndPosition coordinates and
/// discards them, the Windows drop handler drops the OLE point, egui's
/// `DroppedFile` carries paths only), so a pane hit test cannot work. While
/// the panes are open, a drop that contains a PDF or a URL is the library's
/// — the hover banner announces it — and everything else is an ordinary
/// document drop. A drop is one gesture and is never split between the two.
pub fn drop_sources(paths: &[PathBuf]) -> Option<Vec<IngestSource>> {
    let sources = sources_from_paths(paths);
    sources
        .iter()
        .any(IngestSource::is_ingestible)
        .then_some(sources)
}

/// Whether `value` is an `http(s)` URL with a host.
pub fn url_like(value: &str) -> bool {
    let Ok(url) = url::Url::parse(value.trim()) else {
        return false;
    };
    matches!(url.scheme(), "http" | "https") && url.host_str().is_some()
}

/// How a run is parameterised. The defaults match the shared pre-pass; the
/// OpenAlex and probe fields are seams for hermetic tests.
#[derive(Clone, Debug)]
pub struct IngestOptions {
    /// GROBID server base URL.
    pub grobid_url: String,
    /// OpenAlex-compatible base URL; `None` uses the public API.
    pub openalex_url: Option<String>,
    /// Whether to complete parsed headers against OpenAlex at all.
    pub openalex: bool,
    /// Whether to rename parsed files to their metadata name.
    pub rename: bool,
    /// Concurrent GROBID/OpenAlex requests.
    pub workers: usize,
    /// Liveness probes before giving up on the GROBID server.
    pub probe_attempts: usize,
    /// Delay between liveness probes.
    pub probe_delay: Duration,
}

impl Default for IngestOptions {
    fn default() -> Self {
        Self {
            grobid_url: DEFAULT_GROBID_URL.to_string(),
            openalex_url: None,
            openalex: true,
            rename: true,
            workers: prepass::DEFAULT_WORKERS,
            probe_attempts: prepass::DEFAULT_PROBE_ATTEMPTS,
            probe_delay: prepass::DEFAULT_PROBE_DELAY,
        }
    }
}

/// Where a run writes: the library folder, and the bibliography to merge into.
#[derive(Clone, Copy, Debug)]
pub struct IngestTargets<'a> {
    /// The folder that receives downloaded and copied PDFs, and where files
    /// are renamed. Callers create it beforehand.
    pub folder: &'a Path,
    /// Merge target; `None` runs a rename-only pass.
    pub bibtex: Option<&'a Path>,
    /// Record the PDF path in each merged entry's `file` field.
    pub link: bool,
}

/// What one run did (also carried by the worker's `IngestFinished` event).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IngestOutcome {
    /// Sources that ended with a record (freshly parsed or from the cache).
    pub processed: usize,
    /// Files renamed to their metadata name.
    pub renamed: usize,
    /// Records completed against OpenAlex.
    pub completed: usize,
    /// Entries newly appended to the bibliography.
    pub merged: usize,
    /// Records the merge skipped because they were already in the file.
    pub duplicates: usize,
    /// Sources that failed (`source — reason`).
    pub failed: Vec<String>,
    /// Sources not ingested on purpose (`source — reason`).
    pub skipped: Vec<String>,
}

impl IngestOutcome {
    /// Whether the run changed nothing the user could see (re-dropped files,
    /// everything already merged).
    pub fn nothing_new(&self) -> bool {
        self.renamed == 0 && self.merged == 0 && self.failed.is_empty() && self.skipped.is_empty()
    }
}

/// Ingest `sources` into `targets`, reporting progress and honouring `cancel`.
///
/// A source that fails (unreadable file, download error, GROBID parse error)
/// lands in [`IngestOutcome::failed`]; the run continues with the rest. Only a
/// corrupt `.bib` or an unreachable GROBID server aborts the whole run —
/// there is nothing sensible to do without them.
pub async fn run(
    sources: &[IngestSource],
    targets: &IngestTargets<'_>,
    options: &IngestOptions,
    cancel: &CancellationToken,
    progress: &(dyn Fn(IngestProgress) + Send + Sync),
) -> Result<IngestOutcome> {
    let mut outcome = IngestOutcome::default();

    // A corrupt bibliography must fail before any work: the run could not
    // record what it parses. The pipeline repeats the check for callers that
    // do not ingest drops.
    if let Some(bibtex) = targets.bibtex {
        Collection::load_or_new(bibtex)
            .with_context(|| format!("cannot read the BibTeX file {}", bibtex.display()))?;
    }

    // 1 — URLs are downloaded into the library folder first; dropped files are
    //     copied in (both end up as local PDF paths under `folder`).
    let urls: Vec<&str> = sources
        .iter()
        .filter_map(|source| match source {
            IngestSource::Url(url) => Some(url.as_str()),
            IngestSource::File(_) => None,
        })
        .collect();
    let mut local: Vec<PathBuf> = Vec::new();
    for (index, url) in urls.iter().enumerate() {
        check_cancelled(cancel)?;
        progress(IngestProgress {
            stage: IngestStage::Downloading,
            done: index + 1,
            total: urls.len(),
            name: (*url).to_string(),
        });
        match download(url, targets.folder, cancel).await {
            Ok(path) => local.push(path),
            Err(err) => outcome.failed.push(format!("{url} — {err:#}")),
        }
    }
    for source in sources {
        let IngestSource::File(path) = source else {
            continue;
        };
        if path.is_dir() {
            outcome
                .skipped
                .push(format!("{} — folders are not ingested", path.display()));
            continue;
        }
        if !is_pdf_path(path) {
            outcome.skipped.push(format!(
                "{} — not a PDF (GROBID ingestion handles PDFs only)",
                display_name(path)
            ));
            continue;
        }
        match bring_into_folder(path, targets.folder) {
            Ok(placed) => local.push(placed),
            Err(err) => outcome
                .failed
                .push(format!("{} — {err:#}", display_name(path))),
        }
    }

    if local.is_empty() {
        return Ok(outcome);
    }

    // 2 — The shared pipeline fingerprints the local files, parses and
    //     completes the new or changed ones, renames them and merges the
    //     records; cached records need no server.
    let config = prepass::PrepassConfig {
        folder: targets.folder.to_path_buf(),
        sources: Some(local),
        bibtex: targets.bibtex.map(|path| prepass::BibtexTarget {
            path: path.to_path_buf(),
            link: targets.link,
        }),
        grobid_url: options.grobid_url.clone(),
        openalex_url: options.openalex_url.clone(),
        openalex: options.openalex,
        rename: options.rename,
        workers: options.workers.max(1),
        probe_attempts: options.probe_attempts,
        probe_delay: options.probe_delay,
        openalex_timeout: prepass::DEFAULT_OPENALEX_TIMEOUT,
    };
    let mut observer = IngestObserver { cancel, progress };
    let report = prepass::prepass_with(&config, &mut observer)
        .await
        .map_err(abort_error)?;

    outcome.processed = report.processed;
    outcome.renamed = report.renamed;
    outcome.completed = report.completed;
    outcome.merged = report.merged;
    outcome.duplicates = report.duplicates;
    outcome.failed.extend(
        report
            .failed
            .into_iter()
            .map(|failure| format!("{} — {}", display_name(&failure.path), failure.reason)),
    );
    Ok(outcome)
}

/// Map the pipeline's cancellation marker to the error the worker matches on.
fn abort_error(error: prepass::PrepassError) -> anyhow::Error {
    match error {
        prepass::PrepassError::Cancelled => anyhow::Error::new(Cancelled),
        prepass::PrepassError::NotReady { url, source } => {
            anyhow::Error::new(source).context(format!(
                "GROBID server at {url} is not ready — start one with \
                 `docker run --rm --init --ulimit core=0 -p 8070:8070 grobid/grobid:0.9.1-crf` \
                 or change the server URL in the Corpus menu"
            ))
        }
        other => anyhow::Error::new(other),
    }
}

/// Adapts the worker's cancellation token and progress callback to the
/// pipeline observer.
struct IngestObserver<'a> {
    cancel: &'a CancellationToken,
    progress: &'a (dyn Fn(IngestProgress) + Send + Sync),
}

impl PrepassObserver for IngestObserver<'_> {
    fn progress(&mut self, update: PrepassProgress) {
        let stage = match update.stage {
            PrepassStage::Parsing => IngestStage::Parsing,
            PrepassStage::Completing => IngestStage::Completing,
            PrepassStage::Renaming => IngestStage::Renaming,
            PrepassStage::Merging => IngestStage::Merging,
        };
        (self.progress)(IngestProgress {
            stage,
            done: update.done,
            total: update.total,
            name: update.path.as_deref().map(display_name).unwrap_or_default(),
        });
    }

    fn cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }
}

/// Move a dropped file into the library folder.
///
/// A file that already lives there stays as it is; so does a file whose
/// content is already in the folder under its metadata name — a re-drop must
/// not create a second copy that GROBID would parse and merge again. A file
/// from outside is copied (the original is the user's, not ours); when the
/// destination name is taken by different content, the copy gets a number.
pub(crate) fn bring_into_folder(source: &Path, folder: &Path) -> Result<PathBuf> {
    anyhow::ensure!(source.is_file(), "not a readable file");
    if in_folder(source, folder) {
        return Ok(source.to_path_buf());
    }
    if let Some(existing) = find_identical_in_folder(source, folder)? {
        return Ok(existing);
    }
    let name = source
        .file_name()
        .context("the file has no name")?
        .to_os_string();
    let destination = folder.join(&name);
    let destination = if destination.exists() {
        numbered(folder, &name)?
    } else {
        destination
    };
    std::fs::copy(source, &destination).with_context(|| {
        format!(
            "cannot copy {} to {}",
            source.display(),
            destination.display()
        )
    })?;
    Ok(destination)
}

/// The first PDF in `folder` whose content equals `source`.
///
/// Sizes are compared from metadata first, so a drop scans the folder but
/// hashes nothing unless a candidate of the same size exists.
fn find_identical_in_folder(source: &Path, folder: &Path) -> Result<Option<PathBuf>> {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Ok(None);
    };
    let source_len = std::fs::metadata(source)?.len();
    let mut source_hash: Option<blake3::Hash> = None;
    for entry in entries.flatten() {
        let candidate = entry.path();
        if !candidate.is_file() || !is_pdf_path(&candidate) {
            continue;
        }
        let Ok(metadata) = std::fs::metadata(&candidate) else {
            continue;
        };
        if metadata.len() != source_len {
            continue;
        }
        let source_hash = match source_hash {
            Some(hash) => hash,
            None => {
                let hash = hash_file(source)?;
                source_hash = Some(hash);
                hash
            }
        };
        if let Ok(candidate_hash) = hash_file(&candidate) {
            if candidate_hash == source_hash {
                return Ok(Some(candidate));
            }
        }
    }
    Ok(None)
}

/// Whether `path`'s directory is `folder` (both canonicalized).
fn in_folder(path: &Path, folder: &Path) -> bool {
    let parent = path.parent().and_then(|parent| parent.canonicalize().ok());
    let folder = folder.canonicalize().ok();
    match (parent, folder) {
        (Some(parent), Some(folder)) => parent == folder,
        _ => false,
    }
}

/// `name.pdf`, `name-2.pdf`, `name-3.pdf`, … (first free name).
fn numbered(folder: &Path, name: &std::ffi::OsStr) -> Result<PathBuf> {
    let name = Path::new(name);
    let stem = name
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "document".to_string());
    let extension = name
        .extension()
        .map(|ext| ext.to_string_lossy().into_owned());
    for suffix in 2..=999u32 {
        let candidate = match &extension {
            Some(extension) => folder.join(format!("{stem}-{suffix}.{extension}")),
            None => folder.join(format!("{stem}-{suffix}")),
        };
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    bail!("cannot find a free file name for {stem}")
}

fn hash_file(path: &Path) -> Result<blake3::Hash> {
    let file =
        std::fs::File::open(path).with_context(|| format!("cannot read {}", path.display()))?;
    let mut hasher = blake3::Hasher::new();
    hasher
        .update_reader(file)
        .with_context(|| format!("cannot hash {}", path.display()))?;
    Ok(hasher.finalize())
}

/// Whether `path` names a PDF (extension check; also drop routing).
pub fn is_pdf_path(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"))
}

fn display_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// Turn cancellation into the marker error the worker matches on.
fn check_cancelled(cancel: &CancellationToken) -> Result<()> {
    if cancel.is_cancelled() {
        return Err(anyhow::Error::new(Cancelled));
    }
    Ok(())
}

// ── Download ────────────────────────────────────────────────────────────────

/// Download `url` into `folder` as a PDF, without clobbering existing files.
///
/// The body is capped (50 MiB, like ragrig's `fetch_url`) and has to start
/// with a PDF signature: a URL that serves HTML (a landing page) is a user
/// error worth reporting, not something to hand to GROBID.
async fn download(url: &str, folder: &Path, cancel: &CancellationToken) -> Result<PathBuf> {
    let folder = folder.to_path_buf();
    let url = url.to_string();
    let cancel = cancel.clone();
    tokio::task::spawn_blocking(move || download_blocking(&url, &folder, &cancel)).await?
}

fn download_blocking(url: &str, folder: &Path, cancel: &CancellationToken) -> Result<PathBuf> {
    let parsed = url::Url::parse(url).with_context(|| format!("not a URL: {url}"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        bail!("only http(s) URLs can be downloaded");
    }

    let agent = ureq::AgentBuilder::new()
        .timeout_connect(DOWNLOAD_CONNECT_TIMEOUT)
        .timeout_read(DOWNLOAD_READ_TIMEOUT)
        .user_agent(concat!("sciscraper/", env!("CARGO_PKG_VERSION")))
        .build();
    let response = agent
        .get(url)
        .call()
        .with_context(|| format!("download failed for {url}"))?;
    let disposition = response.header("content-disposition").map(str::to_string);
    let content_type = response.header("content-type").map(str::to_string);

    let mut bytes = Vec::new();
    let mut reader = response.into_reader();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        check_cancelled(cancel)?;
        let read = std::io::Read::read(&mut reader, &mut buffer)
            .with_context(|| format!("download failed for {url}"))?;
        if read == 0 {
            break;
        }
        if bytes.len() as u64 + read as u64 > MAX_DOWNLOAD_BYTES {
            bail!(
                "the download exceeds the {} MiB limit",
                MAX_DOWNLOAD_BYTES / 1024 / 1024
            );
        }
        bytes.extend_from_slice(&buffer[..read]);
    }
    if !bytes.starts_with(b"%PDF-") {
        bail!(
            "the URL did not return a PDF (content type: {})",
            content_type.as_deref().unwrap_or("unknown")
        );
    }
    check_cancelled(cancel)?;

    let name = pdf_name(&parsed, disposition.as_deref());
    let destination = if folder.join(&name).exists() {
        numbered(folder, std::ffi::OsStr::new(&name))?
    } else {
        folder.join(&name)
    };
    std::fs::write(&destination, &bytes)
        .with_context(|| format!("cannot write the download to {}", destination.display()))?;
    log::info!(
        "Ingest: downloaded {url} \u{2192} {}",
        destination.display()
    );
    Ok(destination)
}

/// The file name for a download: `content-disposition` first, then the last
/// URL path segment; always sanitized and `.pdf`-suffixed.
fn pdf_name(url: &url::Url, content_disposition: Option<&str>) -> String {
    let from_header = content_disposition.and_then(file_name_from_disposition);
    let from_url = || {
        url.path_segments()
            .and_then(|mut segments| {
                segments
                    .rfind(|segment| !segment.is_empty())
                    .map(percent_decode)
            })
            .unwrap_or_else(|| "download".to_string())
    };
    let raw = from_header.unwrap_or_else(from_url);
    let sanitized = sanitize_file_name(&raw);
    if sanitized.to_ascii_lowercase().ends_with(".pdf") {
        sanitized
    } else {
        format!("{sanitized}.pdf")
    }
}

/// `filename*=UTF-8''…` (RFC 5987) wins over `filename=…`.
fn file_name_from_disposition(value: &str) -> Option<String> {
    for part in value.split(';') {
        let part = part.trim();
        if let Some(rest) = part.strip_prefix("filename*=") {
            let rest = rest.trim_matches('"');
            let encoded = rest
                .split_once("''")
                .map(|(_, value)| value)
                .unwrap_or(rest);
            return Some(percent_decode(encoded));
        }
    }
    for part in value.split(';') {
        let part = part.trim();
        if let Some(rest) = part.strip_prefix("filename=") {
            let name = rest.trim_matches('"').trim();
            if !name.is_empty() {
                return Some(name.to_string());
            }
        }
    }
    None
}

/// Keep `[A-Za-z0-9._-]`, replace everything else; never empty.
fn sanitize_file_name(name: &str) -> String {
    let sanitized: String = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = sanitized.trim_matches(['.', '_']);
    if trimmed.is_empty() {
        "download".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Minimal percent-decoder for file names (`%20` → space).
fn percent_decode(value: &str) -> String {
    fn hex(byte: u8) -> Option<u8> {
        (byte as char).to_digit(16).map(|digit| digit as u8)
    }
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let (Some(high), Some(low)) = (hex(bytes[index + 1]), hex(bytes[index + 2])) {
                decoded.push(high * 16 + low);
                index += 3;
                continue;
            }
        }
        decoded.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

/// Test doubles shared with the worker tests: a minimal GROBID server.
#[cfg(test)]
pub(crate) mod mock {
    use std::net::SocketAddr;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// A header whose title is “Tiny.” stays below the OpenAlex search
    /// threshold, so nothing reaches the OpenAlex API.
    pub(crate) const MOCK_TEI: &str = r#"<TEI xmlns="http://www.tei-c.org/ns/1.0">
    <teiHeader>
        <encodingDesc><appInfo>
            <application version="0.9.1" when="2026-01-01T00:00+0000"/>
        </appInfo></encodingDesc>
        <fileDesc>
            <titleStmt><title level="a" type="main">Tiny</title></titleStmt>
            <publicationStmt><publisher/></publicationStmt>
            <sourceDesc>
                <biblStruct>
                    <analytic>
                        <title level="a" type="main">Tiny.</title>
                        <author><persName>
                            <forename type="first">Jane</forename>
                            <surname>Smith</surname>
                        </persName></author>
                    </analytic>
                    <monogr><imprint><date type="published" when="2020"/></imprint></monogr>
                </biblStruct>
            </sourceDesc>
        </fileDesc>
    </teiHeader>
    <text/>
</TEI>"#;

    /// A header with a title long enough to trigger an OpenAlex title search.
    pub(crate) const MOCK_TEI_LONG_TITLE: &str = r#"<TEI xmlns="http://www.tei-c.org/ns/1.0">
    <teiHeader>
        <encodingDesc><appInfo>
            <application version="0.9.1" when="2026-01-01T00:00+0000"/>
        </appInfo></encodingDesc>
        <fileDesc>
            <titleStmt><title level="a" type="main">The Barc model</title></titleStmt>
            <publicationStmt><publisher/></publicationStmt>
            <sourceDesc>
                <biblStruct>
                    <analytic>
                        <title level="a" type="main">The Barc model for continuous variables</title>
                        <author><persName><surname>Kahle</surname></persName></author>
                    </analytic>
                    <monogr><imprint><date type="published" when="2000"/></imprint></monogr>
                </biblStruct>
            </sourceDesc>
        </fileDesc>
    </teiHeader>
    <text/>
</TEI>"#;

    /// Serve the two GROBID endpoints the pre-pass uses; everything else is
    /// 404, so a misrouted OpenAlex base URL fails fast. Returns the address
    /// and the number of header parses served.
    pub(crate) async fn spawn_mock_grobid(tei: &'static str) -> (SocketAddr, Arc<AtomicUsize>) {
        use tokio::io::AsyncWriteExt;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock server");
        let addr = listener.local_addr().expect("local addr");
        let parses = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&parses);
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let counter = Arc::clone(&counter);
                tokio::spawn(async move {
                    let Some(head) = read_request_head(&mut stream).await else {
                        return;
                    };
                    let request_line = head.lines().next().unwrap_or("");
                    let (status, body) = if request_line.contains("/api/isalive") {
                        ("200 OK", "true".to_string())
                    } else if request_line.contains("/api/processHeaderDocument") {
                        counter.fetch_add(1, Ordering::SeqCst);
                        ("200 OK", tei.to_string())
                    } else {
                        ("404 Not Found", String::new())
                    };
                    let response = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: text/plain\r\n\
                         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                });
            }
        });
        (addr, parses)
    }

    /// Read a full request (headers and body, by `Content-Length`) and return
    /// the header block, so the client never sees a connection closed while it
    /// is still uploading.
    async fn read_request_head(stream: &mut tokio::net::TcpStream) -> Option<String> {
        use tokio::io::AsyncReadExt;

        let mut buf = Vec::new();
        let mut tmp = [0u8; 4096];
        loop {
            let n = stream.read(&mut tmp).await.ok()?;
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&tmp[..n]);
            let Some(header_end) = buf.windows(4).position(|w| w == b"\r\n\r\n") else {
                continue;
            };
            let header_end = header_end + 4;
            let head = String::from_utf8_lossy(&buf[..header_end]).into_owned();
            let content_length = head
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    if name.eq_ignore_ascii_case("content-length") {
                        value.trim().parse::<usize>().ok()
                    } else {
                        None
                    }
                })
                .unwrap_or(0);
            if buf.len() >= header_end + content_length {
                return Some(head);
            }
        }
        Some(String::from_utf8_lossy(&buf).into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;
    use std::sync::Arc;

    use grobid_bibtex::files::{self, Manifest};

    fn test_options(grobid_url: String) -> IngestOptions {
        IngestOptions {
            grobid_url,
            openalex: false,
            workers: 2,
            probe_attempts: 1,
            probe_delay: Duration::ZERO,
            ..IngestOptions::default()
        }
    }

    fn no_progress() -> impl Fn(IngestProgress) + Send + Sync {
        |_| {}
    }

    #[test]
    fn url_like_detects_http_urls_only() {
        assert!(url_like("https://example.org/paper.pdf"));
        assert!(url_like("  http://example.org/a.pdf  "));
        assert!(!url_like("/home/me/paper.pdf"));
        assert!(!url_like("C:\\Users\\me\\paper.pdf"));
        assert!(!url_like("file:///home/me/paper.pdf"));
        assert!(!url_like("just text"));
        assert!(!url_like(""));
    }

    #[test]
    fn dropped_paths_are_classified() {
        let sources = sources_from_paths(&[
            PathBuf::from("/tmp/scan.pdf"),
            PathBuf::from("https://example.org/paper.pdf"),
        ]);
        assert_eq!(
            sources,
            vec![
                IngestSource::File(PathBuf::from("/tmp/scan.pdf")),
                IngestSource::Url("https://example.org/paper.pdf".to_string()),
            ]
        );
    }

    #[test]
    fn drop_routing_keeps_non_pdf_drops_with_the_editor() {
        let dir = tempfile::tempdir().unwrap();
        let pdf = dir.path().join("paper.pdf");
        std::fs::write(&pdf, b"%PDF-1.4 x").unwrap();
        let notes = dir.path().join("notes.md");
        std::fs::write(&notes, "# Notes").unwrap();
        let folder = dir.path().join("papers");
        std::fs::create_dir_all(&folder).unwrap();
        // A PDF-shaped path that does not exist is not ingestible either.
        let ghost = dir.path().join("ghost.pdf");

        // PDFs and URLs route to the library…
        assert!(drop_sources(&[pdf.clone()]).is_some());
        assert!(drop_sources(&[PathBuf::from("https://example.org/a.pdf")]).is_some());
        // …a mixed drop is one gesture: everything goes to the run, which
        // reports what it skipped.
        let mixed = drop_sources(&[pdf.clone(), folder.clone()]).expect("pdf routes the drop");
        assert_eq!(mixed.len(), 2);
        assert!(
            !mixed[1].is_ingestible(),
            "the folder is reported as skipped"
        );

        // Documents, folders and missing files keep the editor's behaviour.
        assert!(drop_sources(&[]).is_none());
        assert!(drop_sources(&[notes]).is_none());
        assert!(drop_sources(&[folder]).is_none());
        assert!(drop_sources(&[ghost]).is_none());
    }

    #[test]
    fn sanitized_names_stay_in_a_safe_subset() {
        assert_eq!(
            sanitize_file_name("My Paper (final).pdf"),
            "My_Paper__final_.pdf"
        );
        assert_eq!(sanitize_file_name("../../etc/passwd"), "etc_passwd");
        assert_eq!(sanitize_file_name("   "), "download");
    }

    #[test]
    fn disposition_prefers_the_rfc_5987_form() {
        assert_eq!(
            file_name_from_disposition(
                "attachment; filename=\"a.pdf\"; filename*=UTF-8''b%20c.pdf"
            ),
            Some("b c.pdf".to_string())
        );
        assert_eq!(
            file_name_from_disposition("inline; filename=\"paper.pdf\""),
            Some("paper.pdf".to_string())
        );
        assert_eq!(file_name_from_disposition("attachment"), None);
    }

    #[test]
    fn pdf_name_falls_back_to_the_url_segment() {
        let url = url::Url::parse("https://example.org/dir/My%20Paper.pdf?token=1").unwrap();
        assert_eq!(pdf_name(&url, None), "My_Paper.pdf");
        assert_eq!(
            pdf_name(&url, Some("attachment; filename=renamed.pdf")),
            "renamed.pdf"
        );
        let bare = url::Url::parse("https://example.org/").unwrap();
        assert_eq!(pdf_name(&bare, None), "download.pdf");

        // `%20` is decoded before sanitizing, so it becomes `_`.
        let encoded = url::Url::parse("https://example.org/a%20b.pdf").unwrap();
        assert_eq!(pdf_name(&encoded, None), "a_b.pdf");
    }

    #[test]
    fn files_are_brought_into_the_library_folder_without_clobbering() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("library");
        let incoming = dir.path().join("incoming");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::create_dir_all(&incoming).unwrap();

        // In-folder files stay where they are.
        let inside = folder.join("inside.pdf");
        std::fs::write(&inside, b"%PDF-1.4 a").unwrap();
        assert_eq!(bring_into_folder(&inside, &folder).unwrap(), inside);

        // A file from outside is copied, the original stays.
        let outside = incoming.join("paper.pdf");
        std::fs::write(&outside, b"%PDF-1.4 b").unwrap();
        let placed = bring_into_folder(&outside, &folder).unwrap();
        assert_eq!(placed, folder.join("paper.pdf"));
        assert!(outside.exists(), "the dropped original must survive");

        // Re-dropping the identical file reuses the copy…
        assert_eq!(bring_into_folder(&outside, &folder).unwrap(), placed);

        // …while a same-named file with different content gets a number.
        std::fs::write(&outside, b"%PDF-1.4 c").unwrap();
        let numbered = bring_into_folder(&outside, &folder).unwrap();
        assert_eq!(numbered, folder.join("paper-2.pdf"));
        assert_eq!(std::fs::read(&numbered).unwrap(), b"%PDF-1.4 c");
    }

    /// A one-shot HTTP server for the download tests.
    fn spawn_download_server(
        body: &'static [u8],
        content_type: &'static str,
        disposition: Option<&'static str>,
    ) -> std::net::SocketAddr {
        use std::io::{Read, Write};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut request = [0u8; 2048];
                let _ = stream.read(&mut request);
                let disposition = disposition
                    .map(|value| format!("Content-Disposition: {value}\r\n"))
                    .unwrap_or_default();
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\n{disposition}\
                     Content-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(body);
            }
        });
        addr
    }

    #[test]
    fn download_writes_the_pdf_under_a_sanitized_name() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("library");
        std::fs::create_dir_all(&folder).unwrap();
        let addr = spawn_download_server(
            b"%PDF-1.4 downloaded",
            "application/pdf",
            Some("attachment; filename=\"My Paper.pdf\""),
        );
        let cancel = CancellationToken::new();
        let path = download_blocking(
            &format!("http://{addr}/some/landing-page"),
            &folder,
            &cancel,
        )
        .expect("download succeeds");
        assert_eq!(path, folder.join("My_Paper.pdf"));
        assert_eq!(std::fs::read(&path).unwrap(), b"%PDF-1.4 downloaded");

        // A second download of the same name is numbered, not overwritten.
        let second = download_blocking(
            &format!("http://{addr}/some/landing-page"),
            &folder,
            &cancel,
        )
        .unwrap();
        assert_eq!(second, folder.join("My_Paper-2.pdf"));
    }

    #[test]
    fn download_rejects_non_pdf_bodies() {
        let dir = tempfile::tempdir().unwrap();
        let addr = spawn_download_server(b"<!doctype html>", "text/html", None);
        let cancel = CancellationToken::new();
        let err = download_blocking(&format!("http://{addr}/page"), dir.path(), &cancel)
            .expect_err("HTML must be rejected");
        let message = format!("{err:#}");
        assert!(
            message.contains("text/html"),
            "unexpected message: {message}"
        );
        assert!(
            std::fs::read_dir(dir.path()).unwrap().next().is_none(),
            "nothing must be written for a rejected download"
        );
    }

    fn write_pdf(path: &Path) {
        std::fs::write(path, b"%PDF-1.4 fake").unwrap();
    }

    #[tokio::test]
    async fn run_extracts_renames_and_merges_against_a_mock_server() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("library");
        let incoming = dir.path().join("incoming");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::create_dir_all(&incoming).unwrap();
        let source = incoming.join("scan_0012.pdf");
        write_pdf(&source);
        let bib = folder.join("library.bib");
        let (addr, parses) = mock::spawn_mock_grobid(mock::MOCK_TEI).await;

        let sources = [IngestSource::File(source.clone())];
        let targets = IngestTargets {
            folder: &folder,
            bibtex: Some(&bib),
            link: true,
        };
        let outcome = run(
            &sources,
            &targets,
            &test_options(format!("http://{addr}")),
            &CancellationToken::new(),
            &no_progress(),
        )
        .await
        .expect("run succeeds");

        assert_eq!(outcome.processed, 1);
        assert_eq!(outcome.renamed, 1);
        assert_eq!(outcome.merged, 1);
        assert!(outcome.failed.is_empty(), "{:?}", outcome.failed);
        assert!(outcome.skipped.is_empty(), "{:?}", outcome.skipped);
        assert!(!outcome.nothing_new());
        assert_eq!(parses.load(Ordering::SeqCst), 1, "one header parsed");
        assert!(source.exists(), "the dropped original must survive");

        // Renamed to `Key - Full authors - Full title - Year.pdf`.
        let pdfs = files::collect_pdfs(&folder).unwrap();
        assert_eq!(pdfs.len(), 1, "{pdfs:?}");
        let name = pdfs[0].file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("Smith2020 - "), "{name}");
        assert!(name.contains("Tiny"), "{name}");
        assert!(name.ends_with(" - 2020.pdf"), "{name}");

        // The entry links the final name.
        let text = std::fs::read_to_string(&bib).unwrap();
        assert!(text.starts_with("@misc{Smith2020,"), "{text}");
        assert!(
            text.contains(&format!("file = {{{}}}", pdfs[0].display())),
            "{text}"
        );

        // The manifest caches the record, so re-runs need no GROBID.
        let manifest = Manifest::load(&folder.join(prepass::MANIFEST_NAME)).unwrap();
        assert!(manifest.biblio(&folder, &pdfs[0]).is_some());

        // Re-dropping the same file is detected, merges a duplicate and
        // parses nothing.
        let outcome = run(
            &sources,
            &targets,
            &test_options(format!("http://{addr}")),
            &CancellationToken::new(),
            &no_progress(),
        )
        .await
        .expect("second run succeeds");
        assert!(outcome.nothing_new(), "{outcome:?}");
        assert_eq!(outcome.duplicates, 1);
        assert_eq!(parses.load(Ordering::SeqCst), 1, "still one header parse");
        assert_eq!(std::fs::read_to_string(&bib).unwrap(), text);
    }

    #[tokio::test]
    async fn completion_failures_keep_the_parsed_record() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("library");
        std::fs::create_dir_all(&folder).unwrap();
        let source = folder.join("scan.pdf");
        write_pdf(&source);
        let (addr, _) = mock::spawn_mock_grobid(mock::MOCK_TEI_LONG_TITLE).await;

        // The mock answers 404 for OpenAlex requests: the lookup fails, the
        // GROBID header is kept and the run still succeeds.
        let mut options = test_options(format!("http://{addr}"));
        options.openalex = true;
        options.openalex_url = Some(format!("http://{addr}"));
        let sources = [IngestSource::File(source)];
        let targets = IngestTargets {
            folder: &folder,
            bibtex: None,
            link: false,
        };
        let outcome = run(
            &sources,
            &targets,
            &options,
            &CancellationToken::new(),
            &no_progress(),
        )
        .await
        .expect("run succeeds");

        assert_eq!(outcome.completed, 0, "the OpenAlex lookup failed");
        assert_eq!(outcome.renamed, 1, "the parsed name is still applied");
        assert!(outcome.failed.is_empty(), "{:?}", outcome.failed);
    }

    #[tokio::test]
    async fn unreachable_grobid_fails_the_run() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("library");
        std::fs::create_dir_all(&folder).unwrap();
        let source = folder.join("scan.pdf");
        write_pdf(&source);

        let sources = [IngestSource::File(source)];
        let targets = IngestTargets {
            folder: &folder,
            bibtex: None,
            link: false,
        };
        let err = run(
            &sources,
            &targets,
            // Port 1 on localhost refuses connections immediately.
            &test_options("http://127.0.0.1:1".to_string()),
            &CancellationToken::new(),
            &no_progress(),
        )
        .await
        .expect_err("an unreachable server aborts the run");
        let message = format!("{err:#}");
        assert!(
            message.contains("not ready"),
            "unexpected message: {message}"
        );
    }

    #[tokio::test]
    async fn folders_and_non_pdfs_are_skipped_with_a_reason() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("library");
        std::fs::create_dir_all(&folder).unwrap();
        let notes = folder.join("notes.md");
        std::fs::write(&notes, "# Notes").unwrap();

        let sources = [
            IngestSource::File(folder.clone()),
            IngestSource::File(notes.clone()),
        ];
        let targets = IngestTargets {
            folder: &folder,
            bibtex: None,
            link: false,
        };
        let outcome = run(
            &sources,
            &targets,
            &test_options("http://127.0.0.1:1".to_string()),
            &CancellationToken::new(),
            &no_progress(),
        )
        .await
        .expect("skipping is not an error");

        assert_eq!(outcome.skipped.len(), 2, "{:?}", outcome.skipped);
        assert!(outcome.skipped[0].contains("folders are not ingested"));
        assert!(outcome.skipped[1].contains("not a PDF"));
        assert_eq!(outcome.processed, 0);
        // Nothing was parsed, so the unreachable server never mattered.
    }

    #[tokio::test]
    async fn cancellation_stops_the_run_with_the_marker_error() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("library");
        std::fs::create_dir_all(&folder).unwrap();
        let cancel = CancellationToken::new();
        cancel.cancel();

        let sources = [IngestSource::Url("https://example.org/paper.pdf".into())];
        let targets = IngestTargets {
            folder: &folder,
            bibtex: None,
            link: false,
        };
        let err = run(
            &sources,
            &targets,
            &test_options("http://127.0.0.1:1".to_string()),
            &cancel,
            &no_progress(),
        )
        .await
        .expect_err("cancelled before the download");
        assert!(
            err.downcast_ref::<Cancelled>().is_some(),
            "the worker matches on `Cancelled`: {err:#}"
        );
    }

    #[tokio::test]
    async fn corrupt_bibliography_fails_before_any_download() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("library");
        std::fs::create_dir_all(&folder).unwrap();
        let bib = folder.join("library.bib");
        std::fs::write(&bib, "@misc{ broken").unwrap();

        let sources = [IngestSource::Url("https://example.org/paper.pdf".into())];
        let targets = IngestTargets {
            folder: &folder,
            bibtex: Some(&bib),
            link: false,
        };
        let err = run(
            &sources,
            &targets,
            &test_options("http://127.0.0.1:1".to_string()),
            &CancellationToken::new(),
            &no_progress(),
        )
        .await
        .expect_err("a corrupt bibliography aborts the run");
        let message = format!("{err:#}");
        assert!(message.contains("BibTeX"), "unexpected message: {message}");
        assert!(
            files::collect_pdfs(&folder).unwrap().is_empty(),
            "no PDF may be downloaded or renamed"
        );
    }

    #[tokio::test]
    async fn progress_reports_every_stage() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("library");
        std::fs::create_dir_all(&folder).unwrap();
        let source = folder.join("scan.pdf");
        write_pdf(&source);
        let (addr, _) = mock::spawn_mock_grobid(mock::MOCK_TEI).await;

        let stages = Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorded = Arc::clone(&stages);
        let progress = move |update: IngestProgress| {
            recorded.lock().unwrap().push(update.stage);
        };
        let sources = [IngestSource::File(source)];
        let targets = IngestTargets {
            folder: &folder,
            bibtex: Some(&folder.join("library.bib")),
            link: true,
        };
        run(
            &sources,
            &targets,
            &test_options(format!("http://{addr}")),
            &CancellationToken::new(),
            &progress,
        )
        .await
        .expect("run succeeds");

        let stages = stages.lock().unwrap().clone();
        assert!(stages.contains(&IngestStage::Parsing), "{stages:?}");
        assert!(stages.contains(&IngestStage::Renaming), "{stages:?}");
        assert!(stages.contains(&IngestStage::Merging), "{stages:?}");
        assert!(!stages.contains(&IngestStage::Downloading), "{stages:?}");
    }
}
