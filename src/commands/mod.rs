//! Subcommand implementations. Each module exposes a `run` function that
//! takes the subcommand's parsed arguments from [`crate::cli`].

pub mod init;
pub mod journal;
pub mod record;
pub mod run;
pub mod verify;
