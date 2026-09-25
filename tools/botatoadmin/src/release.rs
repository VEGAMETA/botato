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
    apk32: PathBuf,
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
            apk32: app.join(r"build\app\outputs\flutter-apk\app-armeabi-v7a-release.apk"),
            exe: app.join(format!(r"build\installer\botato-setup-{version}.exe")),
        })
    }

    /// Runs `tools\build-release.ps1` (APK + Windows installer), streaming its output.
    pub fn build(&self, ctx: &egui::Context) {
        use std::io::{BufRead, BufReader};
        let mut d = self.deploy.lock().unwrap();
        if d.running {
            return;
        }
        d.running = true;
        d.log = "Сборка релиза (несколько минут)…\n".into();
        drop(d);
        let script = self.repo.join(r"tools\build-release.ps1");
        let (state, ctx) = (self.deploy.clone(), ctx.clone());
        std::thread::spawn(move || {
            let mut cmd = std::process::Command::new("powershell.exe");
            cmd.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command"])
                .arg(format!("& '{}' 2>&1 | ForEach-Object {{ \"$_\" }}", script.display()))
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped());
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
            }
            let ok = match cmd.spawn() {
                Ok(mut child) => {
                    if let Some(out) = child.stdout.take() {
                        for line in BufReader::new(out).lines().map_while(Result::ok) {
                            // Gradle/flutter progress lines are noise here.
                            if line.trim().is_empty() || line.contains("Built-in Kotlin") {
                                continue;
                            }
                            let mut d = state.lock().unwrap();
                            d.log.push_str(&line);
                            d.log.push('\n');
                            drop(d);
                            ctx.request_repaint();
                        }
                    }
                    child.wait().is_ok_and(|s| s.success())
                }
                Err(e) => {
                    state.lock().unwrap().log.push_str(&format!("не запустился powershell: {e}\n"));
                    false
                }
            };
            let mut d = state.lock().unwrap();
            d.log.push_str(if ok { "\nСобрано. Теперь «Подписать релиз и выкатить»." } else { "\nОШИБКА сборки, см. вывод выше." });
            d.running = false;
            ctx.request_repaint();
        });
    }

    /// Runs `deploy.yml --tags <tags>` in WSL on a background thread.
    /// Runs Ansible with `tags`; `nostr`: then also publishes the manifest on Nostr.
    pub fn publish(&self, ctx: &egui::Context, tags: &str, nostr: Option<admin::NostrJob>) {
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
            // The manifest on Nostr too: clients whose server is gone look for it there.
            if let Some(job) = nostr {
                d.log.push_str("\n\nNostr:\n");
                drop(d);
                let lines = job.run().join("\n");
                d = state.lock().unwrap();
                d.log.push_str(&lines);
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
                for (name, p) in [("APK", &a.apk), ("APK 32-bit", &a.apk32), ("Установщик", &a.exe)] {
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
                && arts.as_ref().is_ok_and(|a| a.apk.exists() && a.apk32.exists() && a.exe.exists());
            if ui
                .add_enabled(!running, egui::Button::new("Собрать релиз"))
                .on_hover_text("tools\\build-release.ps1: APK + установщик, версия из pubspec.yaml")
                .clicked()
            {
                self.build(&ctx);
            }
            if ui.add_enabled(ready, egui::Button::new("Подписать релиз и выкатить")).clicked() {
                self.sign_and_publish(&ctx, admin, arts.as_ref().ok());
            }
            if ui
                .add_enabled(!running, egui::Button::new("Выкатить только манифест"))
                .on_hover_text("После удаления устройства или смены сервера")
                .clicked()
            {
                self.publish(&ctx, "manifest", admin.nostr_job(now_secs()).ok());
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
            egui::ScrollArea::vertical()
                .id_salt("release-log")
                .max_height(260.0)
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    ui.add(
                        egui::TextEdit::multiline(&mut shown)
                            .desired_rows(8)
                            .desired_width(f32::INFINITY)
                            .font(egui::TextStyle::Monospace),
                    );
                });
        }
    }

    fn sign_and_publish(&mut self, ctx: &egui::Context, admin: &mut Admin, arts: Option<&Artifacts>) {
        let Some(a) = arts else { return };
        let result = (|| -> Result<(), String> {
            let root = admin.unlock(self.password.as_bytes()).map_err(|e| e.to_string())?;
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
            let files = [
                (proto::release::ANDROID_ARM64, a.apk.as_path(), format!("botato-{}.apk", a.version)),
                (proto::release::ANDROID_ARM, a.apk32.as_path(), format!("botato-{}-arm32.apk", a.version)),
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
                self.publish(ctx, "manifest,release", admin.nostr_job(now_secs()).ok());
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

pub fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()
}
