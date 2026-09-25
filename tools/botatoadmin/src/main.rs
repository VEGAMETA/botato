//! botatoadmin: minimal GUI over the admin library for the everyday task —
//! turn a device's enrollment request into a certificate, or remove a lost device.

#![windows_subsystem = "windows"]

mod release;

use std::path::PathBuf;

use admin::{Admin, DEFAULT_CERT_DAYS, parse_id};
use eframe::egui;
use proto::Id;
use proto::enroll::{self, Request};
use zeroize::Zeroize;

/// An unlocked root key is dropped after this long unused.
const AUTO_LOCK: std::time::Duration = std::time::Duration::from_secs(30 * 60);

/// Admin data directory; override with BOTATO_ADMIN_DIR.
const DEFAULT_DIR: &str = r"C:\Users\VEGAMETA\botato-admin";

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([980.0, 720.0])
            .with_title("botatoadmin"),
        ..Default::default()
    };
    eframe::run_native(
        "botatoadmin",
        options,
        Box::new(|_| Ok(Box::new(App::new()))),
    )
}

#[derive(PartialEq)]
enum Target {
    NewUser,
    Existing(String),
}

struct App {
    dir: PathBuf,
    admin: Result<Admin, String>,
    request_text: String,
    request: Option<Request>,
    request_error: Option<String>,
    target: Target,
    name: String,
    days: u32,
    checked: bool,
    unlock_password: String,
    /// The unlocked root key: in memory only while the app runs, locked again after
    /// [`AUTO_LOCK`] unused or with the button. One password per session instead of one per action.
    root: Option<(proto::SigningKey, std::time::Instant)>,
    status: Option<(bool, String)>,
    cert_text: Option<(String, String)>, // (title, text)
    qr: Option<egui::TextureHandle>,
    /// User whose devices are listed for removal.
    selected: Option<String>,
    release: release::ReleasePanel,
}

impl App {
    fn new() -> Self {
        let dir = std::env::var_os("BOTATO_ADMIN_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_DIR));
        let admin = Admin::open(&dir).map_err(|e| e.to_string());
        App {
            dir,
            admin,
            request_text: String::new(),
            request: None,
            request_error: None,
            target: Target::NewUser,
            name: String::new(),
            days: DEFAULT_CERT_DAYS as u32,
            checked: false,
            unlock_password: String::new(),
            root: None,
            status: None,
            cert_text: None,
            qr: None,
            selected: None,
            release: release::ReleasePanel::new(),
        }
    }

    /// The root key for an action (refreshes the auto-lock timer); `None` = locked.
    fn key(&mut self) -> Option<proto::SigningKey> {
        if self.root.as_ref().is_some_and(|(_, t)| t.elapsed() > AUTO_LOCK) {
            self.root = None;
        }
        let (k, t) = self.root.as_mut()?;
        *t = std::time::Instant::now();
        Some(k.clone())
    }

    fn unlock(&mut self) {
        let result = match &self.admin {
            Ok(a) => a.unlock(self.unlock_password.as_bytes()).map_err(|e| e.to_string()),
            Err(e) => Err(e.clone()),
        };
        self.unlock_password.zeroize();
        self.unlock_password.clear();
        match result {
            Ok(k) => {
                self.root = Some((k, std::time::Instant::now()));
                self.status = None;
            }
            Err(e) => self.status = Some((false, e)),
        }
    }

    fn parse_request(&mut self) {
        self.request = None;
        self.request_error = None;
        self.checked = false;
        let t = self.request_text.trim();
        if t.is_empty() {
            return;
        }
        match enroll::from_text(enroll::REQUEST_PREFIX, t)
            .map_err(|_| "это не запрос botato-req1:".to_string())
            .and_then(|raw| Request::verify(&raw).map_err(|e| format!("запрос повреждён: {e}")))
        {
            Ok(r) => {
                if self.name.is_empty() {
                    self.name = r.name.clone();
                }
                self.request = Some(r);
            }
            Err(e) => self.request_error = Some(e),
        }
    }

    fn issue(&mut self, ctx: &egui::Context) {
        let Some(req) = self.request.clone() else {
            return;
        };
        let Some(root) = self.key() else {
            self.status = Some((false, "Сначала разблокируйте корневой ключ (вверху).".into()));
            return;
        };
        let Ok(admin) = self.admin.as_mut() else {
            return;
        };
        let result = (|| -> Result<(Id, String), String> {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs();
            let (user, name) = match &self.target {
                Target::NewUser => (None, Some(self.name.trim())),
                Target::Existing(u) => (Some(parse_id(u).map_err(|e| e.to_string())?), None),
            };
            let issued = admin
                .enroll(&root, &req, name, user, self.days as u64, now)
                .map_err(|e| e.to_string())?;
            Ok((issued.user_id, issued.text))
        })();
        match result {
            Ok((user, text)) => {
                self.status = Some((
                    true,
                    format!("Готово: пользователь {user}. Отправьте сертификат на устройство."),
                ));
                self.show_cert(ctx, "Новый сертификат".into(), text);
                self.request_text.clear();
                self.request = None;
                self.name.clear();
                self.checked = false;
            }
            Err(e) => self.status = Some((false, e)),
        }
    }

    /// Reissues the owner's cert without the device and signs a manifest revoking it.
    fn remove_device(&mut self, ctx: &egui::Context, device: &str) {
        let Some(root) = self.key() else {
            self.status = Some((false, "Сначала разблокируйте корневой ключ (вверху).".into()));
            return;
        };
        let Ok(admin) = self.admin.as_mut() else {
            return;
        };
        let result = (|| -> Result<(Option<String>, u64), String> {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs();
            let id = parse_id(device).map_err(|e| e.to_string())?;
            let issued = admin
                .remove_device(&root, &id, self.days as u64, now)
                .map_err(|e| e.to_string())?;
            let (version, _) = admin.sign_manifest(&root, now).map_err(|e| e.to_string())?;
            Ok((issued.map(|i| i.text), version))
        })();
        match result {
            Ok((cert, version)) => {
                self.status = Some((
                    true,
                    format!(
                        "Устройство {} удалено, манифест v{version} подписан и выкатывается на сервер (ниже).",
                        &device[..8]
                    ),
                ));
                // The relay drops a revoked device only once it has the new manifest.
                self.release.publish(ctx, "manifest", admin.nostr_job(release::now_secs()).ok());
                match cert {
                    Some(text) => self.show_cert(ctx, "Перевыпущенный сертификат".into(), text),
                    None => {
                        self.cert_text = None;
                        self.qr = None;
                    }
                }
            }
            Err(e) => self.status = Some((false, e)),
        }
    }

    fn show_cert(&mut self, ctx: &egui::Context, title: String, text: String) {
        self.qr = qr_texture(ctx, &text);
        self.cert_text = Some((title, text));
    }
}

fn qr_texture(ctx: &egui::Context, text: &str) -> Option<egui::TextureHandle> {
    let code = qrcode::QrCode::new(text.as_bytes()).ok()?;
    let n = code.width();
    let (scale, quiet) = (4usize, 4usize);
    let size = (n + 2 * quiet) * scale;
    let colors = code.to_colors();
    let mut img = egui::ColorImage::new([size, size], vec![egui::Color32::WHITE; size * size]);
    for y in 0..n {
        for x in 0..n {
            if colors[y * n + x] == qrcode::Color::Dark {
                for dy in 0..scale {
                    for dx in 0..scale {
                        let px = (x + quiet) * scale + dx;
                        let py = (y + quiet) * scale + dy;
                        img.pixels[py * size + px] = egui::Color32::BLACK;
                    }
                }
            }
        }
    }
    Some(ctx.load_texture("qr", img, egui::TextureOptions::NEAREST))
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let users: Vec<(String, String, Vec<String>)> = match &self.admin {
            Ok(a) => a
                .state
                .users
                .iter()
                .filter(|u| !u.revoked)
                .map(|u| {
                    (
                        u.user_id.clone(),
                        u.name.clone(),
                        u.devices.iter().map(|d| d.device_id.clone()).collect(),
                    )
                })
                .collect(),
            Err(_) => vec![],
        };

        egui::Panel::left("users")
            .resizable(true)
            .default_size(280.0)
            .show(ui, |ui| {
                ui.heading("Пользователи");
                ui.label(
                    egui::RichText::new(self.dir.display().to_string())
                        .small()
                        .weak(),
                );
                ui.separator();
                if let Err(e) = &self.admin {
                    ui.colored_label(
                        egui::Color32::LIGHT_RED,
                        format!("Папка админа не открылась:\n{e}"),
                    );
                    return;
                }
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for (id, name, devices) in &users {
                        let resp = ui.add(
                            egui::Button::new(egui::RichText::new(name).strong()).frame(false),
                        );
                        ui.label(
                            egui::RichText::new(format!(
                                "устройств: {} · {}",
                                devices.len(),
                                &id[..8]
                            ))
                            .small()
                            .weak(),
                        );
                        if resp.clicked()
                            && let Ok(a) = &self.admin
                        {
                            self.selected = Some(id.clone());
                            match parse_id(id).and_then(|uid| a.cert_text(&uid)) {
                                Ok(text) => {
                                    self.show_cert(&ctx, format!("Сертификат: {name}"), text)
                                }
                                Err(e) => self.status = Some((false, e.to_string())),
                            }
                        }
                        ui.add_space(6.0);
                    }
                });
            });

        if self.root.as_ref().is_some_and(|(_, t)| t.elapsed() > AUTO_LOCK) {
            self.root = None;
        }
        ctx.request_repaint_after(std::time::Duration::from_secs(60)); // the auto-lock shows up
        let unlocked = self.root.is_some();

        egui::CentralPanel::default().show(ui, |ui| {
            ui.horizontal(|ui| {
                if unlocked {
                    ui.colored_label(egui::Color32::LIGHT_GREEN, "Корневой ключ разблокирован");
                    if ui.button("Заблокировать").clicked() {
                        self.root = None;
                    }
                } else {
                    ui.label("Пароль корневого ключа:");
                    let r = ui.add(
                        egui::TextEdit::singleline(&mut self.unlock_password)
                            .password(true)
                            .desired_width(220.0),
                    );
                    let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if (ui
                        .add_enabled(!self.unlock_password.is_empty(), egui::Button::new("Разблокировать"))
                        .clicked()
                        || enter)
                        && !self.unlock_password.is_empty()
                    {
                        self.unlock();
                    }
                }
            });
            ui.separator();
            egui::ScrollArea::vertical().show(ui, |ui| {
                if let Some((_, name, devices)) = self
                    .selected
                    .as_ref()
                    .and_then(|s| users.iter().find(|u| &u.0 == s))
                    .cloned()
                {
                    ui.horizontal(|ui| {
                        ui.heading(format!("Устройства «{name}»"));
                        if ui.button("Скрыть").clicked() {
                            self.selected = None;
                        }
                    });
                    let mut remove = None;
                    for d in &devices {
                        ui.horizontal(|ui| {
                            ui.monospace(d);
                            if ui
                                .add_enabled(
                                    unlocked,
                                    egui::Button::new("Удалить"),
                                )
                                .clicked()
                            {
                                remove = Some(d.clone());
                            }
                        });
                    }
                    ui.label(
                        egui::RichText::new(
                            "Удаление перевыпускает сертификат и отзывает устройство в манифесте.",
                        )
                        .small()
                        .weak(),
                    );
                    if let Some(d) = remove {
                        self.remove_device(&ctx, &d);
                    }
                    ui.separator();
                }
                ui.heading("Подключить устройство");
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label("1. Запрос с устройства:");
                    if ui.button("Вставить из буфера").clicked()
                        && let Ok(t) = arboard::Clipboard::new().and_then(|mut c| c.get_text())
                    {
                        self.request_text = t;
                        self.parse_request();
                    }
                });
                let edit = ui.add(
                    egui::TextEdit::multiline(&mut self.request_text)
                        .desired_rows(3)
                        .desired_width(f32::INFINITY)
                        .hint_text("botato-req1:…"),
                );
                if edit.changed() {
                    self.parse_request();
                }
                if let Some(e) = &self.request_error {
                    ui.colored_label(egui::Color32::LIGHT_RED, e);
                }

                if let Some(req) = self.request.clone() {
                    ui.add_space(10.0);
                    ui.label(format!(
                        "Устройство {}   имя в запросе: {:?}",
                        req.device_id, req.name
                    ));
                    ui.label("2. Сверьте отпечаток с человеком голосом или лично:");
                    ui.label(
                        egui::RichText::new(req.fingerprint())
                            .monospace()
                            .size(28.0)
                            .color(egui::Color32::from_rgb(0x2E, 0xB3, 0xDC)),
                    );
                    ui.checkbox(&mut self.checked, "Отпечаток совпадает");

                    ui.add_space(10.0);
                    ui.label("3. Кому:");
                    ui.radio_value(&mut self.target, Target::NewUser, "Новый пользователь");
                    if self.target == Target::NewUser {
                        ui.horizontal(|ui| {
                            ui.label("Имя:");
                            ui.text_edit_singleline(&mut self.name);
                        });
                    }
                    for (id, name, _) in &users {
                        ui.radio_value(
                            &mut self.target,
                            Target::Existing(id.clone()),
                            format!("Ещё одно устройство для «{name}»"),
                        );
                    }
                    ui.horizontal(|ui| {
                        ui.label("Срок, дней:");
                        ui.add(egui::DragValue::new(&mut self.days).range(1..=3650));
                    });

                    ui.add_space(10.0);
                    let ready = self.checked
                        && unlocked
                        && (self.target != Target::NewUser || !self.name.trim().is_empty());
                    if ui
                        .add_enabled(ready, egui::Button::new("Выпустить сертификат"))
                        .clicked()
                    {
                        self.issue(&ctx);
                    }
                }

                if let Some((ok, msg)) = &self.status {
                    ui.add_space(8.0);
                    ui.colored_label(
                        if *ok {
                            egui::Color32::LIGHT_GREEN
                        } else {
                            egui::Color32::LIGHT_RED
                        },
                        msg,
                    );
                }

                ui.add_space(16.0);
                ui.separator();
                egui::CollapsingHeader::new(egui::RichText::new("Релиз и выкатка").heading())
                    .default_open(false)
                    .show(ui, |ui| {
                        let root = self.root.as_ref().map(|(k, _)| k.clone());
                        if let Ok(admin) = self.admin.as_mut() {
                            self.release.ui(ui, admin, root.as_ref());
                        }
                    });

                if let Some((title, text)) = self.cert_text.clone() {
                    ui.separator();
                    ui.horizontal(|ui| {
                        ui.heading(title);
                        if ui.button("Скопировать").clicked() {
                            ctx.copy_text(text.clone());
                        }
                    });
                    let mut shown = text.clone();
                    ui.add(
                        egui::TextEdit::multiline(&mut shown)
                            .desired_rows(4)
                            .desired_width(f32::INFINITY)
                            .font(egui::TextStyle::Monospace),
                    );
                    if let Some(qr) = &self.qr {
                        ui.image((qr.id(), qr.size_vec2()));
                    }
                }
            });
        });
    }
}
