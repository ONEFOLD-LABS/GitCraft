//! GitCraft desktop app: `gitcraft [path]` opens the repository that contains `path`.

use std::path::PathBuf;
use std::process::ExitCode;

use gitcraft_git::GitRunner;
use gitcraft_ui::{RepoWindow, i18n, theme};

const USAGE: &str = "\
Aufruf: gitcraft [pfad]

  pfad        Ordner (oder Datei) in einem Git-Repository, das geöffnet wird
  --version   Version anzeigen
  --help      Diese Hilfe

Ein Ordner lässt sich auch ins Fenster ziehen.";

/// Reverse-DNS id: Wayland app id, X11 WM class and desktop file name.
const APP_ID: &str = "io.github.onefold_labs.gitcraft";

struct App {
    window: RepoWindow,
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.window.ui(ui);
    }
}

fn main() -> ExitCode {
    let mut path: Option<PathBuf> = None;
    for arg in std::env::args_os().skip(1) {
        match arg.to_str() {
            Some("--version" | "-V") => {
                println!("gitcraft {}", env!("CARGO_PKG_VERSION"));
                return ExitCode::SUCCESS;
            }
            Some("--help" | "-h") => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            Some(a) if a.starts_with('-') => {
                eprintln!("Unbekannte Option {a}\n\n{USAGE}");
                return ExitCode::FAILURE;
            }
            _ if path.is_some() => {
                eprintln!("Nur ein Pfad möglich\n\n{USAGE}");
                return ExitCode::FAILURE;
            }
            _ => path = Some(PathBuf::from(arg)),
        }
    }
    // Relative to where GitCraft was started; default: the current directory, if it is a
    // repository (otherwise the window shows the error and lets the user pick another one).
    let initial = path.or_else(|| std::env::current_dir().ok());

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("GitCraft")
            .with_app_id(APP_ID)
            .with_inner_size([1000.0, 680.0])
            .with_min_inner_size([480.0, 320.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };
    let result = eframe::run_native(
        "GitCraft",
        options,
        Box::new(move |cc| {
            // File names come from repositories and may contain any character: draw a box for
            // one no font has, never stop.
            cc.egui_ctx
                .set_missing_glyph_policy(egui::MissingGlyphPolicy::Tofu);
            i18n::set_language(i18n::Language::System);
            theme::install(&cc.egui_ctx, egui::ThemePreference::System);
            Ok(Box::new(App {
                window: RepoWindow::new(GitRunner::new(), initial),
            }))
        }),
    );
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("Fehler: {e}");
            ExitCode::FAILURE
        }
    }
}
