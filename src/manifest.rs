// Copyright © 2026 James 'akses' Burger
//
// This program is free software: you can redistribute it and/or modify it under the terms of the
// GNU General Public License as published by the Free Software Foundation, either version 3 of
// the License, or (at your option) any later version.
//
// This program is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY;
// without even the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.
//
// See the GNU General Public License for more details. You should have received a copy of
// the GNU General Public License along with this program. If not, see <https://www.gnu.org/licenses/>.
// --------------------------------------------------------- //
// Cellar - Cross-platform GUI for ISO 9660 image creation.  //
// Joliet support for long filenames.                        //
// --------------------------------------------------------- //
// manifest.rs - Structs and functions for working with ISO manifest files.

use chrono::Local;
use serde::Serialize;
use std::fmt::Write as _;

/// Optional metadata captured per build. Only written into the ISO when
/// research mode is enabled.
#[derive(Default, Clone, Serialize)]
pub struct ManifestFields {
    pub source: String,
    pub package_name: String,
    pub package_version: String,
    pub severity: String,
    pub references: String, // freeform, one URL per line
    pub notes: String,
}

/// Platform-specific metadata for a single file.
#[derive(Default, Clone, Serialize)]
pub struct FileMetadata {
    pub mtime: Option<String>, // RFC 3339
    pub atime: Option<String>,
    pub ctime: Option<String>,
    pub permissions: Option<u32>,
    pub uid: Option<u32>,
    pub gid: Option<u32>,
}

#[derive(Serialize)]
struct ManifestFile<'a> {
    name: String,
    sha256: &'a str,
    size_bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none", flatten)]
    metadata: Option<&'a FileMetadata>,
}

#[derive(Serialize)]
pub struct Manifest<'a> {
    created: String,
    tool: &'a str,
    label: &'a str,
    fields: &'a ManifestFields,
    files: Vec<ManifestFile<'a>>,
}

impl<'a> Manifest<'a> {
    pub fn build(
        label: &'a str,
        fields: &'a ManifestFields,
        files: impl IntoIterator<Item = (&'a str, &'a str, u64, &'a FileMetadata)>,
    ) -> Self {
        let files = files
            .into_iter()
            .map(|(name, hash, size, meta)| ManifestFile {
                name: name.to_string(),
                sha256: hash,
                size_bytes: size,
                metadata: Some(meta),
            })
            .collect();

        Self {
            created: Local::now().to_rfc3339(),
            tool: concat!("cellar ", env!("CARGO_PKG_VERSION")),
            label,
            fields,
            files,
        }
    }

    pub fn to_text(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "cellar ISO manifest");
        let _ = writeln!(out, "===================");
        let _ = writeln!(out, "Created: {}", self.created);
        let _ = writeln!(out, "Tool:    {}", self.tool);
        let _ = writeln!(out, "Label:   {}", self.label);
        let _ = writeln!(out);

        let f = self.fields;
        if !f.package_name.is_empty() || !f.package_version.is_empty() {
            let _ = writeln!(out, "Package: {} {}", f.package_name, f.package_version);
        }
        if !f.source.is_empty() {
            let _ = writeln!(out, "Source:  {}", f.source);
        }
        if !f.severity.is_empty() {
            let _ = writeln!(out, "Severity: {}", f.severity);
        }
        if !f.references.trim().is_empty() {
            let _ = writeln!(out, "\nReferences:");
            for line in f.references.lines() {
                let line = line.trim();
                if !line.is_empty() {
                    let _ = writeln!(out, "  - {line}");
                }
            }
        }
        if !f.notes.trim().is_empty() {
            let _ = writeln!(out, "\nNotes:");
            for line in f.notes.lines() {
                let _ = writeln!(out, "  {line}");
            }
        }

        let _ = writeln!(out, "\nFiles:");
        let name_width = self
            .files
            .iter()
            .map(|f| f.name.len())
            .max()
            .unwrap_or(0)
            .max(10);
        for f in &self.files {
            let _ = writeln!(
                out,
                "  {name:<width$}  {size:>12} bytes  SHA-256: {hash}",
                name = f.name,
                width = name_width,
                size = f.size_bytes,
                hash = f.sha256,
            );
        }
        out
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }
}
