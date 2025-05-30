//! This module prints backtraces in case of panics
//! Adapted from https://github.com/rust-cli/human-panic/blob/0ebcb91b29e3f23b3559ea49d931a493ed7c8139
//! under MIT license

use std::env;
use std::error::Error;
use std::fmt::Write as FmtWrite;
use std::fs::File;
use std::io::{self, Write};
use std::mem;
use std::panic::PanicHookInfo;
use std::path::{Path, PathBuf};

use backtrace::Backtrace;
use backtrace_ext::short_frames_strict;
use termcolor::{Color, ColorChoice, ColorSpec, StandardStream, WriteColor};

pub fn setup_panic_handler() {
    if cfg!(debug_assertions) {
        // we want the normal panic handler for debugging
        return;
    }

    std::panic::set_hook(Box::new(move |info: &PanicHookInfo| {
        let file_path = handle_dump(info);
        print_msg(file_path).expect("printing error message to console failed");
    }));
}

// Utility function which will handle dumping information to disk
fn handle_dump(panic_info: &PanicHookInfo) -> Option<PathBuf> {
    let mut expl = String::new();

    #[cfg(feature = "nightly")]
    let message = panic_info.payload_as_str().map(|m| m.to_owned());

    #[cfg(not(feature = "nightly"))]
    let message = match (
        panic_info.payload().downcast_ref::<&str>(),
        panic_info.payload().downcast_ref::<String>(),
    ) {
        (Some(&s), _) => Some(s.to_string()),
        (_, Some(s)) => Some(s.to_string()),
        (None, None) => None,
    };

    let cause = message.unwrap_or_else(|| "Unknown".into());

    match panic_info.location() {
        Some(location) => {
            let file = location.file();
            let line = location.line();
            let _ = writeln!(expl, "Panic occurred in file '{file}' at line {line}",);
        }
        None => expl.push_str("Panic location unknown.\n"),
    }

    let report = Report::new(expl, cause);

    match report.persist() {
        Ok(fp) => Some(fp),
        Err(_) => {
            eprintln!("{}", report.0);
            None
        }
    }
}

fn print_msg<P: AsRef<Path>>(file_path: Option<P>) -> io::Result<()> {
    let mut stderr = StandardStream::stderr(ColorChoice::Auto);
    stderr.set_color(ColorSpec::new().set_fg(Some(Color::Red)).set_bold(true))?;
    writeln!(stderr, "OpenVAF encountered a problem and has crashed!")?;
    stderr.set_color(ColorSpec::new().set_reset(true))?;
    writeln!(
        stderr,
        "\nA log file has been generated at \"{}\".
To help us fix the problem, please open an issue at https://github.com/pascalkuthe/OpenVAF/
or send an email to pascal.kuthe@semimod.de and attach the log file.
If possible please also attach the source file that OpenVAF was compiling.",
        match file_path {
            Some(fp) => format!("{}", fp.as_ref().display()),
            None => "<Failed to store file to disk>".to_string(),
        },
    )?;
    Ok(())
}

/// Contains metadata about the crash like the backtrace and
/// information about the crate and operating system. Can
/// be used to be serialized and persisted or printed as
/// information to the user.
#[derive(Debug)]
struct Report(String);

impl Report {
    /// Create a new instance.
    fn new(explanation: String, cause: String) -> Self {
        let mut dst = String::new();
        let _ = writeln!(dst, "OpenVAF {}", env!("CARGO_PKG_VERSION"));
        if let Ok(args) = super::ARGS.lock() {
            if let Some(args) = &*args {
                let _ = writeln!(dst, "{:#?}", args);
            }
        }
        let _ = write!(dst, "{explanation}{cause}");
        // We take padding for address and extra two letters to pad after index.
        const HEX_WIDTH: usize = mem::size_of::<usize>() + 2;
        // Padding for next lines after frame's address
        const NEXT_SYMBOL_PADDING: usize = HEX_WIDTH + 6;

        for (frame, idx) in short_frames_strict(&Backtrace::new()) {
            let ip = frame.ip();
            if idx.start + 1 < idx.end {
                let _ = write!(dst, "\n{:4}..{:4}: {ip:HEX_WIDTH$?}", idx.start, idx.end);
            } else {
                let _ = write!(dst, "\n{:4}: {ip:HEX_WIDTH$?}", idx.start);
            }

            let symbols = frame.symbols();
            if symbols.is_empty() {
                let _ = write!(dst, " - <unresolved>");
                continue;
            }

            for (idx, symbol) in symbols.iter().enumerate() {
                //Print symbols from this address,
                //if there are several addresses
                //we need to put it on next line
                if idx != 0 {
                    let _ = write!(dst, "\n{:1$}", "", NEXT_SYMBOL_PADDING);
                }

                if let Some(name) = symbol.name() {
                    let _ = write!(dst, " - {name}");
                } else {
                    let _ = write!(dst, " - <unknown>");
                }

                //See if there is debug information with file name and line
                if let (Some(file), Some(line)) = (symbol.filename(), symbol.lineno()) {
                    let _ = write!(
                        dst,
                        "\n{:3$}at {}:{}",
                        "",
                        file.display(),
                        line,
                        NEXT_SYMBOL_PADDING
                    );
                }
            }
        }

        Self(dst)
    }

    /// Write a file to disk.
    fn persist(&self) -> Result<PathBuf, Box<dyn Error + 'static>> {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let tmp_dir = env::temp_dir();
        let file_name = format!("openvaf-crash-{timestamp}.log");
        let file_path = Path::new(&tmp_dir).join(file_name);
        let mut file = File::create(&file_path)?;
        file.write_all(self.0.as_bytes())?;
        // TODO: include log
        Ok(file_path)
    }
}
