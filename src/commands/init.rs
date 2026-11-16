//! `dflog init`: create a new case workspace.

use crate::cli::InitArgs;
use crate::config::{CaseConfig, Config, HashConfig};
use crate::error::Result;
use crate::workspace::Workspace;

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
    let ws = Workspace::create(&init_args.base_path, config)?;

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
