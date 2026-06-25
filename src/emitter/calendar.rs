// This is free and unencumbered software released into the public domain.

#[cfg(not(feature = "std"))]
compile_error!("asimov-apple-calendar-emitter requires the 'std' feature");

use asimov_module::SysexitsError::{self, *};
use clap::Parser;
use clientele::StandardOptions;
use serde_json::json;
use std::{
    error::Error as StdError,
    fmt, io,
    process::{Command, ExitStatus},
};

type CoreResult<T> = Result<T, CalendarError>;

#[derive(Debug)]
enum CalendarError {
    Io {
        context: &'static str,
        source: io::Error,
    },
    OsaScriptFailed {
        status: ExitStatus,
        stderr: String,
    },
    CalendarParse {
        context: &'static str,
        message: String,
    },
    Json {
        context: &'static str,
        source: serde_json::Error,
    },
    Jq {
        context: &'static str,
        source: jq::JsonFilterError,
    },
}

impl fmt::Display for CalendarError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CalendarError::Io { context, .. } => {
                write!(f, "I/O error while {context}")
            }
            CalendarError::OsaScriptFailed { .. } => {
                write!(f, "failed to talk to Apple Calendar (osascript)")
            }
            CalendarError::CalendarParse { context, message } => {
                write!(
                    f,
                    "failed to parse Apple Calendar output while {context}: {message}"
                )
            }
            CalendarError::Json { context, .. } => {
                write!(f, "failed to serialize JSON while {context}")
            }
            CalendarError::Jq { context, .. } => {
                write!(f, "failed to filter JSON while {context}")
            }
        }
    }
}

impl StdError for CalendarError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            CalendarError::Io { source, .. } => Some(source),
            CalendarError::Json { source, .. } => Some(source),
            CalendarError::Jq { source, .. } => Some(source),
            _ => None,
        }
    }
}

impl From<io::Error> for CalendarError {
    fn from(source: io::Error) -> Self {
        CalendarError::Io {
            context: "performing I/O",
            source,
        }
    }
}

impl From<serde_json::Error> for CalendarError {
    fn from(e: serde_json::Error) -> Self {
        CalendarError::Json {
            context: "writing JSON to stdout",
            source: e,
        }
    }
}

fn handle_error(err: &CalendarError, _flags: &StandardOptions) -> SysexitsError {
    eprintln!("Error: {err}");

    #[cfg(feature = "tracing")]
    match err {
        CalendarError::Io { context, source } => {
            asimov_module::tracing::debug!(
                target: "asimov_apple_module::calendar_emitter",
                %context,
                error = %source,
                "I/O error details"
            );
        }
        CalendarError::OsaScriptFailed { status, stderr } => {
            asimov_module::tracing::debug!(
                target: "asimov_apple_module::calendar_emitter",
                ?status,
                stderr = %stderr,
                "osascript failure details"
            );
        }
        CalendarError::CalendarParse { context, message } => {
            asimov_module::tracing::debug!(
                target: "asimov_apple_module::calendar_emitter",
                %context,
                %message,
                "parse failure details"
            );
        }
        CalendarError::Json { context, source } => {
            asimov_module::tracing::debug!(
                target: "asimov_apple_module::calendar_emitter",
                %context,
                error = %source,
                "JSON serialization failure details"
            );
        }
        CalendarError::Jq { context, source } => {
            asimov_module::tracing::debug!(
                target: "asimov_apple_module::calendar_emitter",
                %context,
                error = %source,
                "jq filter failure details"
            );
        }
    }

    match err {
        CalendarError::Io { .. } => EX_IOERR,
        CalendarError::OsaScriptFailed { .. } => EX_UNAVAILABLE,
        CalendarError::CalendarParse { .. } => EX_DATAERR,
        CalendarError::Json { .. } => EX_DATAERR,
        CalendarError::Jq { .. } => EX_DATAERR,
    }
}

/// asimov-apple-calendar-emitter
#[derive(Debug, Parser)]
struct Options {
    #[clap(flatten)]
    flags: StandardOptions,
}

pub fn main() -> Result<SysexitsError, Box<dyn StdError>> {
    // Load environment variables from `.env`:
    asimov_module::dotenv().ok();

    // Expand wildcards and @argfiles:
    let args = asimov_module::args_os()?;

    // Parse command-line options:
    let options = Options::parse_from(args);

    // Handle the `--version` flag:
    if options.flags.version {
        println!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
        return Ok(EX_OK);
    }

    // Handle the `--license` flag:
    if options.flags.license {
        print!("{}", include_str!("../../UNLICENSE"));
        return Ok(EX_OK);
    }

    // Configure logging & tracing:
    #[cfg(feature = "tracing")]
    asimov_module::init_tracing_subscriber(&options.flags).expect("failed to initialize logging");

    let exit_code = match run_emitter(&options) {
        Ok(()) => EX_OK,
        Err(err) => handle_error(&err, &options.flags),
    };

    Ok(exit_code)
}

fn run_emitter(_opts: &Options) -> CoreResult<()> {
    use std::io::{self, BufWriter, Write};

    const APPLESCRIPT: &str = r#"
        set output to ""
        tell application "Calendar"
            set theCalendars to every calendar
            repeat with cal in theCalendars
                set calName to the name of cal
                set eventsList to every event of cal
                repeat with e in eventsList
                    set eventId to the uid of e
                    set eventTitle to the summary of e
                    set eventStart to the start date of e
                    set eventEnd to the end date of e
                    set eventLoc to the location of e
                    set eventDesc to the description of e
                    set output to output & eventId & "|||"
                    set output to output & eventTitle & "|||"
                    set output to output & (eventStart as string) & "|||"
                    set output to output & (eventEnd as string) & "|||"
                    if eventLoc is missing value then
                        set output to output & "" & "|||"
                    else
                        set output to output & eventLoc & "|||"
                    end if
                    if eventDesc is missing value then
                        set output to output & "" & "|||"
                    else
                        set output to output & eventDesc & "|||"
                    end if
                    set output to output & calName & "~~~"
                end repeat
            end repeat
        end tell
        return output
    "#;

    #[cfg(feature = "tracing")]
    asimov_module::tracing::info!(
        target: "asimov_apple_module::calendar_emitter",
        "starting apple calendar emitter"
    );

    let output = Command::new("osascript")
        .arg("-e")
        .arg(APPLESCRIPT)
        .output()
        .map_err(|e| CalendarError::Io {
            context: "invoking osascript",
            source: e,
        })?;

    #[cfg(feature = "tracing")]
    asimov_module::tracing::debug!(
        target: "asimov_apple_module::calendar_emitter",
        status = ?output.status,
        stdout_len = output.stdout.len(),
        stderr_len = output.stderr.len(),
        "osascript completed"
    );

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        return Err(CalendarError::OsaScriptFailed {
            status: output.status,
            stderr,
        });
    }

    let stdout = String::from_utf8_lossy(&output.stdout);

    if stdout.trim().is_empty() {
        #[cfg(feature = "tracing")]
        asimov_module::tracing::info!(
            target: "asimov_apple_module::calendar_emitter",
            "no events returned from Apple Calendar"
        );
        return Ok(());
    }

    let locked = io::stdout().lock();
    let mut writer = BufWriter::new(locked);

    let mut count = 0usize;

    for chunk in stdout.split("~~~").filter(|c| !c.trim().is_empty()) {
        let mut parts = chunk.split("|||");

        let id = parts
            .next()
            .ok_or_else(|| CalendarError::CalendarParse {
                context: "reading event id",
                message: "missing id field".to_string(),
            })?
            .trim();

        let name = parts
            .next()
            .ok_or_else(|| CalendarError::CalendarParse {
                context: "reading event title",
                message: "missing title field".to_string(),
            })?
            .trim()
            .to_string();

        let start_date = parts
            .next()
            .ok_or_else(|| CalendarError::CalendarParse {
                context: "reading start date",
                message: "missing start date field".to_string(),
            })?
            .trim()
            .to_string();

        let end_date = parts
            .next()
            .ok_or_else(|| CalendarError::CalendarParse {
                context: "reading end date",
                message: "missing end date field".to_string(),
            })?
            .trim()
            .to_string();

        let location = parts
            .next()
            .ok_or_else(|| CalendarError::CalendarParse {
                context: "reading location",
                message: "missing location field".to_string(),
            })?
            .trim()
            .to_string();

        let description = parts
            .next()
            .ok_or_else(|| CalendarError::CalendarParse {
                context: "reading description",
                message: "missing description field".to_string(),
            })?
            .trim()
            .to_string();

        let calendar = parts
            .next()
            .ok_or_else(|| CalendarError::CalendarParse {
                context: "reading calendar name",
                message: "missing calendar field".to_string(),
            })?
            .trim()
            .to_string();

        if parts.next().is_some() {
            return Err(CalendarError::CalendarParse {
                context: "reading calendar event fields",
                message: "unexpected extra field delimiter in event data".to_string(),
            });
        }

        #[cfg(feature = "tracing")]
        asimov_module::tracing::debug!(
            target: "asimov_apple_module::calendar_emitter",
            event_id = %id,
            calendar = %calendar,
            name = %name,
            "emitting event"
        );

        let mut node = json!({
            "@type": "Event",
            "@id": format!("urn:apple:calendar:event:{id}"),
            "name": name,
            "startDate": start_date,
            "endDate": end_date,
            "isPartOf": calendar,
            "source": "apple-calendar",
        });

        if !location.is_empty() {
            node["location"] = json!(location);
        }

        if !description.is_empty() {
            node["description"] = json!(description);
        }

        let node = asimov_apple_module::calendar()
            .filter_json(node)
            .map_err(|e| CalendarError::Jq {
                context: "filtering calendar event JSON",
                source: e,
            })?;

        serde_json::to_writer(&mut writer, &node)?;
        writer.write_all(b"\n").map_err(|e| CalendarError::Io {
            context: "writing newline to stdout",
            source: e,
        })?;

        count += 1;
    }

    writer.flush().map_err(|e| CalendarError::Io {
        context: "flushing stdout",
        source: e,
    })?;

    #[cfg(feature = "tracing")]
    asimov_module::tracing::info!(
        target: "asimov_apple_module::calendar_emitter",
        events = count,
        "finished apple calendar emitter"
    );

    Ok(())
}
