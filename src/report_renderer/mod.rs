use crate::error::Result;
use crate::journal::{Entry, Verification};
use std::io::Write;

pub struct Report {
    pub case_if: String,
    pub examiner: String,
    pub generated_at: jiff::Timestamp,
    pub dflog_version: &'static str,
    pub verification: Verification,
    pub journal_hash: String,
    pub entries: Vec<Entry>,
}

pub trait Renderer {
    fn render(&self, report: &Report, out: &mut dyn Write) -> Result<()>;
}
