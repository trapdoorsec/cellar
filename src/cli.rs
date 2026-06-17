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
// GNU General Public License along with this program. If not, see <https://www.gnu.org/licenses/>.
// --------------------------------------------------------- //
// Cellar - Cross-platform GUI for ISO 9660 image creation.  //
// Joliet support for long filenames.                        //
// --------------------------------------------------------- //
// cli.rs - Headless (no-GUI) build path.

use std::path::PathBuf;
use std::process;

use clap::Parser;

use crate::backend::{self, StagedFile};
use crate::hash;
use crate::iso::JolietLabelMode;
use crate::manifest::{FileMetadata, ManifestFields};

#[derive(Parser, Debug)]
#[command(name = "cellar", version, about = "Have files. Will ISO.")]
pub struct Args {
    #[arg(long, help = "Run without the GUI (headless mode)")]
    pub no_gui: bool,

    #[arg(long, short, help = "Output ISO file path")]
    pub output: Option<PathBuf>,

    #[arg(long, short, help = "Volume label")]
    pub label: Option<String>,

    #[arg(long, help = "Use Legacy Joliet label mode (blank SVD label)")]
    pub legacy_label: bool,

    #[arg(long, num_args = 1.., help = "Files to include in the ISO")]
    pub files: Vec<PathBuf>,

    #[arg(long, help = "Source URL for manifest")]
    pub manifest_source: Option<String>,

    #[arg(long, help = "Package name for manifest")]
    pub manifest_name: Option<String>,

    #[arg(long, help = "Package version for manifest")]
    pub manifest_version: Option<String>,

    #[arg(long, help = "Severity for manifest")]
    pub manifest_severity: Option<String>,

    #[arg(long, help = "References (one per line) for manifest")]
    pub manifest_references: Option<String>,

    #[arg(long, help = "Notes for manifest")]
    pub manifest_notes: Option<String>,
}

pub fn run_headless(args: &Args) {
    if args.files.is_empty() {
        eprintln!("error: --no-gui requires at least one --files path");
        process::exit(1);
    }

    let label = args.label.as_deref().unwrap_or("CELLAR");

    let output = match args.output {
        Some(ref p) => p.clone(),
        None => {
            let safe = sanitize_filename(label);
            let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
            let dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            dir.join(format!("{safe}-{stamp}.iso"))
        }
    };

    let joliet_label_mode = if args.legacy_label {
        JolietLabelMode::Legacy
    } else {
        JolietLabelMode::Strict
    };

    let mut staged = Vec::with_capacity(args.files.len());
    for path in &args.files {
        let meta = match std::fs::metadata(path) {
            Ok(m) => m,
            Err(e) => {
                eprintln!("error: cannot access '{}': {e}", path.display());
                process::exit(1);
            }
        };
        if !meta.is_file() {
            eprintln!("error: '{}' is not a regular file", path.display());
            process::exit(1);
        }
        eprintln!("hashing {}...", path.display());
        let hash_rx = hash::hash_async(path);
        let sha256 = match hash_rx.recv() {
            Ok(Ok(h)) => h,
            Ok(Err(e)) => {
                eprintln!("error hashing '{}': {e}", path.display());
                process::exit(1);
            }
            Err(_) => {
                eprintln!("error hashing '{}': thread panicked", path.display());
                process::exit(1);
            }
        };

        let mtime = meta.modified().ok().map(|t| {
            let dt: chrono::DateTime<chrono::Utc> = t.into();
            dt.to_rfc3339()
        });

        let permissions = {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                Some(meta.permissions().mode())
            }
            #[cfg(not(unix))]
            {
                None::<u32>
            }
        };

        let uid = {
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                Some(meta.uid())
            }
            #[cfg(not(unix))]
            {
                None::<u32>
            }
        };

        let gid = {
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                Some(meta.gid())
            }
            #[cfg(not(unix))]
            {
                None::<u32>
            }
        };

        staged.push(StagedFile {
            path: path.clone(),
            sha256,
            size: meta.len(),
            metadata: FileMetadata {
                mtime,
                atime: None,
                ctime: None,
                permissions,
                uid,
                gid,
            },
        });
    }

    let manifest = if args.manifest_source.is_some()
        || args.manifest_name.is_some()
        || args.manifest_version.is_some()
        || args.manifest_severity.is_some()
        || args.manifest_references.is_some()
        || args.manifest_notes.is_some()
    {
        Some((
            ManifestFields {
                source: args.manifest_source.clone().unwrap_or_default(),
                package_name: args.manifest_name.clone().unwrap_or_default(),
                package_version: args.manifest_version.clone().unwrap_or_default(),
                severity: args.manifest_severity.clone().unwrap_or_default(),
                references: args.manifest_references.clone().unwrap_or_default(),
                notes: args.manifest_notes.clone().unwrap_or_default(),
            },
            label.to_string(),
        ))
    } else {
        None
    };

    let req = backend::BuildRequest {
        files: staged,
        output: output.clone(),
        label: label.to_string(),
        joliet_label_mode,
        manifest,
    };

    eprintln!("building {}...", output.display());
    let rx = backend::build_async(req);

    while let Ok(event) = rx.recv() {
        match event {
            backend::BuildEvent::Progress(msg) => eprintln!("  {msg}"),
            backend::BuildEvent::Done(path) => {
                eprintln!("done: {}", path.display());
                return;
            }
            backend::BuildEvent::Failed(msg) => {
                eprintln!("error: {msg}");
                process::exit(1);
            }
        }
    }
}

fn sanitize_filename(label: &str) -> String {
    let s: String = label
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let s = s.trim_matches('-');
    if s.is_empty() {
        "iso".to_string()
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn no_gui_defaults_false() {
        let args = Args::try_parse_from(["cellar"]).unwrap();
        assert!(!args.no_gui);
    }

    #[test]
    fn no_gui_flag_sets_true() {
        let args = Args::try_parse_from(["cellar", "--no-gui", "--files", "/dev/null"]).unwrap();
        assert!(args.no_gui);
    }

    #[test]
    fn legacy_label_defaults_false() {
        let args = Args::try_parse_from(["cellar"]).unwrap();
        assert!(!args.legacy_label);
    }

    #[test]
    fn legacy_label_flag_sets_true() {
        let args = Args::try_parse_from(["cellar", "--legacy-label"]).unwrap();
        assert!(args.legacy_label);
    }

    #[test]
    fn label_defaults_none() {
        let args = Args::try_parse_from(["cellar"]).unwrap();
        assert!(args.label.is_none());
    }

    #[test]
    fn label_short_and_long() {
        let short = Args::try_parse_from(["cellar", "-l", "MyLabel"]).unwrap();
        let long = Args::try_parse_from(["cellar", "--label", "MyLabel"]).unwrap();
        assert_eq!(short.label.as_deref(), Some("MyLabel"));
        assert_eq!(long.label.as_deref(), Some("MyLabel"));
    }

    #[test]
    fn output_defaults_none() {
        let args = Args::try_parse_from(["cellar"]).unwrap();
        assert!(args.output.is_none());
    }

    #[test]
    fn output_short_and_long() {
        let short = Args::try_parse_from(["cellar", "-o", "/tmp/out.iso"]).unwrap();
        let long = Args::try_parse_from(["cellar", "--output", "/tmp/out.iso"]).unwrap();
        assert_eq!(short.output.unwrap(), PathBuf::from("/tmp/out.iso"));
        assert_eq!(long.output.unwrap(), PathBuf::from("/tmp/out.iso"));
    }

    #[test]
    fn files_multiple() {
        let args = Args::try_parse_from(["cellar", "--files", "a.bin", "b.dat", "c.txt"]).unwrap();
        assert_eq!(args.files.len(), 3);
        assert_eq!(args.files[0], PathBuf::from("a.bin"));
        assert_eq!(args.files[1], PathBuf::from("b.dat"));
        assert_eq!(args.files[2], PathBuf::from("c.txt"));
    }

    #[test]
    fn sanitize_filename_alphanumeric() {
        assert_eq!(sanitize_filename("MyLabel"), "MyLabel");
    }

    #[test]
    fn sanitize_filename_replaces_special() {
        assert_eq!(sanitize_filename("my label!@#"), "my-label");
    }

    #[test]
    fn sanitize_filename_trims_dashes() {
        assert_eq!(sanitize_filename("--hello--"), "hello");
    }

    #[test]
    fn sanitize_filename_empty_input() {
        assert_eq!(sanitize_filename("!!!"), "iso");
    }

    #[test]
    fn legacy_label_maps_correctly() {
        let strict_args = Args::try_parse_from(["cellar"]).unwrap();
        let legacy_args = Args::try_parse_from(["cellar", "--legacy-label"]).unwrap();
        let strict_mode = if strict_args.legacy_label {
            JolietLabelMode::Legacy
        } else {
            JolietLabelMode::Strict
        };
        let legacy_mode = if legacy_args.legacy_label {
            JolietLabelMode::Legacy
        } else {
            JolietLabelMode::Strict
        };
        assert!(matches!(strict_mode, JolietLabelMode::Strict));
        assert!(matches!(legacy_mode, JolietLabelMode::Legacy));
    }
}
