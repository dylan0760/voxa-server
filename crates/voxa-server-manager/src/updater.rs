//! Background update checks / installs against GitHub Releases.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use voxa_proto::update::{self, Release};

#[derive(Clone)]
pub enum UpdState {
    Idle,
    Checking,
    UpToDate,
    Available(Release),
    Downloading(Release, u64, u64),
    /// The installer has been started; the app should exit now.
    Installing,
    Failed(String),
}

pub struct Updater {
    pub repo: &'static str,
    matcher: fn(&str) -> bool,
    pub state: Arc<Mutex<UpdState>>,
    last_check: Option<Instant>,
    ctx: egui::Context,
    /// Folder where the notes of an update being installed are saved, so "What's new"
    /// can be shown after the restart even without internet.
    notes_dir: Option<PathBuf>,
}

impl Updater {
    pub fn new(repo: &'static str, matcher: fn(&str) -> bool, ctx: egui::Context) -> Updater {
        Updater { repo, matcher, state: Arc::new(Mutex::new(UpdState::Idle)), last_check: None, ctx, notes_dir: None }
    }

    pub fn with_notes_dir(mut self, dir: PathBuf) -> Updater {
        self.notes_dir = Some(dir);
        self
    }

    pub fn get(&self) -> UpdState {
        self.state.lock().unwrap().clone()
    }

    pub fn available(&self) -> Option<Release> {
        match self.get() {
            UpdState::Available(r) => Some(r),
            _ => None,
        }
    }

    pub fn busy(&self) -> bool {
        matches!(self.get(), UpdState::Checking | UpdState::Downloading(..) | UpdState::Installing)
    }

    pub fn check(&mut self) {
        if self.busy() {
            return;
        }
        self.last_check = Some(Instant::now());
        *self.state.lock().unwrap() = UpdState::Checking;
        let (state, ctx, repo, matcher) = (self.state.clone(), self.ctx.clone(), self.repo, self.matcher);
        std::thread::spawn(move || {
            let r = match update::check(repo, voxa_proto::APP_VERSION, matcher) {
                Ok(Some(rel)) => UpdState::Available(rel),
                Ok(None) => UpdState::UpToDate,
                Err(e) => UpdState::Failed(e),
            };
            *state.lock().unwrap() = r;
            ctx.request_repaint();
        });
    }

    /// Periodic automatic check (every 12 hours, plus once at startup).
    pub fn auto_check(&mut self) {
        let due = self.last_check.map(|t| t.elapsed() > Duration::from_secs(12 * 3600)).unwrap_or(true);
        if due {
            self.check();
        }
    }

    pub fn install(&self, rel: Release) {
        if self.busy() {
            return;
        }
        *self.state.lock().unwrap() = UpdState::Downloading(rel.clone(), 0, rel.size);
        if let Some(dir) = &self.notes_dir {
            save_pending_notes(dir, &rel);
        }
        let (state, ctx) = (self.state.clone(), self.ctx.clone());
        std::thread::spawn(move || {
            let st = state.clone();
            let c = ctx.clone();
            let r2 = rel.clone();
            let res = update::download(&rel, move |done, total| {
                *st.lock().unwrap() = UpdState::Downloading(r2.clone(), done, total);
                c.request_repaint();
            })
            .and_then(|path| update::install(&path));
            *state.lock().unwrap() = match res {
                Ok(()) => UpdState::Installing,
                Err(e) => UpdState::Failed(e),
            };
            ctx.request_repaint();
        });
    }
}

// ------------------------------------------------------------------ what's new

const NOTES_FILE: &str = "update-notes.json";

fn save_pending_notes(dir: &Path, rel: &Release) {
    let _ = std::fs::create_dir_all(dir);
    let json = serde_json::json!({ "version": rel.version, "title": rel.name, "notes": rel.notes, "url": rel.page_url });
    let _ = std::fs::write(dir.join(NOTES_FILE), json.to_string());
}

fn load_pending_notes(dir: &Path, version: &str) -> Option<(String, String, String)> {
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join(NOTES_FILE)).ok()?).ok()?;
    if v["version"].as_str()?.trim_start_matches('v') != version.trim_start_matches('v') {
        return None;
    }
    Some((v["title"].as_str()?.to_string(), v["notes"].as_str()?.to_string(), v["url"].as_str().unwrap_or_default().to_string()))
}

#[derive(Clone)]
pub enum NotesState {
    Loading,
    Ready { title: String, notes: String, url: String },
    Failed(String),
}

/// "What's new" for the running version: fetched from GitHub, falling back to the
/// notes saved when the update was installed.
pub struct WhatsNew {
    pub version: String,
    pub state: Arc<Mutex<NotesState>>,
}

impl WhatsNew {
    pub fn fetch(repo: &'static str, version: &str, notes_dir: Option<PathBuf>, ctx: egui::Context) -> WhatsNew {
        let state = Arc::new(Mutex::new(NotesState::Loading));
        let (st, v) = (state.clone(), version.to_string());
        std::thread::spawn(move || {
            let r = match update::release_notes(repo, &v) {
                Ok((title, notes, url)) => NotesState::Ready { title, notes, url },
                Err(e) => match notes_dir.as_deref().and_then(|d| load_pending_notes(d, &v)) {
                    Some((title, notes, url)) => NotesState::Ready { title, notes, url },
                    None => NotesState::Failed(e),
                },
            };
            *st.lock().unwrap() = r;
            ctx.request_repaint();
        });
        WhatsNew { version: version.to_string(), state }
    }
}

/// Minimal Markdown rendering for release notes (headings, bullets, bold, links).
pub fn render_notes(ui: &mut egui::Ui, text: &str) {
    use egui::RichText;
    for raw in text.lines() {
        let line = raw.trim_end();
        let trimmed = line.trim_start();
        let indent = (line.len() - trimmed.len()) as f32 * 6.0;
        let clean = |s: &str| s.replace("**", "").replace('`', "");
        if trimmed.is_empty() {
            ui.add_space(4.0);
        } else if let Some(h) = trimmed.strip_prefix("### ").or_else(|| trimmed.strip_prefix("## ")).or_else(|| trimmed.strip_prefix("# ")) {
            ui.add_space(6.0);
            ui.label(RichText::new(clean(h)).strong().size(16.0));
        } else if let Some(item) = trimmed.strip_prefix("* ").or_else(|| trimmed.strip_prefix("- ")) {
            ui.horizontal_wrapped(|ui| {
                ui.add_space(indent + 6.0);
                ui.label("•");
                ui.label(clean(item));
            });
        } else {
            ui.horizontal_wrapped(|ui| {
                ui.add_space(indent);
                ui.label(clean(trimmed));
            });
        }
    }
}

/// The "What's new" window. Returns false once the user closed it.
pub fn whats_new_window(ctx: &egui::Context, wn: &WhatsNew, product: &str) -> bool {
    use egui::RichText;
    let mut open = true;
    let mut close = false;
    egui::Window::new(format!("🎉 What's new in {product} {}", wn.version))
        .id(egui::Id::unique("voxa_whats_new"))
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .default_size([520.0, 440.0])
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            let state = wn.state.lock().unwrap().clone();
            match state {
                NotesState::Loading => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Loading the change log...");
                    });
                }
                NotesState::Failed(e) => {
                    ui.label(format!("{product} was updated to {}.", wn.version));
                    ui.label(RichText::new(format!("The change log could not be loaded: {e}")).weak());
                }
                NotesState::Ready { title, notes, url } => {
                    ui.label(RichText::new(title).heading());
                    ui.separator();
                    egui::ScrollArea::vertical().max_height(340.0).auto_shrink([false, true]).show(ui, |ui| {
                        render_notes(ui, &notes);
                    });
                    ui.separator();
                    if !url.is_empty() && ui.link("View this release on GitHub").clicked() {
                        open_url(&url);
                    }
                }
            }
            ui.add_space(4.0);
            if ui.button("Got it").clicked() {
                close = true;
            }
        });
    open && !close
}

pub fn open_url(url: &str) {
    if url.starts_with("https://") {
        let _ = std::process::Command::new("explorer.exe").arg(url).spawn();
    }
}

/// Shared updates panel used by the client's Updates window.
pub fn ui(updater: &mut Updater, ui: &mut egui::Ui, product: &str) {
    use egui::{Color32, RichText};
    let dim = ui.visuals().weak_text_color();
    ui.label(format!("Installed: {product} {}", voxa_proto::APP_VERSION));
    ui.label(RichText::new(format!("Updates come from github.com/{}", updater.repo)).small().color(dim));
    ui.add_space(6.0);
    match updater.get() {
        UpdState::Idle => {
            ui.label("Click \"Check for updates\" to look for a newer version.");
        }
        UpdState::Checking => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Checking GitHub for updates...");
            });
        }
        UpdState::UpToDate => {
            ui.label(RichText::new("✔ You are running the latest version.").color(Color32::from_rgb(0x3B, 0xA5, 0x5D)));
        }
        UpdState::Failed(e) => {
            ui.label(RichText::new(format!("Update failed: {e}")).color(Color32::from_rgb(0xED, 0x42, 0x45)));
        }
        UpdState::Available(rel) => {
            ui.label(RichText::new(format!("⬆ {product} {} is available!", rel.version)).strong().color(Color32::from_rgb(0x3B, 0xA5, 0x5D)));
            if !rel.notes.trim().is_empty() {
                egui::ScrollArea::vertical().max_height(180.0).show(ui, |ui| {
                    ui.label(&rel.notes);
                });
            }
            ui.horizontal(|ui| {
                let b = egui::Button::new(RichText::new("Download and install").color(Color32::WHITE)).fill(Color32::from_rgb(0x25, 0x80, 0xC3));
                if ui.add(b).clicked() {
                    updater.install(rel.clone());
                }
                if ui.button("Release page").clicked() {
                    open_url(&rel.page_url);
                }
            });
            ui.label(RichText::new(format!("{} will close while the update installs and start again afterwards.", product)).small().color(dim));
        }
        UpdState::Downloading(rel, done, total) => {
            let frac = if total > 0 { done as f32 / total as f32 } else { 0.0 };
            ui.label(format!("Downloading {}...", rel.asset_name));
            ui.add(egui::ProgressBar::new(frac).show_percentage());
        }
        UpdState::Installing => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Starting the installer...");
            });
        }
    }
    ui.add_space(6.0);
    if ui.add_enabled(!updater.busy(), egui::Button::new("⟲ Check for updates")).clicked() {
        updater.check();
    }
}
