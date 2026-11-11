//! Background update checks / installs against GitHub Releases.

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
}

impl Updater {
    pub fn new(repo: &'static str, matcher: fn(&str) -> bool, ctx: egui::Context) -> Updater {
        Updater { repo, matcher, state: Arc::new(Mutex::new(UpdState::Idle)), last_check: None, ctx }
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
