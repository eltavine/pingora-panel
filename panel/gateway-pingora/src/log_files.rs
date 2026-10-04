//! The gateway's log files (ADR 0025). Workers queue records on a bounded
//! channel; one writer thread appends them, rotates files by size and day
//! and prunes rotated files by age and count. A record that finds the queue
//! full is dropped and counted.

use chrono::{DateTime, NaiveDate, NaiveDateTime, TimeDelta, Utc};
use panel_ir::LogFiles;
use panel_metrics::Metrics;
use prometheus_client::{
    encoding::EncodeLabelSet,
    metrics::{counter::Counter, family::Family},
    registry::Unit,
};
use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{self, BufWriter, Write},
    path::{Path, PathBuf},
    sync::{
        mpsc::{self, Receiver, RecvTimeoutError, SyncSender},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

/// Records waiting for the writer.
const QUEUE: usize = 16_384;
/// Records written between flushes while the queue stays busy.
const BATCH: usize = 1024;
/// How often the writer looks for files to close while idle.
const MAINTENANCE: Duration = Duration::from_secs(1);
/// Files without a record for this long are closed.
const IDLE: Duration = Duration::from_secs(300);
const BUFFER: usize = 64 * 1024;
/// The UTC time a rotated file's name ends with.
const ROTATED: &str = "%Y%m%dT%H%M%SZ";

/// The file a record goes to.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) enum Destination {
    /// Requests no site took.
    Gateway,
    /// Requests the site took.
    Site(Arc<str>),
    Errors,
}

impl Destination {
    /// The site is part of the file name, never a directory, so any site
    /// identifier stays inside the log directory.
    fn path(&self, directory: &Path) -> PathBuf {
        match self {
            Self::Gateway => directory.join("access.log"),
            Self::Site(site) => directory.join("sites").join(format!("{site}.access.log")),
            Self::Errors => directory.join("error.log"),
        }
    }

    fn log(&self) -> &'static str {
        match self {
            Self::Errors => "error",
            Self::Gateway | Self::Site(_) => "access",
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq, EncodeLabelSet)]
struct LogLabels {
    log: &'static str,
}

#[derive(Clone, Debug)]
struct LogMetrics {
    records: Family<LogLabels, Counter>,
    bytes: Family<LogLabels, Counter>,
    dropped: Family<LogLabels, Counter>,
}

impl LogMetrics {
    fn register(metrics: &mut Metrics) -> Self {
        let log = Self {
            records: Family::default(),
            bytes: Family::default(),
            dropped: Family::default(),
        };
        let registry = metrics.registry();
        registry.register(
            "pingora_panel_log_records",
            "Number of log records written",
            log.records.clone(),
        );
        registry.register_with_unit(
            "pingora_panel_log",
            "Size of the log records written",
            Unit::Bytes,
            log.bytes.clone(),
        );
        registry.register(
            "pingora_panel_log_records_dropped",
            "Number of log records dropped because the writer fell behind or could not write",
            log.dropped.clone(),
        );
        for name in ["access", "error"] {
            let labels = LogLabels { log: name };
            drop(log.records.get_or_create(&labels));
            drop(log.bytes.get_or_create(&labels));
            drop(log.dropped.get_or_create(&labels));
        }
        log
    }

    fn dropped(&self, log: &'static str) {
        self.dropped.get_or_create(&LogLabels { log }).inc();
    }
}

struct Entry {
    destination: Destination,
    line: Vec<u8>,
    files: LogFiles,
}

/// Where the data plane sends its records; cloned into every listener.
#[derive(Clone, Debug)]
pub struct Logs {
    sender: SyncSender<Entry>,
    metrics: LogMetrics,
}

impl Logs {
    /// Starts the writer for the log files under `directory`. It stops once
    /// every clone is dropped, after writing what is queued.
    pub fn start(directory: impl Into<PathBuf>, metrics: &mut Metrics) -> io::Result<Self> {
        let directory = directory.into();
        fs::create_dir_all(directory.join("sites"))?;
        let (sender, receiver) = mpsc::sync_channel(QUEUE);
        let metrics = LogMetrics::register(metrics);
        let writer = Writer {
            directory,
            files: HashMap::new(),
            metrics: metrics.clone(),
            warned: None,
        };
        thread::Builder::new()
            .name("gateway-logs".to_owned())
            .spawn(move || writer.run(&receiver))?;
        Ok(Self { sender, metrics })
    }

    /// Queues `line` for `destination`, or drops and counts it when the
    /// writer is behind.
    pub(crate) fn send(&self, destination: Destination, line: Vec<u8>, files: LogFiles) {
        let log = destination.log();
        let entry = Entry {
            destination,
            line,
            files,
        };
        if self.sender.try_send(entry).is_err() {
            self.metrics.dropped(log);
        }
    }
}

struct Writer {
    directory: PathBuf,
    files: HashMap<Destination, RotatingFile>,
    metrics: LogMetrics,
    /// When the writer last reported a failed write.
    warned: Option<Instant>,
}

impl Writer {
    fn run(mut self, receiver: &Receiver<Entry>) {
        loop {
            match receiver.recv_timeout(MAINTENANCE) {
                Ok(entry) => {
                    self.write(entry);
                    let mut written = 1;
                    while let Ok(entry) = receiver.try_recv() {
                        self.write(entry);
                        written += 1;
                        if written % BATCH == 0 {
                            self.flush();
                        }
                    }
                    self.flush();
                }
                Err(RecvTimeoutError::Timeout) => self.close_idle(),
                Err(RecvTimeoutError::Disconnected) => {
                    self.flush();
                    return;
                }
            }
        }
    }

    fn write(&mut self, entry: Entry) {
        let log = entry.destination.log();
        let now = Utc::now();
        let result = match self.files.get_mut(&entry.destination) {
            Some(file) => file.write(&entry.line, entry.files, now),
            None => RotatingFile::open(entry.destination.path(&self.directory), entry.files, now)
                .and_then(|file| {
                    self.files.entry(entry.destination).or_insert(file).write(
                        &entry.line,
                        entry.files,
                        now,
                    )
                }),
        };
        match result {
            Ok(()) => {
                let labels = LogLabels { log };
                self.metrics.records.get_or_create(&labels).inc();
                self.metrics
                    .bytes
                    .get_or_create(&labels)
                    .inc_by(entry.line.len() as u64);
            }
            Err(error) => {
                self.metrics.dropped(log);
                if self
                    .warned
                    .is_none_or(|warned| warned.elapsed() >= Duration::from_secs(60))
                {
                    self.warned = Some(Instant::now());
                    tracing::warn!(%error, "log records cannot be written");
                }
            }
        }
    }

    fn flush(&mut self) {
        for file in self.files.values_mut() {
            if let Err(error) = file.flush() {
                tracing::warn!(%error, path = %file.path.display(), "log file cannot be flushed");
            }
        }
    }

    fn close_idle(&mut self) {
        self.files.retain(|_, file| {
            if file.written.elapsed() < IDLE {
                return true;
            }
            let _ = file.flush();
            false
        });
    }
}

fn open_append(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o640);
    options.open(path)
}

struct RotatingFile {
    path: PathBuf,
    writer: BufWriter<File>,
    size: u64,
    /// The UTC day of the file's latest record.
    day: NaiveDate,
    written: Instant,
    dirty: bool,
}

impl RotatingFile {
    fn open(path: PathBuf, limits: LogFiles, now: DateTime<Utc>) -> io::Result<Self> {
        let file = open_append(&path)?;
        let metadata = file.metadata()?;
        let day = match metadata.len() {
            0 => now,
            _ => metadata.modified().map_or(now, DateTime::<Utc>::from),
        }
        .date_naive();
        prune(&path, limits, now);
        Ok(Self {
            path,
            writer: BufWriter::with_capacity(BUFFER, file),
            size: metadata.len(),
            day,
            written: Instant::now(),
            dirty: false,
        })
    }

    fn write(&mut self, line: &[u8], limits: LogFiles, now: DateTime<Utc>) -> io::Result<()> {
        let today = now.date_naive();
        let length = line.len() as u64;
        let full = self.size.saturating_add(length) > limits.max_size_bytes;
        let new_day = limits.rotate_daily && today != self.day;
        if self.size > 0 && (full || new_day) {
            self.rotate(limits, now)?;
        }
        self.writer.write_all(line)?;
        self.size += length;
        self.day = today;
        self.written = Instant::now();
        self.dirty = true;
        Ok(())
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.dirty {
            self.writer.flush()?;
            self.dirty = false;
        }
        Ok(())
    }

    /// Renames the file with the time of the rotation and starts a new one.
    fn rotate(&mut self, limits: LogFiles, now: DateTime<Utc>) -> io::Result<()> {
        self.writer.flush()?;
        self.dirty = false;
        let stamp = now.format(ROTATED).to_string();
        let mut rotated = suffixed(&self.path, &stamp);
        let mut counter = 0;
        while rotated.exists() {
            counter += 1;
            rotated = suffixed(&self.path, &format!("{stamp}.{counter}"));
        }
        fs::rename(&self.path, &rotated)?;
        self.writer = BufWriter::with_capacity(BUFFER, open_append(&self.path)?);
        self.size = 0;
        prune(&self.path, limits, now);
        Ok(())
    }
}

fn suffixed(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".");
    name.push(suffix);
    PathBuf::from(name)
}

/// When a rotated file of a log was rotated, from the rest of its name.
fn rotated_at(suffix: &str) -> Option<DateTime<Utc>> {
    let stamp = match suffix.split_once('.') {
        Some((stamp, counter))
            if !counter.is_empty() && counter.bytes().all(|byte| byte.is_ascii_digit()) =>
        {
            stamp
        }
        Some(_) => return None,
        None => suffix,
    };
    NaiveDateTime::parse_from_str(stamp, ROTATED)
        .ok()
        .map(|time| time.and_utc())
}

/// Deletes the rotated files of `path` older than `keep_days` and, oldest
/// first, beyond `max_files`. Only names this module gives rotated files are
/// considered, so another log's files are never touched.
fn prune(path: &Path, limits: LogFiles, now: DateTime<Utc>) {
    let (Some(directory), Some(name)) = (path.parent(), path.file_name()) else {
        return;
    };
    let prefix = format!("{}.", name.to_string_lossy());
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    let mut rotated: Vec<(String, DateTime<Utc>, PathBuf)> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let file_name = entry.file_name().into_string().ok()?;
            let suffix = file_name.strip_prefix(&prefix)?;
            let at = rotated_at(suffix)?;
            Some((suffix.to_owned(), at, entry.path()))
        })
        .collect();
    rotated.sort_by(|left, right| left.1.cmp(&right.1).then_with(|| left.0.cmp(&right.0)));
    let excess = match limits.max_files {
        0 => 0,
        kept => rotated.len().saturating_sub(kept as usize),
    };
    let cutoff = (limits.keep_days > 0).then(|| now - TimeDelta::days(i64::from(limits.keep_days)));
    for (index, (_, at, path)) in rotated.iter().enumerate() {
        if index < excess || cutoff.is_some_and(|cutoff| *at < cutoff) {
            if let Err(error) = fs::remove_file(path) {
                tracing::warn!(%error, path = %path.display(), "rotated log file cannot be deleted");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits(max_size_bytes: u64, rotate_daily: bool, keep_days: u32, max_files: u32) -> LogFiles {
        LogFiles {
            max_size_bytes,
            rotate_daily,
            keep_days,
            max_files,
        }
    }

    fn at(text: &str) -> DateTime<Utc> {
        text.parse().unwrap()
    }

    fn names(directory: &Path) -> Vec<String> {
        let mut names: Vec<_> = fs::read_dir(directory)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn files_rotate_before_growing_past_their_size() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("access.log");
        let limits = limits(10, false, 0, 0);
        let now = at("2026-10-04T06:00:00Z");
        let mut file = RotatingFile::open(path.clone(), limits, now).unwrap();
        file.write(b"123456\n", limits, now).unwrap();
        file.write(b"abcdef\n", limits, now).unwrap();
        file.write(b"ABCDEF\n", limits, now).unwrap();
        file.flush().unwrap();
        assert_eq!(
            names(directory.path()),
            [
                "access.log",
                "access.log.20261004T060000Z",
                "access.log.20261004T060000Z.1"
            ]
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), "ABCDEF\n");
        assert_eq!(
            fs::read_to_string(directory.path().join("access.log.20261004T060000Z")).unwrap(),
            "123456\n"
        );
    }

    #[test]
    fn files_rotate_on_a_new_utc_day_when_asked() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("access.log");
        let daily = limits(1 << 20, true, 0, 0);
        let mut file = RotatingFile::open(path.clone(), daily, at("2026-10-04T23:59:59Z")).unwrap();
        file.write(b"late\n", daily, at("2026-10-04T23:59:59Z"))
            .unwrap();
        file.write(b"early\n", daily, at("2026-10-05T00:00:01Z"))
            .unwrap();
        file.flush().unwrap();
        assert_eq!(
            names(directory.path()),
            ["access.log", "access.log.20261005T000001Z"]
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), "early\n");

        let undaily = limits(1 << 20, false, 0, 0);
        file.write(b"next\n", undaily, at("2026-10-06T00:00:01Z"))
            .unwrap();
        file.flush().unwrap();
        assert_eq!(names(directory.path()).len(), 2);
    }

    #[test]
    fn rotated_files_are_pruned_by_age_and_count() {
        let directory = tempfile::tempdir().unwrap();
        for name in [
            "access.log",
            "access.log.20260920T000000Z",
            "access.log.20261001T000000Z",
            "access.log.20261002T000000Z",
            "access.log.20261003T000000Z",
            "access.log.20261003T000000Z.1",
            "access.log.notes",
            "other.log.20260101T000000Z",
        ] {
            fs::write(directory.path().join(name), "x").unwrap();
        }
        let path = directory.path().join("access.log");
        prune(
            &path,
            limits(1 << 20, true, 7, 3),
            at("2026-10-04T00:00:00Z"),
        );
        assert_eq!(
            names(directory.path()),
            [
                "access.log",
                "access.log.20261002T000000Z",
                "access.log.20261003T000000Z",
                "access.log.20261003T000000Z.1",
                "access.log.notes",
                "other.log.20260101T000000Z",
            ]
        );
        prune(
            &path,
            limits(1 << 20, true, 1, 0),
            at("2026-10-04T00:00:00Z"),
        );
        assert_eq!(
            names(directory.path()),
            [
                "access.log",
                "access.log.20261003T000000Z",
                "access.log.20261003T000000Z.1",
                "access.log.notes",
                "other.log.20260101T000000Z",
            ]
        );
    }

    #[test]
    fn site_files_stay_inside_the_log_directory() {
        let directory = Path::new("/var/log/pingora-panel");
        assert_eq!(
            Destination::Site(Arc::from("..")).path(directory),
            directory.join("sites").join("...access.log")
        );
        assert_eq!(
            Destination::Site(Arc::from("shop")).path(directory),
            directory.join("sites/shop.access.log")
        );
    }

    #[test]
    fn queued_records_are_written_and_a_full_queue_drops_and_counts() {
        let directory = tempfile::tempdir().unwrap();
        let mut metrics = Metrics::new();
        let logs = Logs::start(directory.path(), &mut metrics).unwrap();
        let files = LogFiles::default();
        logs.send(
            Destination::Site(Arc::from("shop")),
            b"one\n".to_vec(),
            files,
        );
        logs.send(Destination::Errors, b"two\n".to_vec(), files);
        drop(logs);
        let site = directory.path().join("sites/shop.access.log");
        let errors = directory.path().join("error.log");
        for _ in 0..200 {
            if fs::read_to_string(&site).is_ok_and(|text| text == "one\n")
                && fs::read_to_string(&errors).is_ok_and(|text| text == "two\n")
            {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(fs::read_to_string(&site).unwrap(), "one\n");
        assert_eq!(
            fs::read_to_string(directory.path().join("error.log")).unwrap(),
            "two\n"
        );
        let text = metrics.encode();
        assert!(
            text.contains("pingora_panel_log_records_total{log=\"access\"} 1"),
            "{text}"
        );
        assert!(
            text.contains("pingora_panel_log_bytes_total{log=\"error\"} 4"),
            "{text}"
        );

        let (sender, _receiver) = mpsc::sync_channel(0);
        let full = Logs {
            sender,
            metrics: LogMetrics::register(&mut Metrics::new()),
        };
        full.send(Destination::Gateway, b"lost\n".to_vec(), files);
        assert_eq!(
            full.metrics
                .dropped
                .get_or_create(&LogLabels { log: "access" })
                .get(),
            1
        );
    }
}
