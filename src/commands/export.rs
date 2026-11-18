use crate::cli::ExportArgs;
use crate::error::{Error, Result};
use crate::journal::Journal;
use crate::report_renderer::{Report, ReportFormat};
use crate::workspace::Workspace;
use std::fs::OpenOptions;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::process::ExitCode;

pub fn run(args: ExportArgs) -> Result<ExitCode> {
    let cwd = std::env::current_dir()?;
    let ws = Workspace::discover(&cwd)?;
    let journal = Journal::for_workspace(&ws)?;

    let snapshot = journal.snapshot()?;
    let now = jiff::Timestamp::now();

    let (out_path, out_format) = match args.output {
        Some(output) => {
            let format = match args.format {
                Some(f) => f,
                None => ReportFormat::from_extension(&output)?,
            };
            (output, format)
        }
        None => {
            let format = args.format.unwrap_or_default();
            let name = format!(
                "{}.{}",
                now.strftime(&ws.config.report.name_format),
                format.extension()
            );
            if Path::new(&name).file_name() != Some(name.as_ref()) {
                return Err(Error::InvalidReportName(name));
            }

            (ws.reports_dir().join(name), format)
        }
    };

    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(out_path)?;
    let mut buf_writer = BufWriter::new(file);

    let report = Report {
        case_id: ws.config.case.case_id.clone(),
        examiner: ws.config.case.examiner.clone(),
        generated_at: now,
        dflog_version: env!("CARGO_PKG_VERSION"),
        verification: snapshot.verification,
        journal_hash: snapshot.file_hash,
        entries: snapshot.entries,
    };
    out_format.renderer().render(&report, &mut buf_writer)?;
    buf_writer.flush()?;

    Ok(ExitCode::SUCCESS)
}
