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
    /// All known releases (newest first) and the one being shown.
    Ready { releases: Vec<update::ReleaseNote>, selected: usize },
    Failed(String),
}

/// "What's new": the change log of this version and of the older ones, fetched from
/// GitHub, falling back to the notes saved when the update was installed.
pub struct WhatsNew {
    pub version: String,
    pub state: Arc<Mutex<NotesState>>,
}

impl WhatsNew {
    pub fn fetch(repo: &'static str, version: &str, notes_dir: Option<PathBuf>, ctx: egui::Context) -> WhatsNew {
        let state = Arc::new(Mutex::new(NotesState::Loading));
        let (st, v) = (state.clone(), version.to_string());
        std::thread::spawn(move || {
            let saved = || {
                notes_dir.as_deref().and_then(|d| load_pending_notes(d, &v)).map(|(title, notes, url)| update::ReleaseNote {
                    version: v.clone(),
                    title,
                    notes,
                    url,
                    date: String::new(),
                })
            };
            let r = match update::release_history(repo, 40) {
                Ok(mut releases) => {
                    // A version that is not published (yet) still shows its saved notes.
                    if !releases.iter().any(|r| r.version == v) {
                        if let Some(n) = saved() {
                            releases.insert(0, n);
                        }
                    }
                    if releases.is_empty() {
                        NotesState::Failed("No releases have been published yet".into())
                    } else {
                        let selected = releases.iter().position(|r| r.version == v).unwrap_or(0);
                        NotesState::Ready { releases, selected }
                    }
                }
                Err(e) => match saved() {
                    Some(n) => NotesState::Ready { releases: vec![n], selected: 0 },
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
#[allow(dead_code)] // the client renders notes with its own Markdown renderer
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

/// The "What's new" window: versions on the left (newest first, the installed one
/// marked), the selected version's change log on the right. Movable and resizable like
/// the other windows. Returns false once the user closed it.
/// The `render` function draws the release notes (Markdown).
pub fn whats_new_window(ctx: &egui::Context, wn: &WhatsNew, product: &str, render: fn(&mut egui::Ui, &str)) -> bool {
    use egui::RichText;
    let mut open = true;
    let mut close = false;
    let center = ctx.content_rect().center();
    egui::Window::new(format!("🎉 What's new in {product}"))
        .id(egui::Id::unique("voxa_whats_new"))
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .min_size([420.0, 260.0])
        .default_size([720.0, 480.0])
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(center)
        .show(ctx, |ui| {
            // The body gets exactly the height that is left after the footer; the footer
            // (and spacing) is measured every frame rather than guessed, since a guess
            // that is too small makes a window grow by itself.
            let rest_id = egui::Id::unique("voxa_whats_new_rest");
            let rest = ui.data(|d| d.get_temp::<f32>(rest_id)).unwrap_or(48.0);
            let body_h = (ui.available_height() - rest).max(160.0);
            let body_w = ui.available_width();
            let mut state = wn.state.lock().unwrap();
            let body = ui.allocate_ui_with_layout(egui::vec2(body_w, body_h), egui::Layout::left_to_right(egui::Align::Min), |ui| {
                ui.set_min_size(egui::vec2(body_w, body_h));
                match &mut *state {
                    NotesState::Loading => {
                        ui.spinner();
                        ui.label("Loading the change log...");
                    }
                    NotesState::Failed(e) => {
                        ui.vertical(|ui| {
                            ui.label(format!("{product} {} is installed.", wn.version));
                            ui.label(RichText::new(format!("The change log could not be loaded: {e}")).weak());
                        });
                    }
                    NotesState::Ready { releases, selected } => {
                        // Version list ("tabs").
                        ui.vertical(|ui| {
                            ui.set_width(150.0);
                            ui.label(RichText::new("VERSIONS").small().strong().weak());
                            egui::ScrollArea::vertical().id_salt("wn_versions").max_height(ui.available_height()).auto_shrink([false, false]).show(ui, |ui| {
                                for (i, r) in releases.iter().enumerate() {
                                    let installed = r.version == wn.version;
                                    let mut text = RichText::new(format!("{}{}", r.version, if installed { "  · installed" } else { "" }));
                                    if installed {
                                        text = text.strong();
                                    }
                                    let resp = ui.selectable_label(*selected == i, text);
                                    let resp = if r.date.is_empty() { resp } else { resp.on_hover_text(format!("Released {}", r.date)) };
                                    if resp.clicked() {
                                        *selected = i;
                                    }
                                }
                            });
                        });
                        ui.separator();
                        ui.vertical(|ui| {
                            let r = &releases[(*selected).min(releases.len() - 1)];
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(&r.title).heading());
                                if !r.date.is_empty() {
                                    ui.label(RichText::new(&r.date).small().weak());
                                }
                            });
                            ui.separator();
                            egui::ScrollArea::vertical()
                                .id_salt(("wn_notes", r.version.as_str()))
                                .max_height(ui.available_height())
                                .auto_shrink([false, false])
                                .show(ui, |ui| render(ui, &r.notes));
                        });
                    }
                }
            });
            let body_bottom = body.response.rect.top() + body_h;
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("Got it").clicked() {
                    close = true;
                }
                if let NotesState::Ready { releases, selected } = &*state {
                    let url = &releases[(*selected).min(releases.len() - 1)].url;
                    if !url.is_empty() && ui.link("View this release on GitHub").clicked() {
                        open_url(url);
                    }
                }
            });
            let used = (ui.cursor().top() - body_bottom).max(0.0);
            ui.data_mut(|d| d.insert_temp(rest_id, used));
        });    open && !close
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
