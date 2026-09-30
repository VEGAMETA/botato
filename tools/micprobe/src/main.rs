//! micprobe: which microphones WASAPI opens on this PC, and how.
//!
//! botato's capture (libwebrtc's and our own WASAPI one) got E_INVALIDARG from
//! IAudioClient::Initialize for every flag, format and stream mode on one laptop, while other
//! apps record fine. This exe runs the same calls outside botato, over every capture endpoint,
//! a grid of formats / flags / buffer sizes, stream modes, both COM apartments, records two
//! seconds with the first combination that opens, and adds the Windows privacy entries and
//! per-app audio settings that mention botato. The report goes to the console and to
//! micprobe.txt next to the exe. No audio is saved or sent anywhere.

use std::fmt::Write as _;
use std::process::Command;
use std::time::{Duration, Instant};

use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Media::Audio::*;
use windows::Win32::Media::KernelStreaming::WAVE_FORMAT_EXTENSIBLE;
use windows::Win32::System::Com::*;
use windows::core::{GUID, HRESULT, Interface};

const WAVE_FORMAT_PCM: u16 = 1;
const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
const FLOAT_SUBTYPE: GUID = GUID::from_u128(0x00000003_0000_0010_8000_00aa00389b71);

struct Report(String);

impl Report {
    fn line(&mut self, s: impl AsRef<str>) {
        println!("{}", s.as_ref());
        self.0.push_str(s.as_ref());
        self.0.push('\n');
    }
}

fn hr_text(hr: HRESULT) -> String {
    let name = match hr.0 as u32 {
        0 => "ok",
        0x80070057 => "E_INVALIDARG",
        0x80070005 => "E_ACCESSDENIED",
        0x88890008 => "UNSUPPORTED_FORMAT",
        0x8889000A => "DEVICE_IN_USE",
        0x88890004 => "DEVICE_INVALIDATED",
        0x88890019 => "BUFFER_SIZE_ERROR",
        0x88890020 => "BUFFER_SIZE_NOT_ALIGNED",
        0x88890026 => "ENGINE_PERIODICITY_LOCKED",
        0x80004001 => "E_NOTIMPL",
        0x80004002 => "E_NOINTERFACE",
        0x88890003 => "WRONG_ENDPOINT_TYPE",
        0x8889000E => "SERVICE_NOT_RUNNING",
        0x88890021 => "CPUUSAGE_EXCEEDED",
        0x88890027 => "EFFECT_NOT_AVAILABLE",
        _ => "",
    };
    if hr.0 == 0 { "ok".into() } else { format!("0x{:08x} {name}", hr.0 as u32) }
}

fn describe(f: &WAVEFORMATEX) -> String {
    let tag = f.wFormatTag;
    let mut kind = match tag {
        WAVE_FORMAT_PCM => "int".to_string(),
        WAVE_FORMAT_IEEE_FLOAT => "float".to_string(),
        t if t == WAVE_FORMAT_EXTENSIBLE as u16 => "ext".to_string(),
        t => format!("tag{t}"),
    };
    if tag == WAVE_FORMAT_EXTENSIBLE as u16 {
        let ext = unsafe { &*(f as *const WAVEFORMATEX as *const WAVEFORMATEXTENSIBLE) };
        let sub = ext.SubFormat;
        kind = if sub == FLOAT_SUBTYPE { "ext float".into() } else { "ext int".into() };
        let mask = ext.dwChannelMask;
        let valid = unsafe { ext.Samples.wValidBitsPerSample };
        kind += &format!(" valid{valid} mask0x{mask:x}");
    }
    let (rate, ch, bits) = (f.nSamplesPerSec, f.nChannels, f.wBitsPerSample);
    format!("{rate} Hz {ch} ch {bits}-bit {kind}")
}

fn pcm(tag: u16, rate: u32, ch: u16, bits: u16) -> WAVEFORMATEX {
    let align = ch * bits / 8;
    WAVEFORMATEX {
        wFormatTag: tag,
        nChannels: ch,
        nSamplesPerSec: rate,
        nAvgBytesPerSec: rate * align as u32,
        nBlockAlign: align,
        wBitsPerSample: bits,
        cbSize: 0,
    }
}

unsafe fn activate(dev: &IMMDevice) -> windows::core::Result<IAudioClient> {
    unsafe { dev.Activate::<IAudioClient>(CLSCTX_ALL, None) }
}

/// Initialize on a fresh client (a client initializes only once).
unsafe fn try_init(dev: &IMMDevice, fmt: *const WAVEFORMATEX, flags: u32, dur: i64, props: Option<AudioClientProperties>) -> HRESULT {
    unsafe {
        let client = match activate(dev) {
            Ok(c) => c,
            Err(e) => return e.code(),
        };
        if let Some(p) = props {
            match client.cast::<IAudioClient2>() {
                Ok(c2) => {
                    if let Err(e) = c2.SetClientProperties(&p) {
                        return e.code();
                    }
                }
                Err(e) => return e.code(),
            }
        }
        match client.Initialize(AUDCLNT_SHAREMODE_SHARED, flags, dur, 0, fmt, None) {
            Ok(()) => HRESULT(0),
            Err(e) => e.code(),
        }
    }
}

fn flag_name(flags: u32) -> String {
    let mut v = vec![];
    if flags & AUDCLNT_STREAMFLAGS_EVENTCALLBACK != 0 {
        v.push("event");
    }
    if flags & AUDCLNT_STREAMFLAGS_NOPERSIST != 0 {
        v.push("nopersist");
    }
    if flags & AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM != 0 {
        v.push("convert");
    }
    if v.is_empty() { "-".into() } else { v.join("+") }
}

/// Records ~2 s polled with the given format and reports the level.
unsafe fn record(r: &mut Report, dev: &IMMDevice, fmt: &WAVEFORMATEX, flags: u32) {
    unsafe {
        let run = || -> windows::core::Result<String> {
            let client = activate(dev)?;
            client.Initialize(AUDCLNT_SHAREMODE_SHARED, flags & !AUDCLNT_STREAMFLAGS_EVENTCALLBACK, 1_000_000, 0, fmt, None)?;
            let cap: IAudioCaptureClient = client.GetService()?;
            client.Start()?;
            let is_float = fmt.wFormatTag == WAVE_FORMAT_IEEE_FLOAT
                || (fmt.wFormatTag == WAVE_FORMAT_EXTENSIBLE as u16
                    && std::ptr::addr_of!((*(fmt as *const WAVEFORMATEX as *const WAVEFORMATEXTENSIBLE)).SubFormat).read_unaligned()
                        == FLOAT_SUBTYPE);
            let bytes = (fmt.wBitsPerSample / 8) as usize;
            let (mut frames_total, mut silent_packets, mut packets) = (0u64, 0u32, 0u32);
            let (mut peak, mut sum_sq, mut n) = (0f64, 0f64, 0u64);
            let start = Instant::now();
            while start.elapsed() < Duration::from_secs(2) {
                std::thread::sleep(Duration::from_millis(10));
                while cap.GetNextPacketSize()? > 0 {
                    let mut data = std::ptr::null_mut();
                    let (mut frames, mut bflags) = (0u32, 0u32);
                    cap.GetBuffer(&mut data, &mut frames, &mut bflags, None, None)?;
                    packets += 1;
                    frames_total += frames as u64;
                    if bflags & (AUDCLNT_BUFFERFLAGS_SILENT.0 as u32) != 0 {
                        silent_packets += 1;
                    } else if !data.is_null() {
                        let count = frames as usize * fmt.nChannels as usize;
                        for i in 0..count {
                            let p = data.add(i * bytes);
                            let v = if is_float && bytes == 4 {
                                (p as *const f32).read_unaligned() as f64
                            } else if bytes == 2 {
                                (p as *const i16).read_unaligned() as f64 / 32768.0
                            } else if bytes == 4 {
                                (p as *const i32).read_unaligned() as f64 / 2147483648.0
                            } else {
                                0.0
                            };
                            peak = peak.max(v.abs());
                            sum_sq += v * v;
                            n += 1;
                        }
                    }
                    cap.ReleaseBuffer(frames)?;
                }
            }
            client.Stop()?;
            let db = |x: f64| if x <= 1e-10 { -100.0 } else { 20.0 * x.log10() };
            let rms = if n > 0 { (sum_sq / n as f64).sqrt() } else { 0.0 };
            Ok(format!(
                "{frames_total} frames in {packets} packets ({silent_packets} flagged silent), peak {:.0} dBFS, rms {:.0} dBFS",
                db(peak),
                db(rms)
            ))
        };
        match run() {
            Ok(s) => r.line(format!("    record 2 s: {s}")),
            Err(e) => r.line(format!("    record 2 s: failed {}", hr_text(e.code()))),
        }
    }
}

unsafe fn probe_device(r: &mut Report, dev: &IMMDevice) {
    unsafe {
        let client = match activate(dev) {
            Ok(c) => c,
            Err(e) => {
                r.line(format!("  Activate: {}", hr_text(e.code())));
                return;
            }
        };
        let mix_ptr = match client.GetMixFormat() {
            Ok(p) => p,
            Err(e) => {
                r.line(format!("  GetMixFormat: {}", hr_text(e.code())));
                return;
            }
        };
        let mix = &*mix_ptr;
        r.line(format!("  mix format: {}", describe(mix)));
        let mut closest: *mut WAVEFORMATEX = std::ptr::null_mut();
        let hr = client.IsFormatSupported(AUDCLNT_SHAREMODE_SHARED, mix_ptr, Some(&mut closest));
        r.line(format!("  IsFormatSupported(mix): {}", hr_text(hr)));
        if let Ok(c3) = client.cast::<IAudioClient3>() {
            let (mut d, mut f, mut mn, mut mx) = (0u32, 0u32, 0u32, 0u32);
            match c3.GetSharedModeEnginePeriod(mix_ptr, &mut d, &mut f, &mut mn, &mut mx) {
                Ok(()) => r.line(format!("  engine period (frames): default {d}, min {mn}, max {mx}")),
                Err(e) => r.line(format!("  GetSharedModeEnginePeriod: {}", hr_text(e.code()))),
            }
            let c3b: windows::core::Result<IAudioClient3> = activate(dev).and_then(|c| c.cast());
            if let Ok(c3b) = c3b {
                let res = c3b.InitializeSharedAudioStream(0, if d > 0 { d } else { 480 }, mix_ptr, None);
                r.line(format!(
                    "  IAudioClient3::InitializeSharedAudioStream(mix): {}",
                    hr_text(res.err().map(|e| e.code()).unwrap_or(HRESULT(0)))
                ));
            }
        } else {
            r.line("  IAudioClient3: not available");
        }

        let formats: Vec<(&str, WAVEFORMATEX, u32)> = vec![
            ("mix", *mix, 0),
            ("48k s16 mono conv", pcm(WAVE_FORMAT_PCM, 48000, 1, 16), AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY),
            ("48k s16 stereo conv", pcm(WAVE_FORMAT_PCM, 48000, 2, 16), AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY),
            ("48k s16 stereo", pcm(WAVE_FORMAT_PCM, 48000, 2, 16), 0),
            ("48k f32 stereo", pcm(WAVE_FORMAT_IEEE_FLOAT, 48000, 2, 32), 0),
        ];
        // The mix format must be passed with its extensible tail.
        let flag_sets = [
            0,
            AUDCLNT_STREAMFLAGS_NOPERSIST,
            AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
            AUDCLNT_STREAMFLAGS_EVENTCALLBACK | AUDCLNT_STREAMFLAGS_NOPERSIST,
        ];
        let durations = [0i64, 200_000, 1_000_000];
        let mut first_ok: Option<(WAVEFORMATEX, bool, u32)> = None;
        for (name, fmt, extra) in &formats {
            let mut row = String::new();
            for flags in flag_sets {
                for dur in durations {
                    let ptr: *const WAVEFORMATEX = if *name == "mix" { mix_ptr } else { fmt };
                    let hr = try_init(dev, ptr, flags | extra, dur, None);
                    let _ = write!(row, " [{} {}ms: {}]", flag_name(flags | extra), dur / 10_000, hr_text(hr));
                    if hr.0 == 0 && first_ok.is_none() {
                        first_ok = Some((*fmt, *name == "mix", flags | extra));
                    }
                }
            }
            r.line(format!("  {name}:{row}"));
        }
        for (label, cat, opt) in [
            ("communications", AudioCategory_Communications, AUDCLNT_STREAMOPTIONS_NONE),
            ("raw", AudioCategory_Other, AUDCLNT_STREAMOPTIONS_RAW),
            ("speech", AudioCategory_Speech, AUDCLNT_STREAMOPTIONS_NONE),
        ] {
            let props = AudioClientProperties {
                cbSize: size_of::<AudioClientProperties>() as u32,
                bIsOffload: false.into(),
                eCategory: cat,
                Options: opt,
            };
            let hr = try_init(dev, mix_ptr, 0, 0, Some(props));
            r.line(format!("  mix, {label} mode: {}", hr_text(hr)));
        }
        match first_ok {
            Some((fmt, is_mix, flags)) => {
                let f: &WAVEFORMATEX = if is_mix { mix } else { &fmt };
                record(r, dev, f, flags);
            }
            None => r.line("    nothing opened, no recording"),
        }
        CoTaskMemFree(Some(mix_ptr as _));
    }
}

unsafe fn device_name(dev: &IMMDevice) -> String {
    unsafe {
        dev.OpenPropertyStore(STGM_READ)
            .and_then(|s| s.GetValue(&PKEY_Device_FriendlyName))
            .map(|v| v.to_string())
            .unwrap_or_else(|e| format!("? ({})", hr_text(e.code())))
    }
}

unsafe fn device_id(dev: &IMMDevice) -> String {
    unsafe {
        match dev.GetId() {
            Ok(p) => {
                let s = p.to_string().unwrap_or_default();
                CoTaskMemFree(Some(p.0 as _));
                s
            }
            Err(e) => format!("? ({})", hr_text(e.code())),
        }
    }
}

unsafe fn run(r: &mut Report) -> windows::core::Result<()> {
    unsafe {
        let hr = CoInitializeEx(None, COINIT_MULTITHREADED);
        r.line(format!("CoInitializeEx(MTA): {}", hr_text(hr)));
        let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        for (label, role) in [("console", eConsole), ("communications", eCommunications)] {
            match en.GetDefaultAudioEndpoint(eCapture, role) {
                Ok(d) => r.line(format!("default {label} input: {} ({})", device_name(&d), device_id(&d))),
                Err(e) => r.line(format!("default {label} input: {}", hr_text(e.code()))),
            }
        }
        let col = en.EnumAudioEndpoints(eCapture, DEVICE_STATE(DEVICE_STATEMASK_ALL))?;
        let count = col.GetCount()?;
        r.line(format!("capture endpoints (all states): {count}"));
        for i in 0..count {
            let dev = col.Item(i)?;
            let state = match dev.GetState() {
                Ok(s) if s == DEVICE_STATE_ACTIVE => "active",
                Ok(s) if s == DEVICE_STATE_DISABLED => "disabled",
                Ok(s) if s == DEVICE_STATE_NOTPRESENT => "not present",
                Ok(s) if s == DEVICE_STATE_UNPLUGGED => "unplugged",
                _ => "?",
            };
            r.line(String::new());
            r.line(format!("#{i} {} [{state}] {}", device_name(&dev), device_id(&dev)));
            if state == "active" {
                probe_device(r, &dev);
            }
        }

        // The same simplest call from a single-threaded apartment (a UI thread).
        let sta = std::thread::spawn(|| {
            let hr = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let res = (|| -> windows::core::Result<HRESULT> {
                let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
                let dev = en.GetDefaultAudioEndpoint(eCapture, eConsole)?;
                let c = activate(&dev)?;
                let mix = c.GetMixFormat()?;
                let hr = try_init(&dev, mix, 0, 0, None);
                CoTaskMemFree(Some(mix as _));
                Ok(hr)
            })();
            format!(
                "STA thread (CoInitializeEx {}): default input, mix, no flags: {}",
                hr_text(hr),
                match res {
                    Ok(h) => hr_text(h),
                    Err(e) => hr_text(e.code()),
                }
            )
        })
        .join()
        .unwrap_or_default();
        r.line(String::new());
        r.line(sta);
        Ok(())
    }
}

fn shell(r: &mut Report, title: &str, args: &[&str]) {
    r.line(String::new());
    r.line(format!("== {title}"));
    match Command::new("reg").args(args).output() {
        Ok(o) => {
            let text = String::from_utf8_lossy(&o.stdout);
            let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
            if lines.is_empty() {
                r.line("  (nothing)");
            }
            for l in lines.iter().take(80) {
                r.line(format!("  {}", l.trim_end()));
            }
        }
        Err(e) => r.line(format!("  reg failed: {e}")),
    }
}

fn main() {
    let mut r = Report(String::new());
    r.line("micprobe: WASAPI capture check (nothing is saved or sent)");
    if let Ok(o) = Command::new("cmd").args(["/c", "ver"]).output() {
        r.line(String::from_utf8_lossy(&o.stdout).trim().to_string());
    }
    if let Err(e) = unsafe { run(&mut r) } {
        r.line(format!("error: {}", hr_text(e.code())));
    }
    let consent = r"HKCU\Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\microphone";
    shell(&mut r, "microphone privacy (this user)", &["query", consent, "/s", "/v", "Value"]);
    shell(
        &mut r,
        "microphone privacy (this device)",
        &["query", r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\microphone", "/v", "Value"],
    );
    shell(
        &mut r,
        "per-app audio settings mentioning botato",
        &["query", r"HKCU\Software\Microsoft\Internet Explorer\LowRegistry\Audio\PolicyConfig\PropertyStore", "/s", "/f", "botato"],
    );

    let path = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("micprobe.txt")))
        .unwrap_or_else(|| "micprobe.txt".into());
    match std::fs::write(&path, &r.0) {
        Ok(()) => println!("\nSaved to {}", path.display()),
        Err(e) => println!("\nCouldn't save {}: {e}", path.display()),
    }
    println!("Press Enter to close.");
    let _ = std::io::stdin().read_line(&mut String::new());
}
