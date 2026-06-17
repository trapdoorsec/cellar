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
// backend.rs - Backend functions for the Cellar application.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;

use crate::iso::{self, IsoDateTime, IsoFile, IsoFileSource, JolietLabelMode};
use crate::manifest::{FileMetadata, Manifest, ManifestFields};

pub struct BuildRequest {
    pub files: Vec<StagedFile>,
    pub output: PathBuf,
    pub label: String,
    pub joliet_label_mode: JolietLabelMode,
    pub manifest: Option<(ManifestFields, String)>, // (fields, label-for-manifest)
}

#[derive(Clone)]
pub struct StagedFile {
    pub path: PathBuf,
    pub sha256: String,
    pub size: u64,
    pub metadata: FileMetadata,
}

#[derive(Debug, Clone)]
pub enum BuildEvent {
    Progress(String),
    Done(PathBuf),
    Failed(String),
}

/// Kick off the build on a background thread. Events stream back over the channel
/// and the channel closes when the build terminates.
pub fn build_async(req: BuildRequest) -> mpsc::Receiver<BuildEvent> {
    let (tx, rx) = mpsc::channel();

    thread::spawn(move || {
        let send = |e: BuildEvent| {
            let _ = tx.send(e);
        };

        match run_build(&req, &send) {
            Ok(path) => send(BuildEvent::Done(path)),
            Err(e) => send(BuildEvent::Failed(e)),
        }
    });

    rx
}

fn run_build(req: &BuildRequest, emit: &dyn Fn(BuildEvent)) -> Result<PathBuf, String> {
    for f in &req.files {
        match std::fs::metadata(&f.path) {
            Ok(m) if m.len() != f.size => {
                return Err(format!(
                    "File '{}' has changed since staging (was {} bytes, now {} bytes). Re-add the file and try again.",
                    f.path.display(), f.size, m.len()
                ));
            }
            Err(e) => {
                return Err(format!("Cannot access '{}': {}", f.path.display(), e));
            }
            Ok(_) => {}
        }
    }

    let resolved = resolve_names(&req.files);

    let mut iso_files = Vec::with_capacity(resolved.len());
    for (file, name) in &resolved {
        let mtime = file
            .metadata
            .mtime
            .as_ref()
            .and_then(|s| {
                s.parse::<chrono::DateTime<chrono::Utc>>()
                    .ok()
                    .map(|dt| dt.into())
            })
            .and_then(|st: std::time::SystemTime| IsoDateTime::from_system_time(st));
        iso_files.push(IsoFile {
            name: name.clone(),
            source: IsoFileSource::Path(file.path.clone()),
            size: file.size,
            mtime,
        });
    }

    // Optional manifest files (as virtual in-memory files).
    if let Some((fields, label)) = &req.manifest {
        emit(BuildEvent::Progress("Writing manifest...".into()));
        let triples = resolved
            .iter()
            .map(|(f, name)| (name.as_str(), f.sha256.as_str(), f.size, &f.metadata));
        let m = Manifest::build(label, fields, triples);
        iso_files.push(IsoFile {
            name: "MANIFEST.txt".to_string(),
            source: IsoFileSource::Bytes(m.to_text().into_bytes()),
            size: m.to_text().len() as u64,
            mtime: None,
        });
        iso_files.push(IsoFile {
            name: "MANIFEST.json".to_string(),
            source: IsoFileSource::Bytes(m.to_json().into_bytes()),
            size: m.to_json().len() as u64,
            mtime: None,
        });
    }

    // Ensure parent directory exists.
    if let Some(parent) = req.output.parent() {
        let _ = fs::create_dir_all(parent);
    }

    // Build the ISO.
    iso::write_iso_with_options(
        &req.output,
        &req.label,
        &iso_files,
        req.joliet_label_mode,
        &|msg| {
        emit(BuildEvent::Progress(msg.to_string()));
    },
    )?;

    // Sidecar checksum.
    emit(BuildEvent::Progress("Writing checksum sidecar...".into()));
    let iso_hash_rx = crate::hash::hash_async(&req.output);
    if let Ok(Ok(h)) = iso_hash_rx.recv() {
        let sidecar = req.output.with_extension(
            req.output
                .extension()
                .and_then(|s| s.to_str())
                .map(|s| format!("{s}.sha256"))
                .unwrap_or_else(|| "sha256".into()),
        );
        let line = format!(
            "{}  {}\n",
            h,
            req.output
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default()
        );
        let _ = fs::write(sidecar, line);
    }

    Ok(req.output.clone())
}

/// Resolve filename collisions: `foo.txt`, `foo_1.txt`, `foo_2.txt`, ...
pub fn resolve_names(files: &[StagedFile]) -> Vec<(StagedFile, String)> {
    let mut seen = HashMap::<String, u32>::new();
    let mut result = Vec::with_capacity(files.len());

    for f in files {
        let original = f
            .path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| f.path.display().to_string());

        let name = match seen.get_mut(&original) {
            Some(n) => {
                *n += 1;
                let (stem, ext) = split_ext(&original);
                if ext.is_empty() {
                    format!("{stem}_{n}")
                } else {
                    format!("{stem}_{n}.{ext}")
                }
            }
            None => {
                seen.insert(original.clone(), 0);
                original
            }
        };

        result.push((f.clone(), name));
    }

    result
}

fn split_ext(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i + 1..]),
        _ => (name, ""),
    }
}
