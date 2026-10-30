mod app;
use anyhow::{Context, Result, bail};
use caelum_rs::model::{self, State};
use std::path::PathBuf;
fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let mut profile = model::profile();
    let mut import = None;
    let mut smoke = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                println!(
                    "Caelum Rust {}\n\nUsage: caelum-rs [--data-dir PATH] [--import-legacy PATH] [--smoke-test REPORT.json]\n\nThe Rust preview has a separate profile. Legacy import copies a workspace without modifying its source.",
                    env!("CARGO_PKG_VERSION")
                );
                return Ok(());
            }
            "--version" | "-V" => {
                println!("Caelum Rust {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            "--data-dir" => {
                profile = PathBuf::from(args.next().context("--data-dir needs a path")?)
            }
            "--import-legacy" => {
                import = Some(PathBuf::from(
                    args.next()
                        .context("--import-legacy needs workspace.json")?,
                ))
            }
            "--smoke-test" => {
                smoke = Some(PathBuf::from(
                    args.next().context("--smoke-test needs a report path")?,
                ))
            }
            _ => bail!("Unknown argument: {arg}"),
        }
    }
    let _smoke_profile = if smoke.is_some() {
        let t = tempfile::tempdir()?;
        profile = t.path().to_path_buf();
        Some(t)
    } else {
        None
    };
    caelum_rs::model::create_private_dir(&profile, true)?;
    // Never load workspace/drafts another user could have written: they can launch commands.
    caelum_rs::model::check_profile_trust(&profile)?;
    // A process-held lock prevents two windows from racing workspace writes.
    let _lock =
        caelum_rs::platform::lock_instance(&profile.join("app.lock"))?.with_context(|| {
            format!(
                "Caelum Rust is already using {}. Open another profile with --data-dir.",
                profile.display()
            )
        })?;
    if let Some(source) = import {
        State::import_legacy(&source, &profile.join("workspace.json"))?;
    }
    let file = profile.join("workspace.json");
    let mut message = String::new();
    let state = if file.exists() {
        match State::read(&file) {
            Ok(s) => s,
            Err(e) => {
                bail!(
                    "Workspace could not be loaded; preserved at {}: {e}. Use --data-dir to start a separate profile.",
                    file.display()
                );
            }
        }
    } else {
        State::default()
    };
    if let Some(first) = state.warnings.first() {
        message = first.clone();
    }
    if smoke.is_some() {
        message = "Desktop validation · isolated profile".into();
    }
    let icon = eframe::icon_data::from_png_bytes(include_bytes!("../assets/brand/icon.png"))
        .context("Loading app icon")?;
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Caelum Rust")
            .with_app_id("io.github.DakodaStemen.CaelumRust")
            .with_inner_size([1360., 880.])
            .with_min_inner_size([760., 480.])
            .with_icon(icon),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    let report = smoke.clone();
    eframe::run_native(
        "io.github.DakodaStemen.CaelumRust",
        options,
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, profile, state, message, smoke)))),
    )
    .map_err(|e| anyhow::anyhow!(e.to_string()))?;
    if let Some(report) = report {
        let value: serde_json::Value = serde_json::from_slice(&std::fs::read(&report)?)?;
        if value["success"] != true {
            bail!("Desktop checks failed: {}", value["error"]);
        }
    }
    Ok(())
}
