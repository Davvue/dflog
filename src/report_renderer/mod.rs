pub mod html;
pub mod markdown;
#[cfg(test)]
mod test_fixtures;
pub mod text;

use crate::error::{Error, Result};
use crate::journal::{Entry, Verification};
use crate::report_renderer::html::HtmlRenderer;
use crate::report_renderer::markdown::MarkdownRenderer;
use crate::report_renderer::text::TextRenderer;
use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum ReportFormat {
    #[default]
    Text,
    Html,
    Markdown,
}

impl ReportFormat {
    pub fn renderer(&self) -> &dyn Renderer {
        match self {
            ReportFormat::Text => &TextRenderer,
            ReportFormat::Html => &HtmlRenderer,
            ReportFormat::Markdown => &MarkdownRenderer,
        }
    }

    pub fn extension(&self) -> &'static str {
        match self {
            ReportFormat::Text => "txt",
            ReportFormat::Html => "html",
            ReportFormat::Markdown => "md",
        }
    }

    pub fn from_extension(path: &Path) -> Result<Self> {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default();
        Self::value_variants()
            .iter()
            .copied()
            .find(|f| f.extension().eq_ignore_ascii_case(ext))
            .ok_or_else(|| Error::UnsupportedExportFormat(path.display().to_string()))
    }
}

pub struct Report {
    pub case_id: String,
    pub examiner: String,
    pub generated_at: jiff::Timestamp,
    pub dflog_version: &'static str,
    pub verification: Verification,
    pub journal_hash: String,
    pub entries: Vec<Result<Entry>>,
}

pub trait Renderer {
    fn render(&self, report: &Report, out: &mut dyn Write) -> Result<()>;
}
