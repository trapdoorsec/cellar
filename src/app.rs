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
// app.rs - Main application logic and entry point.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Instant;

use chrono::Local;
use eframe::CreationContext;
use egui::{Color32, RichText};
use serde::{Deserialize, Serialize};

use crate::backend::{self, BuildEvent, BuildRequest, StagedFile};
use crate::hash;
use crate::iso::JolietLabelMode;
use crate::manifest::{FileMetadata, ManifestFields};

// Paths that, if dragged in, deserve a "are you sure?" before we'll proceed.
const SUSPICIOUS_PATH_HINTS: &[&str] = &[
    ".ssh",
    ".gnupg",
    ".aws",
    ".config",
    ".kube",
    ".npmrc",
    ".pypirc",
    ".gitconfig",
    "id_rsa",
    "id_ed25519",
];

const LARGE_FILE_THRESHOLD: u64 = 100 * 1024 * 1024; // 100 MB

#[derive(Serialize, Deserialize, Default)]
struct Persistent {
    output_dir: Option<PathBuf>,
    research_mode: bool,
    #[serde(default)]
    joliet_label_mode: UiJolietLabelMode,
}

#[derive(Serialize, Deserialize, Clone, Copy, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum UiJolietLabelMode {
    #[default]
    Strict,
    Legacy,
}

impl From<UiJolietLabelMode> for JolietLabelMode {
    fn from(value: UiJolietLabelMode) -> Self {
        match value {
            UiJolietLabelMode::Strict => JolietLabelMode::Strict,
            UiJolietLabelMode::Legacy => JolietLabelMode::Legacy,
        }
    }
}

pub struct CellarApp {
    persistent: Persistent,

    // Staged files, in insertion order.
    files: Vec<FileEntry>,

    // Build config.
    label: String,
    output_filename: String,
    prev_label: String,
    manifest: ManifestFields,

    // Build state.
    build_rx: Option<mpsc::Receiver<BuildEvent>>,
    build_status: BuildStatus,
    last_output: Option<PathBuf>,

    // Confirmation gate for suspicious paths.
    confirm: Option<ConfirmDialog>,

    // Whether files are currently being hovered over the window.
    files_hovered: bool,
}

enum BuildStatus {
    Idle,
    Running { since: Instant, last_msg: String },
    Done(PathBuf),
    Failed(String),
}

struct ConfirmDialog {
    message: String,
    proceed_build: bool,
}

struct FileEntry {
    path: PathBuf,
    size: u64,
    hash: HashState,
    metadata: FileMetadata,
}

enum HashState {
    Pending(mpsc::Receiver<Result<String, String>>),
    Ready(String),
    Failed(String),
}

impl CellarApp {
    pub fn new(cc: &CreationContext) -> Self {
        let persistent: Persistent = cc
            .storage
            .and_then(|s| eframe::get_value(s, "cellar"))
            .unwrap_or_default();

        Self {
            persistent,
            files: Vec::new(),
            label: default_label(),
            output_filename: default_filename(),
            prev_label: default_label(),
            manifest: ManifestFields::default(),
            build_rx: None,
            build_status: BuildStatus::Idle,
            last_output: None,
            confirm: None,
            files_hovered: false,
        }
    }

    fn add_path(&mut self, path: PathBuf) {
        // Reject duplicates (same canonical path).
        let canon = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
        if self
            .files
            .iter()
            .any(|f| std::fs::canonicalize(&f.path).unwrap_or_else(|_| f.path.clone()) == canon)
        {
            return;
        }

        // Directories aren't supported in MVP — flatten would surprise users.
        if path.is_dir() {
            return;
        }

        let (size, metadata) = match std::fs::metadata(&path) {
            Ok(m) => {
                let size = m.len();
                let meta = extract_metadata(&m);
                (size, meta)
            }
            Err(_) => (0, FileMetadata::default()),
        };
        let rx = hash::hash_async(&path);

        self.files.push(FileEntry {
            path,
            size,
            hash: HashState::Pending(rx),
            metadata,
        });
    }

    fn poll_hashes(&mut self) -> bool {
        let mut any_ready = false;
        for f in &mut self.files {
            if let HashState::Pending(rx) = &f.hash {
                match rx.try_recv() {
                    Ok(Ok(h)) => {
                        f.hash = HashState::Ready(h);
                        any_ready = true;
                    }
                    Ok(Err(e)) => {
                        f.hash = HashState::Failed(e);
                        any_ready = true;
                    }
                    Err(mpsc::TryRecvError::Empty) => {}
                    Err(mpsc::TryRecvError::Disconnected) => {
                        f.hash = HashState::Failed("hash worker died".into());
                        any_ready = true;
                    }
                }
            }
        }
        any_ready
    }

    fn poll_build(&mut self) -> bool {
        let mut redraw = false;

        // Take the receiver out so the rest of `self` is freely mutable. We'll
        // put it back if the build is still going.
        let Some(rx) = self.build_rx.take() else {
            return false;
        };
        let mut keep_rx = Some(rx);

        while let Some(rx) = keep_rx.as_ref() {
            match rx.try_recv() {
                Ok(BuildEvent::Progress(msg)) => {
                    let since = match &self.build_status {
                        BuildStatus::Running { since, .. } => *since,
                        _ => Instant::now(),
                    };
                    self.build_status = BuildStatus::Running {
                        since,
                        last_msg: msg,
                    };
                    redraw = true;
                }
                Ok(BuildEvent::Done(path)) => {
                    self.last_output = Some(path.clone());
                    self.build_status = BuildStatus::Done(path);
                    keep_rx = None;
                    redraw = true;
                }
                Ok(BuildEvent::Failed(e)) => {
                    self.build_status = BuildStatus::Failed(e);
                    keep_rx = None;
                    redraw = true;
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    keep_rx = None;
                    break;
                }
            }
        }

        self.build_rx = keep_rx;
        redraw
    }

    fn all_hashes_ready(&self) -> bool {
        self.files
            .iter()
            .all(|f| matches!(f.hash, HashState::Ready(_)))
    }

    fn suspicious_findings(&self) -> Vec<String> {
        let mut findings = Vec::new();
        for f in &self.files {
            let path_lower = f.path.to_string_lossy().to_lowercase();
            for hint in SUSPICIOUS_PATH_HINTS {
                if path_lower.contains(hint) {
                    findings.push(format!("{} contains '{}'", f.path.display(), hint));
                }
            }
            if f.size > LARGE_FILE_THRESHOLD {
                findings.push(format!(
                    "{} is {} (over {})",
                    f.path.display(),
                    human_size(f.size),
                    human_size(LARGE_FILE_THRESHOLD)
                ));
            }
        }
        findings
    }

    fn start_build(&mut self) {
        let output = self.resolve_output_path();
        if let Some(parent) = output.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        let staged: Vec<StagedFile> = self
            .files
            .iter()
            .filter_map(|f| match &f.hash {
                HashState::Ready(h) => Some(StagedFile {
                    path: f.path.clone(),
                    sha256: h.clone(),
                    size: f.size,
                    metadata: f.metadata.clone(),
                }),
                _ => None,
            })
            .collect();

        let manifest = if self.persistent.research_mode {
            Some((self.manifest.clone(), self.label.clone()))
        } else {
            None
        };

        let req = BuildRequest {
            files: staged,
            output,
            label: self.label.clone(),
            joliet_label_mode: self.persistent.joliet_label_mode.into(),
            manifest,
        };

        self.build_rx = Some(backend::build_async(req));
        self.build_status = BuildStatus::Running {
            since: Instant::now(),
            last_msg: "Starting...".into(),
        };
    }

    fn resolve_output_path(&self) -> PathBuf {
        let dir = self
            .persistent
            .output_dir
            .clone()
            .or_else(dirs_desktop)
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

        let filename = if self.output_filename.trim().is_empty() {
            format!("{}.iso", default_label())
        } else if self.output_filename.to_lowercase().ends_with(".iso") {
            self.output_filename.clone()
        } else {
            format!("{}.iso", self.output_filename)
        };

        dir.join(filename)
    }

    fn sync_filename_to_label(&mut self) {
        if self.label != self.prev_label {
            let safe_label = sanitize_filename(&self.label);
            let stamp = Local::now().format("%Y%m%d-%H%M%S");
            self.output_filename = format!("{safe_label}-{stamp}.iso");
            self.prev_label = self.label.clone();
        }
    }
}

impl eframe::App for CellarApp {
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, "cellar", &self.persistent);
    }

    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Pump async work.
        let hashes_changed = self.poll_hashes();
        let build_changed = self.poll_build();
        let pending_hashes = self
            .files
            .iter()
            .any(|f| matches!(f.hash, HashState::Pending(_)));
        let building = self.build_rx.is_some();
        if hashes_changed || build_changed || pending_hashes || building {
            ctx.request_repaint_after(std::time::Duration::from_millis(80));
        }

        // Sync output filename when label changes.
        self.sync_filename_to_label();

        // Drag-drop intake from the OS.
        let mut dropped_paths: Vec<PathBuf> = Vec::new();
        let mut hovering = false;
        ctx.input(|i| {
            hovering = !i.raw.hovered_files.is_empty();
            for f in &i.raw.dropped_files {
                if let Some(p) = f.path.clone() {
                    dropped_paths.push(p);
                } else if !f.name.is_empty() {
                    if let Some(tmp) = save_dropped_bytes(&f.name, f.bytes.as_deref()) {
                        dropped_paths.push(tmp);
                    }
                }
            }
        });
        if hovering {
            ctx.request_repaint();
        }
        self.files_hovered = hovering;
        for p in dropped_paths {
            self.add_path(p);
        }

        // Confirmation dialog renders on top of everything else.
        let confirmed = self.show_confirm_dialog(ctx);
        if confirmed {
            self.start_build();
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default_margins().show_inside(ui, |ui| {
            self.header(ui);
            ui.add_space(8.0);
            self.staging_area(ui);
            ui.add_space(8.0);
            self.options_panel(ui);
            ui.add_space(8.0);
            self.build_panel(ui);
        });
    }
}

impl CellarApp {
    fn header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("cellar")
                    .size(22.0)
                    .strong()
                    .color(Color32::from_rgb(220, 215, 200)),
            );
            ui.label(
                RichText::new("Have files. Will ISO.")
                    .color(Color32::from_rgb(140, 140, 140))
                    .italics(),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let toggle = ui.checkbox(&mut self.persistent.research_mode, "Research mode");
                toggle.on_hover_text(
                    "Adds a manifest, source/notes fields, and shows suspicious-path warnings.",
                );
                ui.label(
                    RichText::new(env!("CARGO_PKG_VERSION"))
                        .small()
                        .color(Color32::from_rgb(80, 80, 80)),
                );
            });
        });
    }

    fn staging_area(&mut self, ui: &mut egui::Ui) {
        let (fill, stroke) = if self.files_hovered {
            (
                Color32::from_rgb(40, 48, 40),
                Color32::from_rgb(90, 160, 90),
            )
        } else {
            (
                Color32::from_rgb(28, 28, 32),
                Color32::from_rgb(60, 60, 68),
            )
        };

        let frame = egui::Frame::default()
            .fill(fill)
            .stroke(egui::Stroke::new(if self.files_hovered { 2.0 } else { 1.0 }, stroke))
            .inner_margin(egui::Margin::same(12))
            .corner_radius(6.0);

        frame.show(ui, |ui| {
            ui.set_min_height(180.0);

            if self.files.is_empty() {
                ui.vertical_centered(|ui| {
                    ui.add_space(40.0);
                    ui.label(
                        RichText::new(if self.files_hovered {
                            "Drop files here — release to add"
                        } else {
                            "Drop files here"
                        })
                        .size(16.0)
                        .color(if self.files_hovered {
                            Color32::from_rgb(120, 200, 120)
                        } else {
                            Color32::from_rgb(160, 160, 160)
                        }),
                    );
                    ui.add_space(6.0);
                    if ui.small_button("or browse...").clicked() {
                        if let Some(paths) = rfd::FileDialog::new().pick_files() {
                            for p in paths {
                                self.add_path(p);
                            }
                        }
                    }
                });
                return;
            }

            let mut to_remove: Option<usize> = None;
            ui.label(
                RichText::new(if self.files_hovered {
                    "Release to add files"
                } else {
                    "Drag and drop files into this panel to add more"
                })
                .small()
                .color(if self.files_hovered {
                    Color32::from_rgb(120, 200, 120)
                } else {
                    Color32::from_rgb(130, 130, 130)
                }),
            );
            ui.add_space(4.0);
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .max_height(220.0)
                .show(ui, |ui| {
                    for (idx, file) in self.files.iter().enumerate() {
                        ui.horizontal(|ui| {
                            // Name (truncated) + size on left, hash on right, X to remove.
                            let name = file
                                .path
                                .file_name()
                                .map(|s| s.to_string_lossy().into_owned())
                                .unwrap_or_else(|| file.path.display().to_string());

                            ui.label(
                                RichText::new(name)
                                    .color(Color32::from_rgb(220, 215, 200))
                                    .monospace(),
                            )
                            .on_hover_text(file.path.display().to_string());

                            ui.label(
                                RichText::new(human_size(file.size))
                                    .color(Color32::from_rgb(130, 130, 130))
                                    .small(),
                            );

                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui.small_button("✕").clicked() {
                                        to_remove = Some(idx);
                                    }
                                    match &file.hash {
                                        HashState::Pending(_) => {
                                            ui.spinner();
                                            ui.label(
                                                RichText::new("hashing")
                                                    .color(Color32::from_rgb(130, 130, 130))
                                                    .small(),
                                            );
                                        }
                                        HashState::Ready(h) => {
                                            let short = &h[..12.min(h.len())];
                                            ui.label(
                                                RichText::new(short)
                                                    .color(Color32::from_rgb(120, 170, 130))
                                                    .monospace()
                                                    .small(),
                                            )
                                            .on_hover_text(h);
                                        }
                                        HashState::Failed(e) => {
                                            ui.colored_label(
                                                Color32::from_rgb(220, 110, 80),
                                                "hash failed",
                                            )
                                            .on_hover_text(e);
                                        }
                                    }
                                },
                            );
                        });
                    }
                });

            if let Some(idx) = to_remove {
                self.files.remove(idx);
            }

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.small_button("Add files...").clicked() {
                    if let Some(paths) = rfd::FileDialog::new().pick_files() {
                        for p in paths {
                            self.add_path(p);
                        }
                    }
                }
                if ui.small_button("Clear all").clicked() {
                    self.files.clear();
                }
                let total: u64 = self.files.iter().map(|f| f.size).sum();
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        RichText::new(format!(
                            "{} file{} · {}",
                            self.files.len(),
                            if self.files.len() == 1 { "" } else { "s" },
                            human_size(total)
                        ))
                        .color(Color32::from_rgb(140, 140, 140))
                        .small(),
                    );
                });
            });
        });
    }

    fn options_panel(&mut self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new("Output")
            .default_open(true)
            .show(ui, |ui| {
                egui::Grid::new("output-grid")
                    .num_columns(2)
                    .spacing([12.0, 6.0])
                    .show(ui, |ui| {
                        ui.label("Label");
                        ui.text_edit_singleline(&mut self.label);
                        ui.end_row();

                        ui.label("Filename");
                        ui.horizontal(|ui| {
                            let avail = ui.available_width();
                            ui.add(
                                egui::TextEdit::singleline(&mut self.output_filename)
                                    .desired_width(avail),
                            );
                            if ui.small_button("Reset").clicked() {
                                let safe_label = sanitize_filename(&self.label);
                                let stamp = Local::now().format("%Y%m%d-%H%M%S");
                                self.output_filename = format!("{safe_label}-{stamp}.iso");
                            }
                        });
                        ui.end_row();

                        ui.label("Save to");
                        ui.horizontal(|ui| {
                            let current = self
                                .persistent
                                .output_dir
                                .as_ref()
                                .map(|p| p.display().to_string())
                                .unwrap_or_else(|| {
                                    dirs_desktop()
                                        .map(|p| p.display().to_string())
                                        .unwrap_or_else(|| "(working dir)".into())
                                });
                            ui.label(
                                RichText::new(current)
                                    .color(Color32::from_rgb(180, 180, 180))
                                    .monospace()
                                    .small(),
                            );
                            if ui.small_button("Change...").clicked() {
                                if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                                    self.persistent.output_dir = Some(dir);
                                }
                            }
                        });
                        ui.end_row();

                        ui.label("Joliet label");
                        egui::ComboBox::from_id_salt("joliet-label-mode")
                            .selected_text(match self.persistent.joliet_label_mode {
                                UiJolietLabelMode::Strict => "Strict (UCS-2)",
                                UiJolietLabelMode::Legacy => "Legacy (blank SVD label)",
                            })
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut self.persistent.joliet_label_mode,
                                    UiJolietLabelMode::Strict,
                                    "Strict (UCS-2)",
                                );
                                ui.selectable_value(
                                    &mut self.persistent.joliet_label_mode,
                                    UiJolietLabelMode::Legacy,
                                    "Legacy (blank SVD label)",
                                );
                            });
                        ui.end_row();
                    });

                ui.label(
                    RichText::new(
                        "Legacy mode keeps Joliet filenames but leaves the Joliet volume label blank for picky readers.",
                    )
                    .color(Color32::from_rgb(140, 140, 140))
                    .small(),
                );
            });

        if self.persistent.research_mode {
            egui::CollapsingHeader::new("Manifest")
                .default_open(false)
                .show(ui, |ui| {
                    egui::Grid::new("manifest-grid")
                        .num_columns(2)
                        .spacing([12.0, 6.0])
                        .show(ui, |ui| {
                            ui.label("Source");
                            ui.text_edit_singleline(&mut self.manifest.source);
                            ui.end_row();

                            ui.label("Package");
                            ui.horizontal(|ui| {
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.manifest.package_name)
                                        .hint_text("name"),
                                );
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.manifest.package_version)
                                        .hint_text("version"),
                                );
                            });
                            ui.end_row();

                            ui.label("Severity");
                            ui.text_edit_singleline(&mut self.manifest.severity);
                            ui.end_row();

                            ui.label("References");
                            ui.add(
                                egui::TextEdit::multiline(&mut self.manifest.references)
                                    .desired_rows(2)
                                    .hint_text("One URL per line"),
                            );
                            ui.end_row();

                            ui.label("Notes");
                            ui.add(
                                egui::TextEdit::multiline(&mut self.manifest.notes).desired_rows(3),
                            );
                            ui.end_row();
                        });
                });
        }
    }

    fn build_panel(&mut self, ui: &mut egui::Ui) {
        ui.separator();
        ui.add_space(4.0);

        let can_build =
            !self.files.is_empty() && self.all_hashes_ready() && self.build_rx.is_none();

        ui.horizontal(|ui| {
            let button = egui::Button::new(RichText::new("Build ISO").size(15.0).strong())
                .min_size(egui::vec2(140.0, 32.0));
            if ui.add_enabled(can_build, button).clicked() {
                let findings = if self.persistent.research_mode {
                    self.suspicious_findings()
                } else {
                    Vec::new()
                };
                if findings.is_empty() {
                    self.start_build();
                } else {
                    self.confirm = Some(ConfirmDialog {
                        message: format!(
                            "These look unusual:\n\n{}\n\nBuild anyway?",
                            findings.join("\n")
                        ),
                        proceed_build: false,
                    });
                }
            }

            if !can_build {
                let reason = if self.files.is_empty() {
                    "Add at least one file."
                } else if !self.all_hashes_ready() {
                    "Waiting for hashes to finish..."
                } else {
                    "Build in progress."
                };
                ui.label(
                    RichText::new(reason)
                        .color(Color32::from_rgb(140, 140, 140))
                        .small(),
                );
            }
        });

        ui.add_space(6.0);

        match &self.build_status {
            BuildStatus::Idle => {}
            BuildStatus::Running { since, last_msg } => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(
                        RichText::new(format!(
                            "{} · {:.1}s",
                            last_msg,
                            since.elapsed().as_secs_f32()
                        ))
                        .color(Color32::from_rgb(180, 180, 180)),
                    );
                });
            }
            BuildStatus::Done(path) => {
                ui.horizontal(|ui| {
                    ui.colored_label(Color32::from_rgb(120, 170, 130), "Done.");
                    ui.label(
                        RichText::new(path.display().to_string())
                            .monospace()
                            .small(),
                    );
                });
                ui.horizontal(|ui| {
                    if ui.small_button("Show in file manager").clicked() {
                        let target = path.parent().unwrap_or(path);
                        let _ = open_path(target);
                    }
                    if ui.small_button("Copy path").clicked() {
                        ui.ctx().copy_text(path.display().to_string());
                    }
                });
            }
            BuildStatus::Failed(e) => {
                ui.colored_label(Color32::from_rgb(220, 110, 80), format!("Failed: {e}"));
            }
        }
    }

    /// Returns true when the user confirmed and the build should start.
    fn show_confirm_dialog(&mut self, ctx: &egui::Context) -> bool {
        let mut should_proceed = false;
        let mut should_close = false;

        if let Some(dialog) = &mut self.confirm {
            egui::Window::new("Confirm")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label(&dialog.message);
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Cancel").clicked() {
                            should_close = true;
                        }
                        if ui.button("Build anyway").clicked() {
                            dialog.proceed_build = true;
                            should_proceed = true;
                            should_close = true;
                        }
                    });
                });
        }

        if should_close {
            self.confirm = None;
        }
        should_proceed
    }
}

fn default_label() -> String {
    "cellar".into()
}

fn default_filename() -> String {
    let stamp = Local::now().format("%Y%m%d-%H%M%S");
    let safe_label = sanitize_filename("cellar");
    format!("{safe_label}-{stamp}.iso")
}

fn sanitize_filename(label: &str) -> String {
    let s: String = label
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let s = s.trim_matches('-');
    if s.is_empty() { "iso" } else { s }.to_string()
}

fn human_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}

fn extract_metadata(m: &std::fs::Metadata) -> FileMetadata {
    let mtime = m
        .modified()
        .ok()
        .map(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339());
    let atime = m
        .accessed()
        .ok()
        .map(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339());
    let ctime = m
        .created()
        .ok()
        .map(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339());

    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        FileMetadata {
            mtime,
            atime,
            ctime,
            permissions: Some(m.mode()),
            uid: Some(m.uid()),
            gid: Some(m.gid()),
        }
    }

    #[cfg(not(unix))]
    {
        FileMetadata {
            mtime,
            atime,
            ctime,
            permissions: None,
            uid: None,
            gid: None,
        }
    }
}

fn dirs_desktop() -> Option<PathBuf> {
    // Minimal home/desktop resolution without pulling in `dirs`.
    if let Ok(home) = std::env::var("HOME") {
        let p = PathBuf::from(home).join("Desktop");
        if p.exists() {
            return Some(p);
        }
    }
    if let Ok(home) = std::env::var("USERPROFILE") {
        let p = PathBuf::from(home).join("Desktop");
        if p.exists() {
            return Some(p);
        }
    }
    None
}

fn open_path(path: &Path) -> std::io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open").arg(path).spawn()?;
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(path).spawn()?;
    }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer").arg(path).spawn()?;
    }
    let _ = path; // silence unused warning on platforms not above
    Ok(())
}

/// When a dropped file arrives without a path (e.g. on Wayland from a browser),
/// write its bytes to a temp file so it can be staged like any other file.
fn save_dropped_bytes(name: &str, data: Option<&[u8]>) -> Option<PathBuf> {
    let data = data?;
    let dir = std::env::temp_dir().join("cellar-drops");
    std::fs::create_dir_all(&dir).ok()?;
    let safe_name: String = name.chars().map(|c| if c.is_alphanumeric() || c == '.' { c } else { '_' }).collect();
    let path = dir.join(if safe_name.is_empty() { "dropped_file".into() } else { safe_name });
    std::fs::write(&path, data).ok()?;
    Some(path)
}
