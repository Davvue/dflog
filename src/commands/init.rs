//! `dflog init`: create a new case workspace.

use crate::cli::InitArgs;
use crate::config::{CaseConfig, Config, HashConfig};
use crate::error::Result;
use crate::index::Index;
use crate::journal::{Entry, EntryKind, Journal};
use crate::workspace::Workspace;
use std::path::Path;

/// Builds the case config from the arguments, creates the workspace at
/// `base_path` and tells the examiner where to put case files.
///
/// Fails with [`crate::error::Error::AlreadyInitialized`] if `base_path`
/// already contains a workspace.
pub fn run(init_args: InitArgs) -> Result<()> {
    let config = Config {
        case: CaseConfig {
            examiner: init_args.examiner,
            case_id: init_args.case_id,
        },
        hash: HashConfig {
            algorithm: init_args.hash,
        },
    };

    let started_at = jiff::Timestamp::now();
    let ws = Workspace::create(&init_args.base_path, config)?;
    let index = Index::default().snapshot(
        &ws.root,
        Path::new(Workspace::working_scope()),
        ws.config.hash.algorithm,
    )?;

    let mut entry = Entry::new(
        EntryKind::Init {
            config: ws.config.clone(),
        },
        &ws.config,
        init_args
            .message
            .unwrap_or(format!("Initialize {} workspace", env!("CARGO_PKG_NAME"))),
        started_at,
    );
    entry.changes = Index::default().diff(&index);
    Journal::create(&ws, entry)?;

    index.save(&ws.index_path())?;

    println!(
        "Initializing successful. Place case files in '{}'",
        ws.working_dir().display()
    );
    println!(
        "\tUse '{} run -- <cmd>' to automatically create hashes and log entries",
        env!("CARGO_PKG_NAME")
    );

    Ok(())
}
