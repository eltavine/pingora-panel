//! What containers print, as their engine's log driver keeps it (ADR 0031).
//! The engine stamps each line with the time it recorded it; a line comes
//! from standard output or standard error, and everything a container with
//! a terminal prints counts as standard output, as the engine records it.

use crate::containers::failure;
use bollard::{container::LogOutput, query_parameters::LogsOptionsBuilder, Docker};
use futures_util::{future::ready, Stream, StreamExt};
use panel_contracts::ops::v1::{self as wire, ContainerLogStream};
use panel_errors::PanelError;
use std::{
    collections::VecDeque,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

/// The most lines a read returns.
const MOST_LINES: u32 = 5_000;
/// The lines a read returns when the request names no number.
const DEFAULT_LINES: u32 = 200;
/// The most lines sent before following.
const MOST_BACKLOG: u32 = 1_000;
/// How far back a resumed follow looks for where it left off.
const RESUME_LINES: u32 = 5_000;
/// Where a line is cut. Engines keep longer output as entries of 16 KiB,
/// so a cut line is rare.
const LINE_BYTES: usize = 16 * 1024;
/// The most text a read returns, well within gRPC's 4 MiB message limit.
const READ_BYTES: usize = 2 * 1024 * 1024;
/// The most lines sent together while following: at most 2 MiB of text.
const BATCH: usize = 128;
/// How long a read may take.
pub(crate) const READ_TIMEOUT: Duration = Duration::from_secs(20);

struct Line {
    time: SystemTime,
    stream: ContainerLogStream,
    text: String,
}

impl Line {
    /// A line as the engine sends it with timestamps: when it was recorded,
    /// a space, then the text. One without a time, such as the rest of an
    /// entry a terminal split, takes the time of the line before.
    fn read(output: LogOutput, previous: Option<SystemTime>) -> Self {
        let (stream, message) = match output {
            LogOutput::StdErr { message } => (ContainerLogStream::Stderr, message),
            LogOutput::StdOut { message }
            | LogOutput::Console { message }
            | LogOutput::StdIn { message } => (ContainerLogStream::Stdout, message),
        };
        let mut bytes: &[u8] = &message;
        bytes = bytes.strip_suffix(b"\n").unwrap_or(bytes);
        bytes = bytes.strip_suffix(b"\r").unwrap_or(bytes);
        let stamped = bytes
            .iter()
            .position(|byte| *byte == b' ')
            .and_then(|space| {
                let time = std::str::from_utf8(&bytes[..space]).ok()?;
                let time = chrono::DateTime::parse_from_rfc3339(time).ok()?;
                Some((SystemTime::from(time), &bytes[space + 1..]))
            });
        let (time, text) = stamped.unwrap_or((previous.unwrap_or_else(SystemTime::now), bytes));
        Self {
            time,
            stream,
            text: String::from_utf8_lossy(&text[..text.len().min(LINE_BYTES)]).into_owned(),
        }
    }
}

impl From<Line> for wire::ContainerLogLine {
    fn from(value: Line) -> Self {
        Self {
            time: Some(value.time.into()),
            stream: value.stream.into(),
            text: value.text,
        }
    }
}

/// A time as the engine takes it: whole seconds since the epoch.
fn seconds(time: SystemTime) -> i32 {
    time.duration_since(UNIX_EPOCH).map_or(0, |since| {
        i32::try_from(since.as_secs()).unwrap_or(i32::MAX)
    })
}

/// What to ask the engine for: the last `tail` lines from `since` on.
fn options(tail: u32, since: Option<SystemTime>, follow: bool) -> LogsOptionsBuilder {
    let options = LogsOptionsBuilder::default()
        .follow(follow)
        .stdout(true)
        .stderr(true)
        .timestamps(true)
        .tail(&tail.to_string());
    match since {
        Some(since) => options.since(seconds(since)),
        None => options,
    }
}

/// The last `lines` lines at or after `since`, oldest first, and whether
/// older ones were left out to stay within `READ_BYTES`.
pub(crate) async fn read(
    client: &Docker,
    container: &str,
    lines: u32,
    since: Option<SystemTime>,
) -> Result<(Vec<wire::ContainerLogLine>, bool), PanelError> {
    let lines = match lines {
        0 => DEFAULT_LINES,
        lines => lines.min(MOST_LINES),
    };
    let mut outputs = client.logs(container, Some(options(lines, since, false).build()));
    let mut kept = VecDeque::new();
    let (mut bytes, mut truncated, mut previous) = (0, false, None);
    while let Some(output) = outputs.next().await {
        let line = Line::read(output.map_err(|error| failure(&error))?, previous);
        previous = Some(line.time);
        if since.is_some_and(|since| line.time < since) {
            continue;
        }
        bytes += line.text.len();
        kept.push_back(line);
        while bytes > READ_BYTES {
            let Some(dropped) = kept.pop_front() else {
                break;
            };
            bytes -= dropped.text.len();
            truncated = true;
        }
    }
    Ok((kept.into_iter().map(Into::into).collect(), truncated))
}

/// Lines as the container prints them, up to `BATCH` at a time: first the
/// last `lines` it printed before, or the lines after `after`. A response
/// with an error, and no lines, is the last; the stream ends when the
/// container stops.
pub(crate) fn follow(
    client: &Docker,
    container: &str,
    lines: u32,
    after: Option<SystemTime>,
) -> impl Stream<Item = wire::ContainersFollowLogsResponse> + Send + 'static {
    let tail = match after {
        Some(_) => RESUME_LINES,
        None => lines.min(MOST_BACKLOG),
    };
    let mut previous = None;
    client
        .logs(container, Some(options(tail, after, true).build()))
        .map(move |output| -> Result<Line, PanelError> {
            let line = Line::read(output.map_err(|error| failure(&error))?, previous);
            previous = Some(line.time);
            Ok(line)
        })
        .filter(move |line| {
            ready(!matches!((line, after), (Ok(line), Some(after)) if line.time <= after))
        })
        .ready_chunks(BATCH)
        .flat_map(|lines| {
            let mut sent = wire::ContainersFollowLogsResponse::default();
            let mut failed = None;
            for line in lines {
                match line {
                    Ok(line) => sent.lines.push(line.into()),
                    Err(error) => {
                        failed = Some(wire::ContainersFollowLogsResponse {
                            lines: Vec::new(),
                            error: Some((&error).into()),
                        });
                        break;
                    }
                }
            }
            let sent = (!sent.lines.is_empty()).then_some(sent);
            futures_util::stream::iter(sent.into_iter().chain(failed))
        })
        .scan(false, |ended, response| {
            if *ended {
                return ready(None);
            }
            *ended = response.error.is_some();
            ready(Some(response))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output(stream: ContainerLogStream, text: &[u8]) -> LogOutput {
        let message = text.to_vec().into();
        match stream {
            ContainerLogStream::Stderr => LogOutput::StdErr { message },
            _ => LogOutput::StdOut { message },
        }
    }

    fn at(time: &str) -> SystemTime {
        chrono::DateTime::parse_from_rfc3339(time).unwrap().into()
    }

    #[test]
    fn lines_carry_the_time_the_engine_recorded() {
        let line = Line::read(
            output(
                ContainerLogStream::Stdout,
                b"2027-01-15T08:00:00.123456789Z GET / 200\r\n",
            ),
            None,
        );
        assert_eq!(line.text, "GET / 200");
        assert_eq!(line.stream, ContainerLogStream::Stdout);
        assert_eq!(line.time, at("2027-01-15T08:00:00.123456789Z"));

        let error = Line::read(
            output(ContainerLogStream::Stderr, b"2027-01-15T08:00:01Z panic\n"),
            None,
        );
        assert_eq!(
            (error.stream, error.text.as_str()),
            (ContainerLogStream::Stderr, "panic")
        );
        let terminal = Line::read(
            LogOutput::Console {
                message: b"2027-01-15T08:00:02Z $ ls\n".to_vec().into(),
            },
            None,
        );
        assert_eq!(terminal.stream, ContainerLogStream::Stdout);
        assert_eq!(
            Line::read(
                output(ContainerLogStream::Stdout, b"2027-01-15T08:00:03Z \n"),
                None
            )
            .text,
            ""
        );
    }

    #[test]
    fn a_line_without_a_time_takes_the_one_before() {
        let before = at("2027-01-15T08:00:00Z");
        let line = Line::read(
            output(ContainerLogStream::Stdout, b"rest of it\n"),
            Some(before),
        );
        assert_eq!((line.time, line.text.as_str()), (before, "rest of it"));
    }

    #[test]
    fn long_lines_are_cut_and_invalid_text_replaced() {
        let mut long = b"2027-01-15T08:00:00Z ".to_vec();
        long.extend(std::iter::repeat_n(b'x', LINE_BYTES + 10));
        assert_eq!(
            Line::read(output(ContainerLogStream::Stdout, &long), None)
                .text
                .len(),
            LINE_BYTES
        );
        let invalid = Line::read(
            output(ContainerLogStream::Stdout, b"2027-01-15T08:00:00Z a\xffb"),
            None,
        );
        assert_eq!(invalid.text, "a\u{fffd}b");
    }

    #[test]
    fn the_engine_takes_whole_seconds() {
        assert_eq!(seconds(at("2027-01-15T08:00:00.9Z")), 1_800_000_000);
        assert_eq!(seconds(UNIX_EPOCH - Duration::from_secs(1)), 0);
    }
}
