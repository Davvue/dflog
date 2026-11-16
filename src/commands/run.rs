//! `dflog run`: execute a command and record it in the journal

use crate::cli::RunArgs;
use crate::error::Result;
use crate::hash::HashAlgorithm;
use crate::index::Index;
use crate::journal::{
    CommandContext, CommandOutput, Digest, Entry, EntryKind, ExitStatus, Journal, ProgramInfo,
};
use crate::workspace::Workspace;
use std::fs::File;
use std::io::{ErrorKind, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

pub fn run(run_args: RunArgs) -> Result<ExitCode> {
    let cwd = std::env::current_dir()?.canonicalize()?;
    let ws = Workspace::discover(&cwd)?;
    let journal = Journal::for_workspace(&ws)?;
    let alg = ws.config.hash.algorithm;
    let scope = Path::new(Workspace::working_scope());
    let rel_cwd = cwd.strip_prefix(&ws.root).map(Path::to_path_buf).unwrap_or(cwd.clone());
    let rel_cwd = if rel_cwd.as_os_str().is_empty() { PathBuf::from(".") } else { rel_cwd };

    // 1. Check for changes since the last entry
    let saved_index = Index::load(&ws.index_path())?.unwrap_or_default();
    let before = saved_index.snapshot(&ws.root, scope, alg)?;
    let external_changes = saved_index.diff(&before);

    // 2. Create and open output files
    let started_at = jiff::Timestamp::now();
    let file_stem = format!(
        "{}-{}",
        started_at.strftime("%Y%m%dT%H%M%S%.3fZ"),
        std::process::id()
    );
    let stdout_path = ws.cmd_out_dir().join(format!("{file_stem}.stdout"));
    let stderr_path = ws.cmd_out_dir().join(format!("{file_stem}.stderr"));
    let stdout_file = File::create_new(&stdout_path)?;
    let stderr_file = File::create_new(&stderr_path)?;

    let (program, status) = match resolve_program(&run_args.command[0]) {
        Ok(path) => {
            let digest = digest_file(alg, &path);
            let status = execute(&path, &run_args.command, stdout_file, stderr_file)?;
            (Some(ProgramInfo { path, digest }), status)
        }
        Err(e) => (None, ExitStatus::SpawnFailed(e.to_string())),
    };
    let exit_code = exit_code(&status);

    // 4. What did the command change
    let after = before.snapshot(&ws.root, scope, alg)?;
    let changes = before.diff(&after);
    // TODO: --artifacts paths

    // 5. Log, then save index
    let command = CommandContext {
        argv: run_args.command,
        program,
        cwd: rel_cwd,
        status,
        stdout: output_info(&ws, &stdout_path, alg)?,
        stderr: output_info(&ws, &stderr_path, alg)?,
    };
    let mut entry = Entry::new(
        EntryKind::Run { command },
        &ws.config,
        run_args.message,
        started_at,
    );
    entry.changes = changes;
    entry.external_changes = external_changes;
    journal.append(entry)?;
    after.save(&ws.index_path())?;

    Ok(exit_code)
}

fn resolve_program(name: &str) -> std::io::Result<PathBuf> {
    let candidate = Path::new(name);
    if name.contains('/') {
        return candidate.canonicalize();
    }

    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .map(|dir| {
            if dir.as_os_str().is_empty() {
                PathBuf::from(".")
            } else {
                dir
            }
        })
        .map(|dir| dir.join(name))
        .find(|p| {
            p.metadata()
                .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
                .unwrap_or(false)
        })
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("{name}: command not found"),
            )
        })?
        .canonicalize()
}

fn execute(
    program: &Path,
    argv: &[String],
    stdout_file: File,
    stderr_file: File,
) -> std::io::Result<ExitStatus> {
    let spawned = Command::new(program)
        .arg0(&argv[0])
        .args(&argv[1..])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();

    let mut child = match spawned {
        Ok(child) => child,
        Err(e) => return Ok(ExitStatus::SpawnFailed(e.to_string())),
    };

    let child_out = child.stdout.take().expect("stdout is piped");
    let child_err = child.stderr.take().expect("stderr is piped");

    let (waited, out_res, err_res) = std::thread::scope(|s| {
        let out = s.spawn(move || tee(child_out, std::io::stdout(), stdout_file));
        let err = s.spawn(move || tee(child_err, std::io::stderr(), stderr_file));
        let waited = child.wait();
        (
            waited,
            out.join().expect("tee panicked"),
            err.join().expect("tee panicked"),
        )
    });

    for (name, res) in [("stdout", out_res), ("stderr", err_res)] {
        if let Err(e) = res {
            eprintln!(
                "{} warning: capturing {name} failed: {e}",
                env!("CARGO_PKG_NAME")
            );
        }
    }

    let status = waited?;
    Ok(match (status.code(), status.signal()) {
        (Some(code), _) => ExitStatus::Exited(code),
        (None, Some(sig)) => ExitStatus::Signal(sig),
        (None, None) => unreachable!("unix exit status is either a code or a signal"),
    })
}

fn tee(mut from: impl Read, mut terminal: impl Write, mut file: File) -> std::io::Result<()> {
    let mut buf = [0u8; 8192];
    let mut file_result = Ok(());

    loop {
        let n = match from.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        if file_result.is_ok() {
            file_result = file.write_all(&buf[..n]);
        }
        let _ = terminal
            .write_all(&buf[..n])
            .and_then(|()| terminal.flush());
    }

    file_result.and_then(|()| file.sync_all())
}

fn digest_file(alg: HashAlgorithm, path: &Path) -> Digest {
    match File::open(path).and_then(|f| alg.prefixed_digest(f)) {
        Ok(hash) => Digest::Hash(hash),
        Err(e) => Digest::Error(e.to_string()),
    }
}

fn output_info(ws: &Workspace, path: &Path, alg: HashAlgorithm) -> Result<CommandOutput> {
    Ok(CommandOutput {
        path: path
            .strip_prefix(ws.dflog_dir())
            .expect("cmd-out is inside .dflog")
            .to_path_buf(),
        hash: alg.prefixed_digest(File::open(path)?)?,
        size: std::fs::metadata(path)?.len(),
    })
}

fn exit_code(status: &ExitStatus) -> ExitCode {
    match status {
        ExitStatus::Exited(code) => ExitCode::from(*code as u8),
        ExitStatus::Signal(sig) => ExitCode::from((128 + sig) as u8),
        ExitStatus::SpawnFailed(_) => ExitCode::from(127),
    }
}
