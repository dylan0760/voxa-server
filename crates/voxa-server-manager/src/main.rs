//! Voxa Server Manager: a small window plus system tray icon to monitor and control
//! the Voxa Server Windows service. Closing the window keeps it in the tray; the
//! server itself always keeps running as a service.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod gpu;
mod icon;
mod instance;
mod service;
mod updater;

use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use egui::{Align, Align2, Color32, CornerRadius, FontId, Frame, Layout, Margin, RichText, Sense, Vec2};
use service::Svc;
use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

/// GitHub repository the server updates from (override with VOXA_UPDATE_REPO at build time).
const UPDATE_REPO: &str = match option_env!("VOXA_UPDATE_REPO") {
    Some(r) => r,
    None => "dylan0760/voxa-server",
};

fn is_server_msi(name: &str) -> bool {
    name.starts_with("Voxa-Server-") && name.ends_with("-x64.msi")
}

const GREEN: Color32 = Color32::from_rgb(0x3B, 0xA5, 0x5D);
const RED: Color32 = Color32::from_rgb(0xED, 0x42, 0x45);
const YELLOW: Color32 = Color32::from_rgb(0xFA, 0xA6, 0x1A);
const ACCENT: Color32 = Color32::from_rgb(0x28, 0xA0, 0x5F);
const BG: Color32 = Color32::from_rgb(0x1F, 0x20, 0x25);
const CARD: Color32 = Color32::from_rgb(0x27, 0x29, 0x30);
const DIM: Color32 = Color32::from_rgb(0x9A, 0xA0, 0xA8);

#[derive(serde::Deserialize, Default, Clone, PartialEq)]
#[serde(default)]
struct Status {
    version: String,
    port: u16,
    online: usize,
    channels: usize,
    started: u64,
    updated: u64,
    fingerprint: String,
}

enum RawTray {
    Menu(MenuId),
    Activate,
}

struct TrayMenu {
    icon: TrayIcon,
    open: MenuItem,
    start: MenuItem,
    stop: MenuItem,
    restart: MenuItem,
    folder: MenuItem,
    update: MenuItem,
    exit: MenuItem,
    last: Option<(Svc, usize)>,
}

/// Per-user state of the manager (last version whose change log was shown).
fn state_dir() -> PathBuf {
    let base = std::env::var("APPDATA").unwrap_or_else(|_| ".".into());
    PathBuf::from(base).join("Voxa Server Manager")
}

/// Shows the change log once after the manager / server was updated to a new version.
fn whats_new_on_start(ctx: &egui::Context) -> Option<updater::WhatsNew> {
    let file = state_dir().join("last_seen_version.txt");
    let current = voxa_proto::APP_VERSION;
    let last = std::fs::read_to_string(&file).ok().map(|s| s.trim().to_string());
    let _ = std::fs::create_dir_all(state_dir());
    let _ = std::fs::write(&file, current);
    let updated = match last {
        Some(v) => v != current,
        // No record yet: only treat it as an update if the server existed before
        // (its database is older than a few minutes), not on a fresh install.
        None => std::fs::metadata(data_dir().join("voxa.db"))
            .and_then(|m| m.created())
            .map(|t| t.elapsed().map(|e| e.as_secs() > 600).unwrap_or(false))
            .unwrap_or(false),
    };
    updated.then(|| updater::WhatsNew::fetch(UPDATE_REPO, current, Some(state_dir()), ctx.clone()))
}

fn data_dir() -> PathBuf {
    let base = std::env::var("ProgramData").unwrap_or_else(|_| "C:\\ProgramData".into());
    PathBuf::from(base).join("Voxa Server")
}

fn now_unix() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn fmt_duration(secs: u64) -> String {
    let (d, h, m) = (secs / 86400, (secs % 86400) / 3600, (secs % 3600) / 60);
    if d > 0 {
        format!("{d}d {h}h {m}m")
    } else if h > 0 {
        format!("{h}h {m}m")
    } else {
        format!("{m}m {}s", secs % 60)
    }
}

fn read_tail(path: &PathBuf, max: u64) -> Vec<String> {
    let Ok(mut f) = std::fs::File::open(path) else { return Vec::new() };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let _ = f.seek(SeekFrom::Start(len.saturating_sub(max)));
    let mut s = String::new();
    let mut buf = Vec::new();
    let _ = f.read_to_end(&mut buf);
    s.push_str(&String::from_utf8_lossy(&buf));
    let mut lines: Vec<String> = s.lines().map(|l| l.to_string()).collect();
    if len > max && !lines.is_empty() {
        lines.remove(0); // partial first line
    }
    lines
}

fn read_privilege_key(dir: &PathBuf) -> Option<String> {
    let text = std::fs::read_to_string(dir.join("privilege_key.txt")).ok()?;
    text.lines().map(str::trim).find(|l| l.len() >= 20 && !l.contains(' ')).map(|s| s.to_string())
}

/// What the background poller found (service state, status file, key, log tail).
#[derive(Clone, PartialEq)]
struct Polled {
    svc: Svc,
    status: Option<Status>,
    key: Option<String>,
    log: Arc<Vec<String>>,
}

/// Polls the service and the data folder every 2 s on its own thread and asks for a
/// repaint only when something changed, so the manager costs nothing while idle (no
/// periodic redraws, which are expensive on machines without a GPU).
fn start_poller(dir: PathBuf, ctx: egui::Context) -> Arc<Mutex<(u64, Polled)>> {
    let empty = Polled { svc: Svc::Unknown, status: None, key: None, log: Arc::new(Vec::new()) };
    let shared = Arc::new(Mutex::new((0u64, empty)));
    let out = shared.clone();
    std::thread::spawn(move || {
        let log_path = dir.join("logs").join("server.log");
        let mut log_sig = None;
        loop {
            let svc = service::query();
            let status = std::fs::read_to_string(dir.join("status.json"))
                .ok()
                .and_then(|t| serde_json::from_str::<Status>(&t).ok())
                // The server rewrites the file at least once a minute while running.
                .filter(|s| now_unix().saturating_sub(s.updated) < 150 && svc == Svc::Running)
                .map(|s| Status { updated: 0, ..s });
            let key = read_privilege_key(&dir);
            let sig = std::fs::metadata(&log_path).ok().map(|m| (m.len(), m.modified().ok()));
            let mut guard = shared.lock().unwrap();
            let (generation, cur) = &mut *guard;
            let log = if sig != log_sig || cur.log.is_empty() {
                log_sig = sig;
                Arc::new(read_tail(&log_path, 48 * 1024))
            } else {
                cur.log.clone()
            };
            let next = Polled { svc, status, key, log };
            if next != *cur {
                *cur = next;
                *generation += 1;
                drop(guard);
                ctx.request_repaint();
            } else {
                drop(guard);
            }
            std::thread::sleep(Duration::from_secs(2));
        }
    });
    out
}

struct ManagerApp {
    dir: PathBuf,
    svc: Svc,
    status: Option<Status>,
    key: Option<String>,
    log: Arc<Vec<String>>,
    polled: Arc<Mutex<(u64, Polled)>>,
    seen_generation: u64,
    /// True while the window is hidden in the tray (no timed repaints then).
    hidden: bool,
    busy: Arc<Mutex<Option<String>>>,
    message: Arc<Mutex<Option<(String, bool)>>>,
    tray: Option<TrayMenu>,
    tray_rx: Receiver<RawTray>,
    updater: updater::Updater,
    whats_new: Option<updater::WhatsNew>,
    quitting: bool,
    ctx: egui::Context,
}

impl ManagerApp {
    fn new(cc: &eframe::CreationContext<'_>, inst: Option<instance::Instance>) -> ManagerApp {
        let ctx = cc.egui_ctx.clone();
        // Windows symbol/emoji fonts as fallbacks so icons render.
        let mut fonts = egui::FontDefinitions::default();
        let windir = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
        for (name, file) in [("segoe_symbol", "seguisym.ttf"), ("segoe_emoji", "seguiemj.ttf")] {
            if let Ok(bytes) = std::fs::read(std::path::Path::new(&windir).join("Fonts").join(file)) {
                fonts.font_data.insert(name.to_owned(), Arc::new(egui::FontData::from_owned(bytes)));
                for fam in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
                    if let Some(list) = fonts.families.get_mut(&fam) {
                        list.push(name.to_owned());
                    }
                }
            }
        }
        ctx.set_fonts(fonts);
        let mut v = egui::Visuals::dark();
        v.panel_fill = BG;
        v.window_fill = CARD;
        v.selection.bg_fill = ACCENT;
        for w in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.noninteractive] {
            w.corner_radius = CornerRadius::same(6);
        }
        v.widgets.inactive.weak_bg_fill = CARD;
        ctx.set_visuals(v);
        ctx.global_style_mut(|s| {
            s.spacing.item_spacing = egui::vec2(8.0, 6.0);
            s.spacing.button_padding = egui::vec2(12.0, 6.0);
        });

        let (tx, rx): (Sender<RawTray>, Receiver<RawTray>) = std::sync::mpsc::channel();
        install_tray_handlers(tx.clone(), ctx.clone());
        if let Some(inst) = inst {
            let c = ctx.clone();
            let tx = Mutex::new(tx);
            instance::listen(inst, move || {
                let _ = tx.lock().unwrap().send(RawTray::Activate);
                c.request_repaint();
            });
        }
        let tray = build_tray();
        ManagerApp {
            dir: data_dir(),
            svc: Svc::Unknown,
            status: None,
            key: None,
            log: Arc::new(Vec::new()),
            polled: start_poller(data_dir(), ctx.clone()),
            seen_generation: 0,
            hidden: std::env::args().any(|a| a == "--minimized"),
            busy: Arc::new(Mutex::new(None)),
            message: Arc::new(Mutex::new(None)),
            tray,
            tray_rx: rx,
            updater: {
                let mut u = updater::Updater::new(UPDATE_REPO, is_server_msi, cc.egui_ctx.clone()).with_notes_dir(state_dir());
                u.check();
                u
            },
            whats_new: whats_new_on_start(&cc.egui_ctx),
            quitting: false,
            ctx,
        }
    }

    fn poll(&mut self) {
        let guard = self.polled.lock().unwrap();
        if guard.0 == self.seen_generation {
            return;
        }
        self.seen_generation = guard.0;
        let p = guard.1.clone();
        drop(guard);
        self.svc = p.svc;
        self.status = p.status;
        self.key = p.key;
        self.log = p.log;
    }

    fn run_op(&self, label: &str, f: fn() -> Result<(), String>) {
        let busy = self.busy.clone();
        let msg = self.message.clone();
        let ctx = self.ctx.clone();
        *busy.lock().unwrap() = Some(label.to_string());
        std::thread::spawn(move || {
            let r = f();
            *busy.lock().unwrap() = None;
            *msg.lock().unwrap() = Some(match r {
                Ok(()) => ("Done.".into(), false),
                Err(e) => (e, true),
            });
            ctx.request_repaint();
        });
    }

    fn show(&mut self) {
        self.hidden = false;
        self.ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        self.ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        self.ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    }

    fn open_path(p: PathBuf, editor: bool) {
        let prog = if editor { "notepad.exe" } else { "explorer.exe" };
        let _ = std::process::Command::new(prog).arg(p).spawn();
    }

    fn handle_tray(&mut self, ctx: &egui::Context) {
        while let Ok(raw) = self.tray_rx.try_recv() {
            let Some(t) = &self.tray else {
                if matches!(raw, RawTray::Activate) {
                    self.show();
                }
                continue;
            };
            match raw {
                RawTray::Activate => self.show(),
                RawTray::Menu(id) if &id == t.open.id() => self.show(),
                RawTray::Menu(id) if &id == t.start.id() => self.run_op("Starting the server...", service::start),
                RawTray::Menu(id) if &id == t.stop.id() => self.run_op("Stopping the server...", service::stop),
                RawTray::Menu(id) if &id == t.restart.id() => self.run_op("Restarting the server...", service::restart),
                RawTray::Menu(id) if &id == t.folder.id() => Self::open_path(self.dir.clone(), false),
                RawTray::Menu(id) if &id == t.update.id() => {
                    self.show();
                    self.updater.check();
                }
                RawTray::Menu(id) if &id == t.exit.id() => {
                    self.quitting = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                RawTray::Menu(_) => {}
            }
        }
        if let Some(t) = &mut self.tray {
            let online = self.status.as_ref().map(|s| s.online).unwrap_or(0);
            let state = (self.svc, online);
            if t.last != Some(state) {
                t.last = Some(state);
                t.start.set_enabled(matches!(self.svc, Svc::Stopped));
                t.stop.set_enabled(matches!(self.svc, Svc::Running));
                t.restart.set_enabled(matches!(self.svc, Svc::Running | Svc::Stopped));
                let tip = match self.svc {
                    Svc::Running => format!("Voxa Server - Running ({online} online)"),
                    s => format!("Voxa Server - {}", s.label()),
                };
                let _ = t.icon.set_tooltip(Some(tip));
            }
        }
    }
}

fn install_tray_handlers(tx: Sender<RawTray>, ctx: egui::Context) {
    let tx_icon = Mutex::new(tx.clone());
    let ctx_icon = ctx.clone();
    TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
        let activate = matches!(
            e,
            TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. }
                | TrayIconEvent::DoubleClick { button: MouseButton::Left, .. }
        );
        if activate {
            let _ = tx_icon.lock().unwrap().send(RawTray::Activate);
            ctx_icon.request_repaint();
        }
    }));
    let tx_menu = Mutex::new(tx);
    MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
        let _ = tx_menu.lock().unwrap().send(RawTray::Menu(e.id));
        ctx.request_repaint();
    }));
}

fn build_tray() -> Option<TrayMenu> {
    let open = MenuItem::new("Open Voxa Server Manager", true, None);
    let start = MenuItem::new("Start Server", false, None);
    let stop = MenuItem::new("Stop Server", false, None);
    let restart = MenuItem::new("Restart Server", false, None);
    let folder = MenuItem::new("Open Data Folder", true, None);
    let update = MenuItem::new("Check for Updates", true, None);
    let exit = MenuItem::new("Exit Manager (server keeps running)", true, None);
    let menu = Menu::new();
    menu.append_items(&[
        &open,
        &PredefinedMenuItem::separator(),
        &start,
        &stop,
        &restart,
        &PredefinedMenuItem::separator(),
        &folder,
        &update,
        &PredefinedMenuItem::separator(),
        &exit,
    ])
    .ok()?;
    let icon = TrayIconBuilder::new()
        .with_tooltip("Voxa Server")
        .with_icon(Icon::from_rgba(icon::icon_rgba(32, true), 32, 32).ok()?)
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .build()
        .ok()?;
    Some(TrayMenu { icon, open, start, stop, restart, folder, update, exit, last: None })
}

fn card(ui: &mut egui::Ui, title: &str, add: impl FnOnce(&mut egui::Ui)) {
    Frame::new().fill(CARD).corner_radius(10).inner_margin(Margin::same(12)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(RichText::new(title.to_uppercase()).small().strong().color(DIM));
        ui.add_space(4.0);
        add(ui);
    });
    ui.add_space(8.0);
}

impl eframe::App for ManagerApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll();
        self.updater.auto_check();
        if matches!(self.updater.get(), updater::UpdState::Installing) && !self.quitting {
            // The installer upgrades the service and reopens the manager afterwards.
            self.quitting = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        self.handle_tray(ctx);
        // The close button (X) sends the manager to the system tray.
        if ctx.input(|i| i.viewport().close_requested()) && !self.quitting && self.tray.is_some() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            self.hidden = true;
        }
        // While the window is open, refresh the uptime now and then. Hidden: no timer at all.
        if !self.hidden {
            ctx.request_repaint_after(Duration::from_secs(5));
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if let Some(wn) = &self.whats_new {
            if !updater::whats_new_window(ui.ctx(), wn, "Voxa Server", updater::render_notes) {
                self.whats_new = None;
            }
        }
        let busy = self.busy.lock().unwrap().clone();
        egui::CentralPanel::default().frame(Frame::new().fill(BG).inner_margin(Margin::same(16))).show(ui, |ui| {
            // Header.
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(Vec2::splat(48.0), Sense::hover());
                ui.painter().rect_filled(r, CornerRadius::same(12), ACCENT);
                ui.painter().text(r.center(), Align2::CENTER_CENTER, "🖧", FontId::proportional(24.0), Color32::WHITE);
                ui.vertical(|ui| {
                    ui.label(RichText::new("Voxa Server").size(20.0).strong());
                    let (c, txt) = match self.svc {
                        Svc::Running => (GREEN, "● Running"),
                        Svc::Stopped => (RED, "● Stopped"),
                        Svc::NotInstalled => (RED, "● Service not installed"),
                        Svc::Starting | Svc::Stopping => (YELLOW, "● Changing state..."),
                        Svc::Unknown => (YELLOW, "● Unknown"),
                    };
                    ui.label(RichText::new(txt).color(c));
                    if let Some(rel) = self.updater.available() {
                        ui.label(RichText::new(format!("⬆ Update {} available - see Updates below", rel.version)).small().color(GREEN));
                    }
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let idle = busy.is_none();
                    if ui.add_enabled(idle && matches!(self.svc, Svc::Running | Svc::Stopped), egui::Button::new("⟲ Restart")).clicked() {
                        self.run_op("Restarting the server...", service::restart);
                    }
                    if ui.add_enabled(idle && self.svc == Svc::Running, egui::Button::new("■ Stop")).clicked() {
                        self.run_op("Stopping the server...", service::stop);
                    }
                    let start = egui::Button::new(RichText::new("▶ Start").color(Color32::WHITE)).fill(ACCENT);
                    if ui.add_enabled(idle && self.svc == Svc::Stopped, start).clicked() {
                        self.run_op("Starting the server...", service::start);
                    }
                });
            });
            if let Some(b) = &busy {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(b);
                });
            } else if let Some((m, err)) = self.message.lock().unwrap().clone() {
                ui.label(RichText::new(m).color(if err { RED } else { DIM }));
            }
            ui.add_space(10.0);

            card(ui, "Server", |ui| {
                egui::Grid::new("st").num_columns(2).spacing([16.0, 4.0]).show(ui, |ui| {
                    let row = |ui: &mut egui::Ui, k: &str, v: String| {
                        ui.label(RichText::new(k).color(DIM));
                        ui.label(v);
                        ui.end_row();
                    };
                    match &self.status {
                        Some(s) => {
                            row(ui, "Clients online", s.online.to_string());
                            row(ui, "Channels", s.channels.to_string());
                            row(ui, "Uptime", fmt_duration(now_unix().saturating_sub(s.started)));
                            row(ui, "Ports", format!("TCP {0} and UDP {0}", s.port));
                            row(ui, "Version", s.version.clone());
                            ui.label(RichText::new("Fingerprint").color(DIM));
                            ui.label(RichText::new(&s.fingerprint).monospace().small());
                            ui.end_row();
                        }
                        None => {
                            row(ui, "Status", if self.svc == Svc::Running { "Starting up...".into() } else { "Server is not running".into() });
                            row(ui, "Ports", "TCP 9987 and UDP 9987 (default)".into());
                        }
                    }
                });
            });

            card(ui, "Updates", |ui| {
                updater::ui(&mut self.updater, ui, "Voxa Server");
                if ui.button("🎉 What's new in this version").clicked() {
                    self.whats_new = Some(updater::WhatsNew::fetch(UPDATE_REPO, voxa_proto::APP_VERSION, Some(state_dir()), self.ctx.clone()));
                }
            });

            card(ui, "Server Admin privilege key", |ui| match &self.key {
                Some(k) => {
                    ui.horizontal(|ui| {
                        ui.add(egui::Label::new(RichText::new(k).monospace().strong()).selectable(true));
                        if ui.small_button("📋 Copy").clicked() {
                            ui.ctx().copy_text(k.clone());
                        }
                    });
                    ui.label(RichText::new("In the Voxa client click the server name ▸ Use Privilege Key and paste this key to become Server Admin.").small().color(DIM));
                }
                None => {
                    ui.label(RichText::new("No unused admin key - an administrator has already claimed this server.").color(DIM));
                }
            });

            ui.horizontal(|ui| {
                if ui.button("📁 Open Data Folder").clicked() {
                    Self::open_path(self.dir.clone(), false);
                }
                if ui.button("⚙ Edit Configuration").on_hover_text("server.toml - restart the server after editing").clicked() {
                    Self::open_path(self.dir.join("server.toml"), true);
                }
                if ui.button("📄 Open Log File").clicked() {
                    Self::open_path(self.dir.join("logs").join("server.log"), true);
                }
            });
            ui.add_space(8.0);
            ui.label(RichText::new("LOG").small().strong().color(DIM));
            let footer_h = 22.0;
            Frame::new().fill(Color32::from_rgb(0x15, 0x16, 0x1A)).corner_radius(8).inner_margin(Margin::same(8)).show(ui, |ui| {
                egui::ScrollArea::both()
                    .max_height((ui.available_height() - footer_h - 12.0).max(60.0))
                    .auto_shrink([false, false])
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        for l in self.log.iter().rev().take(300).rev() {
                            let c = if l.contains(" WARN ") {
                                YELLOW
                            } else if l.contains(" ERROR ") {
                                RED
                            } else {
                                Color32::from_rgb(0xC8, 0xCC, 0xD2)
                            };
                            ui.label(RichText::new(l).monospace().size(11.0).color(c));
                        }
                    });
            });
            ui.label(
                RichText::new("Closing this window keeps the manager in the system tray. The server runs as a Windows service and keeps running either way.")
                    .small()
                    .color(DIM),
            );
        });
    }
}

fn main() -> eframe::Result<()> {
    let Some(inst) = instance::acquire("VoxaServerManagerInstance") else {
        return Ok(());
    };
    let minimized = std::env::args().any(|a| a == "--minimized");
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Voxa Server Manager")
            .with_app_id("voxa-server-manager")
            .with_inner_size([660.0, 780.0])
            .with_min_inner_size([480.0, 420.0])
            .with_visible(!minimized)
            .with_icon(egui::IconData { rgba: icon::icon_rgba(64, true), width: 64, height: 64 }),
        wgpu_options: gpu::wgpu_options(),
        ..Default::default()
    };
    eframe::run_native("Voxa Server Manager", options, Box::new(move |cc| Ok(Box::new(ManagerApp::new(cc, Some(inst))))))
}
