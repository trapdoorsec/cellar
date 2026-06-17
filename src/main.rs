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
// main.rs - Entry point for the cellar application.

mod app;
mod backend;
mod cli;
mod hash;
mod iso;
mod manifest;

use clap::Parser;

fn main() {
    let args = cli::Args::parse();

    if args.no_gui {
        cli::run_headless(&args);
        return;
    }

    if let Err(e) = run_gui() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run_gui() -> eframe::Result<()> {
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([720.0, 640.0])
        .with_min_inner_size([520.0, 480.0])
        .with_title("cellar")
        .with_app_id("cellar")
        .with_drag_and_drop(true);

    // only meaningful on Windows/X11; macOS uses the icon from the bundle, wayland does wayland things
    // either way, its a packaging issue for mac and wayland
    if let Some(icon) = load_icon() {
        viewport = viewport.with_icon(icon);
    }

    #[allow(unused_mut)]
    let mut options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    // winit 0.30 still has no Wayland drag-and-drop implementation
    // (only X11/Windows/macOS), and `WINIT_UNIX_BACKEND` is no longer honored.
    // Force the X11 backend programmatically on Linux so that, when running
    // under a Wayland session, we route through XWayland and DnD works.
    // Remove this once https://github.com/rust-windowing/winit lands Wayland DnD.
    #[cfg(target_os = "linux")]
    {
        use winit::platform::x11::EventLoopBuilderExtX11;
        options.event_loop_builder = Some(Box::new(|builder| {
            builder.with_x11();
        }));
    }

    eframe::run_native(
        "cellar",
        options,
        Box::new(|cc| {
            cc.egui_ctx.set_visuals(egui::Visuals::dark());
            Ok(Box::new(app::CellarApp::new(cc)))
        }),
    )
}

fn load_icon() -> Option<egui::IconData> {
    let bytes = include_bytes!("../assets/cellar-128x128.png");
    let img = image::load_from_memory(bytes).ok()?;
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    Some(egui::IconData {
        rgba: rgba.into_raw(),
        width: w,
        height: h,
    })
}