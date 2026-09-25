//! "Релиз и выкатка": sign the built app release and publish it, or just publish the manifest.
//! Publishing runs the Ansible playbook in WSL (the same one a human would run by hand).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use admin::Admin;
use eframe::egui;
use zeroize::Zeroize;

/// The repository (build outputs, Ansible). Override with BOTATO_REPO.
const DEFAULT_REPO: &str = r"H:\Desktop\botato";
/// WSL distribution that has Ansible and the SSH keys.
const WSL_DISTRO: &str = "Debian";

#[derive(Default)]
struct Deploy {
    running: bool,
    log: String,
}

pub struct ReleasePanel {
    repo: PathBuf,
    password: String,
    deploy: Arc<Mutex<Deploy>>,
    status: Option<(bool, String)>,
}

struct Artifacts {
    version: String,
    build: u64,
    apk: PathBuf,
    exe: PathBuf,
}

impl ReleasePanel {
    pub fn new() -> Self {
        let repo = std::env::var_os("BOTATO_REPO")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_REPO));
        ReleasePanel { repo, password: String::new(), deploy: Arc::default(), status: None }
    }

    fn artifacts(&self) -> Result<Artifacts, String> {
        let app = self.repo.join("app");
        let pubspec = std::fs::read_to_string(app.join("pubspec.yaml")).map_err(|e| format!("pubspec.yaml: {e}"))?;
        let v = pubspec
            .lines()
            .find_map(|l| l.strip_prefix("version:"))
            .ok_or("pubspec.yaml: нет version")?
            .trim();
        let (version, build) = v.split_once('+').ok_or("version должна быть x.y.z+build")?;
        let build: u64 = build.parse().map_err(|_| "build — не число")?;
        Ok(Artifacts {
            version: version.to_string(),
            build,
            apk: app.join(r"build\app\outputs\flutter-apk\app-arm64-v8a-release.apk"),
            exe: app.join(format!(r"build\installer\botato-setup-{version}.exe")),
        })
    }

    /// Runs `deploy.yml --tags <tags>` in WSL on a background thread.
    pub fn publish(&self, ctx: &egui::Context, tags: &str) {
        let mut d = self.deploy.lock().unwrap();
        if d.running {
            return;
        }
        d.running = true;
        d.log = format!("ansible-playbook --tags {tags} …\n");
        drop(d);
        let repo = wsl_path(&self.repo);
        let script = format!(
            "cd '{repo}' && export ANSIBLE_INVENTORY_ENABLED=ini && \
             ansible-playbook -i infra/ansible/inventory.ini infra/ansible/deploy.yml --tags {tags} 2>&1 | \
             grep -E 'TASK|changed:|fatal|failed=|unreachable=|PLAY RECAP|ok='"
        );
        let (state, ctx) = (self.deploy.clone(), ctx.clone());
        std::thread::spawn(move || {
            let mut cmd = std::process::Command::new("wsl.exe");
            cmd.args(["-d", WSL_DISTRO, "--", "bash", "-lc", &script]);
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
            }
            let out = cmd.output();
            let mut d = state.lock().unwrap();
            match out {
                Ok(o) => {
                    d.log.push_str(&String::from_utf8_lossy(&o.stdout));
                    d.log.push_str(&String::from_utf8_lossy(&o.stderr));
                    d.log.push_str(if o.status.success() { "\nГотово." } else { "\nОШИБКА, см. вывод выше." });
                }
                Err(e) => d.log.push_str(&format!("не запустился wsl.exe: {e}")),
            }
            d.running = false;
            ctx.request_repaint();
        });
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, admin: &mut Admin) {
        let ctx = ui.ctx().clone();
        let arts = self.artifacts();
        match &arts {
            Err(e) => {
                ui.colored_label(egui::Color32::LIGHT_RED, e);
            }
            Ok(a) => {
                ui.label(format!("Версия из pubspec.yaml: {} (сборка {})", a.version, a.build));
                for (name, p) in [("APK", &a.apk), ("Установщик", &a.exe)] {
                    ui.label(match std::fs::metadata(p) {
                        Ok(m) => format!("{name}: {:.1} МБ, {}", m.len() as f64 / 1048576.0, age(&m)),
                        Err(_) => format!("{name}: нет — соберите: tools\\build-release.ps1"),
                    });
                }
            }
        }
        ui.horizontal(|ui| {
            ui.label("Пароль корневого ключа:");
            ui.add(egui::TextEdit::singleline(&mut self.password).password(true).desired_width(220.0));
        });
        let running = self.deploy.lock().unwrap().running;
        ui.horizontal(|ui| {
            let ready = !running
                && !self.password.is_empty()
                && arts.as_ref().is_ok_and(|a| a.apk.exists() && a.exe.exists());
            if ui.add_enabled(ready, egui::Button::new("Подписать релиз и выкатить")).clicked() {
                self.sign_and_publish(&ctx, admin, arts.as_ref().ok());
            }
            if ui
                .add_enabled(!running, egui::Button::new("Выкатить только манифест"))
                .on_hover_text("После удаления устройства или смены сервера")
                .clicked()
            {
                self.publish(&ctx, "manifest");
            }
            if running {
                ui.spinner();
            }
        });
        if let Some((ok, msg)) = &self.status {
            ui.colored_label(if *ok { egui::Color32::LIGHT_GREEN } else { egui::Color32::LIGHT_RED }, msg);
        }
        let log = self.deploy.lock().unwrap().log.clone();
        if !log.is_empty() {
            let mut shown = log;
            ui.add(
                egui::TextEdit::multiline(&mut shown)
                    .desired_rows(8)
                    .desired_width(f32::INFINITY)
                    .font(egui::TextStyle::Monospace),
            );
        }
    }

    fn sign_and_publish(&mut self, ctx: &egui::Context, admin: &mut Admin, arts: Option<&Artifacts>) {
        let Some(a) = arts else { return };
        let result = (|| -> Result<(), String> {
            let root = admin.unlock(self.password.as_bytes()).map_err(|e| e.to_string())?;
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
            let files = [
                (proto::release::ANDROID_ARM64, a.apk.as_path(), format!("botato-{}.apk", a.version)),
                (proto::release::WINDOWS_X64, a.exe.as_path(), format!("botato-setup-{}.exe", a.version)),
            ];
            admin.sign_release(&root, &a.version, a.build, &files, now).map_err(|e| e.to_string())?;
            Ok(())
        })();
        self.password.zeroize();
        self.password.clear();
        match result {
            Ok(()) => {
                self.status = Some((true, format!("Релиз {} подписан, выкатываю…", a.version)));
                self.publish(ctx, "manifest,release");
            }
            Err(e) => self.status = Some((false, e)),
        }
    }
}

fn age(m: &std::fs::Metadata) -> String {
    let Ok(t) = m.modified() else { return String::new() };
    let mins = t.elapsed().map(|d| d.as_secs() / 60).unwrap_or(0);
    match mins {
        0 => "только что".into(),
        m if m < 60 => format!("{m} мин назад"),
        m if m < 48 * 60 => format!("{} ч назад", m / 60),
        m => format!("{} дн назад", m / 1440),
    }
}

/// `H:\Desktop\botato` → `/mnt/h/Desktop/botato`.
fn wsl_path(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    match s.split_once(":/") {
        Some((drive, rest)) => format!("/mnt/{}/{rest}", drive.to_lowercase()),
        None => s,
    }
}
