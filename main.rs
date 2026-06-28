#![windows_subsystem = "windows"]

use eframe::egui;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
#[cfg(windows)]
use std::os::windows::process::CommandExt;
#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;
#[cfg(windows)]
use windows_sys::Win32::Foundation::{GetLastError, HWND, LPARAM, POINT, WPARAM};
#[cfg(windows)]
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
#[cfg(windows)]
use windows_sys::Win32::UI::Shell::{NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW, Shell_NotifyIconW};
#[cfg(windows)]
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow,
    DispatchMessageW, GetCursorPos, GetMessageW, GetWindowLongPtrW, LoadImageW, PostQuitMessage,
    RegisterClassW, SetForegroundWindow, SetWindowLongPtrW, TrackPopupMenu, TranslateMessage,
    GWLP_USERDATA, MF_SEPARATOR, MF_STRING, MSG, TPM_LEFTALIGN, TPM_RETURNCMD,
    WM_DESTROY, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_USER, WNDCLASSW, WS_OVERLAPPED,
};
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

static APP_ICON_PNG: &[u8] = include_bytes!("../icons/icon256.png");

// ── Logger ──────────────────────────────────────────────────────────────────

fn log_msg(msg: &str) {
    let path = std::env::temp_dir().join("wakeupllm.log");
    use std::io::Write;
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "{}", msg);
        let _ = f.flush();
    }
}

fn resolve_exe(path: &str) -> String {
    let p = std::path::Path::new(path);
    if p.is_absolute() && p.exists() {
        return path.to_string();
    }
    if let Some(exe_dir) = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|d| d.to_path_buf()))
    {
        let local = exe_dir.join(p);
        if local.exists() {
            return local.to_string_lossy().into_owned();
        }
    }
    path.to_string()
}

macro_rules! log {
    ($($arg:tt)*) => { log_msg(&format!($($arg)*)) };
}

#[cfg(windows)]
fn to_wide(s: &str) -> Vec<u16> {
    std::ffi::OsStr::new(s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

#[derive(Debug, Clone, Copy)]
enum TrayAction {
    Minimize,
    UnloadAll,
    Refresh,
    Quit,
}

#[cfg(windows)]
unsafe extern "system" fn tray_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> isize {
    if msg == WM_USER + 1 {
        let lparam_msg = lparam as u32;
        log!("WM_TRAYICON lParam={lparam_msg}");
        if lparam_msg == WM_RBUTTONDOWN || lparam_msg == WM_RBUTTONUP {
            log!("Right-click detected, showing menu");
            let mut cursor = POINT { x: 0, y: 0 };
            if GetCursorPos(&mut cursor) != 0 {
                log!("Cursor ({}, {})", cursor.x, cursor.y);
                let hmenu = CreatePopupMenu();
                if !hmenu.is_null() {
                    log!("Menu handle OK");
                    let w_minimize = to_wide("Minimize Window");
                    let w_unload = to_wide("Unload All");
                    let w_refresh = to_wide("Refresh");
                    let w_quit = to_wide("Quit");
                    AppendMenuW(hmenu, MF_STRING, 1001, w_minimize.as_ptr());
                    AppendMenuW(hmenu, MF_STRING, 1002, w_unload.as_ptr());
                    AppendMenuW(hmenu, MF_SEPARATOR, 0, std::ptr::null());
                    AppendMenuW(hmenu, MF_STRING, 1003, w_refresh.as_ptr());
                    AppendMenuW(hmenu, MF_SEPARATOR, 0, std::ptr::null());
                    AppendMenuW(hmenu, MF_STRING, 1004, w_quit.as_ptr());

                    SetForegroundWindow(hwnd);
                    let cmd = TrackPopupMenu(
                        hmenu,
                        TPM_LEFTALIGN | TPM_RETURNCMD,
                        cursor.x,
                        cursor.y,
                        0,
                        hwnd,
                        std::ptr::null(),
                    );
                    log!("TrackPopupMenu returned {cmd}");
                    DestroyMenu(hmenu);

                    let sender_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut std::sync::mpsc::Sender<TrayAction>;
                    if !sender_ptr.is_null() {
                        let sender = &*sender_ptr;
                        match cmd as u32 {
                            1001 => { let _ = sender.send(TrayAction::Minimize); }
                            1002 => { let _ = sender.send(TrayAction::UnloadAll); }
                            1003 => { let _ = sender.send(TrayAction::Refresh); }
                            1004 => { let _ = sender.send(TrayAction::Quit); }
                            _ => {}
                        }
                    }
                } else {
                    log!("CreatePopupMenu NULL!");
                }
            } else {
                log!("GetCursorPos FAILED");
            }
        }
        return 0;
    }
    if msg == WM_DESTROY {
        PostQuitMessage(0);
        return 0;
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

// ── Port health check ───────────────────────────────────────────────────────

fn port_is_alive(port: u16) -> bool {
    TcpStream::connect_timeout(
        &format!("127.0.0.1:{port}").parse().unwrap(),
        Duration::from_secs(2),
    )
    .is_ok()
}

fn server_is_ready(port: u16) -> bool {
    if let Ok(mut stream) = TcpStream::connect_timeout(
        &format!("127.0.0.1:{port}").parse().unwrap(),
        Duration::from_secs(2),
    ) {
        let _ = stream.write_all(b"GET /health HTTP/1.0\r\n\r\n");
        let mut buf = [0u8; 512];
        if let Ok(n) = stream.read(&mut buf) {
            let resp = String::from_utf8_lossy(&buf[..n]);
            return resp.contains("200 OK");
        }
    }
    false
}

fn port_owner_pid(port: u16) -> Option<u32> {
    let output = Command::new("netstat").args(["-ano"]).output().ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let suffix = format!(":{port}");

    for line in text.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 5 {
            continue;
        }
        if fields.get(3).copied() != Some("LISTENING") {
            continue;
        }
        let pid = fields.last()?.parse::<u32>().ok()?;
        if fields.get(1).is_some_and(|local| local.ends_with(&suffix)) {
            return Some(pid);
        }
    }

    None
}

#[cfg(windows)]
fn kill_pid(pid: u32) -> Result<(), String> {
    let pid_str = pid.to_string();
    let status = Command::new("taskkill")
        .args(["/PID", pid_str.as_str(), "/F"])
        .status()
        .map_err(|e| format!("taskkill PID {pid}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("taskkill failed for PID {pid}"))
    }
}

#[cfg(not(windows))]
fn kill_pid(pid: u32) -> Result<(), String> {
    let pid_str = pid.to_string();
    let status = Command::new("kill")
        .args(["-9", pid_str.as_str()])
        .status()
        .map_err(|e| format!("kill PID {pid}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("kill failed for PID {pid}"))
    }
}

// ── Config ──────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Clone)]
struct AppConfig {
    out_port: Option<u16>,
    #[serde(default = "default_idle")]
    idle_timeout_seconds: u64,
    models: Vec<ModelEntry>,
}

fn default_idle() -> u64 {
    0
}

#[derive(Debug, Deserialize, Clone)]
struct ModelProfile {
    name: String,
    #[serde(default)]
    temperature: Option<f32>,
    #[serde(default)]
    top_p: Option<f32>,
    #[serde(default)]
    top_k: Option<u32>,
    #[serde(default)]
    presence_penalty: Option<f32>,
}

#[derive(Debug, Deserialize, Clone)]
struct ModelEntry {
    id: String,
    port: u16,
    executable: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    alias: Option<String>,
    #[serde(default)]
    arguments: Vec<String>,
    #[serde(default)]
    profiles: Vec<ModelProfile>,
}

impl AppConfig {
    fn load() -> Option<Self> {
        let dirs: Vec<PathBuf> = vec![
            std::env::current_exe()
                .ok()
                .as_ref()
                .and_then(|p| p.parent().map(|p| p.to_path_buf())),
            std::env::current_dir().ok(),
            Some(PathBuf::from(env!("CARGO_MANIFEST_DIR"))),
        ]
        .into_iter()
        .flatten()
        .collect();

        for dir in &dirs {
            let path = dir.join("model-config.json");
            if path.exists() {
                if let Ok(data) = std::fs::read_to_string(&path) {
                    if let Ok(cfg) = serde_json::from_str::<AppConfig>(&data) {
                        return Some(cfg);
                    }
                }
            }
        }
        None
    }
}

// ── Process Manager ─────────────────────────────────────────────────────────

struct ProcInfo {
    _port: u16,
    handle: std::process::Child,
    last_activity: Instant,
}

struct ProcessManager {
    procs: HashMap<u16, ProcInfo>,
    cmd_cache: HashMap<u16, (String, Vec<String>)>,
    idle_timeout: Duration,
}

impl ProcessManager {
    fn new(idle_secs: u64, models: &[ModelEntry]) -> Self {
        let mut cmd_cache = HashMap::new();
        for m in models {
            let mut args = Vec::new();
            let mut skip = false;
            for a in &m.arguments {
                if skip {
                    skip = false;
                    continue;
                }
                if a == "--port" || a == "--host" || a == "--sleep-idle-seconds" {
                    skip = true;
                    continue;
                }
                args.push(a.clone());
            }
            // Auto-inject --port and --host so config doesn't need them
            args.push("--port".into());
            args.push(m.port.to_string());
            args.push("--host".into());
            args.push("0.0.0.0".into());
            cmd_cache.insert(m.port, (resolve_exe(&m.executable), args));
        }
        Self {
            procs: HashMap::new(),
            cmd_cache,
            idle_timeout: Duration::from_secs(idle_secs),
        }
    }

    fn is_running(&self, port: u16) -> bool {
        self.procs.contains_key(&port)
    }

    fn touch(&mut self, port: u16) {
        if let Some(p) = self.procs.get_mut(&port) {
            p.last_activity = Instant::now();
        }
    }

    fn start(&mut self, port: u16) -> Result<(), String> {
        if self.is_running(port) {
            self.touch(port);
            return Ok(());
        }
        let (exe, args) = self
            .cmd_cache
            .get(&port)
            .ok_or_else(|| format!("No config for port {port}"))?;

        log!(
            "Spawning {} on port {}: {} {}",
            exe,
            port,
            exe,
            args.join(" ")
        );
        let mut cmd = Command::new(exe);
        cmd.args(args)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        #[cfg(windows)]
        {
            cmd.creation_flags(0x08000000);
        }
        let handle = cmd.spawn().map_err(|e| format!("Spawn {port}: {e}"))?;

        self.procs.insert(
            port,
            ProcInfo {
                _port: port,
                handle,
                last_activity: Instant::now(),
            },
        );

        for i in 0..120 {
            if port_is_alive(port) {
                log!("Model ready on port {port} ({i}s)");
                return Ok(());
            }
            thread::sleep(Duration::from_secs(1));
        }
        Err(format!("Timeout waiting for port {port}"))
    }

    fn stop(&mut self, port: u16) -> Result<String, String> {
        if let Some(mut info) = self.procs.remove(&port) {
            let _ = info.handle.kill();
            let _ = info.handle.wait();
            log!("Stopped port {port}");
            return Ok(format!("Stopped port {port}"));
        }

        if let Some(pid) = port_owner_pid(port) {
            if pid == std::process::id() {
                return Err(format!(
                    "Port {port} is owned by the current process but is not tracked"
                ));
            }
            kill_pid(pid).map_err(|e| format!("Port {port} owner PID {pid}: {e}"))?;
            log!("Killed external process {pid} on port {port}");
            return Ok(format!("Killed external process {pid} on port {port}"));
        }

        Err(format!("Port {port} not running"))
    }

    fn stop_idle(&mut self) -> Vec<u16> {
        if self.idle_timeout.is_zero() {
            return vec![];
        }
        let now = Instant::now();
        let idle: Vec<u16> = self
            .procs
            .iter()
            .filter(|(_, v)| now.duration_since(v.last_activity) > self.idle_timeout)
            .map(|(&k, _)| k)
            .collect();
        for port in &idle {
            if let Some(mut info) = self.procs.remove(port) {
                let _ = info.handle.kill();
                let _ = info.handle.wait();
                log!("Idle timeout stopped port {port}");
            }
        }
        idle
    }

    fn running_ports(&self) -> Vec<u16> {
        self.procs.keys().copied().collect()
    }

    fn kill_all(&mut self) {
        for (_, mut info) in self.procs.drain() {
            let _ = info.handle.kill();
            let _ = info.handle.wait();
        }
    }
}

// ── Proxy Server ────────────────────────────────────────────────────────────

fn start_proxy(pm: Arc<Mutex<ProcessManager>>, cfg: Arc<AppConfig>, out_port: u16) {
    let addr = format!("0.0.0.0:{out_port}");
    log!("Proxy binding {addr}...");
    let listener = match TcpListener::bind(&addr) {
        Ok(l) => {
            log!("Proxy bound on port {out_port}");
            l
        }
        Err(e) => {
            log!("Proxy bind failed: {e}");
            return;
        }
    };

    for stream in listener.incoming() {
        let stream = match stream {
            Ok(s) => s,
            Err(_) => continue,
        };
        let _ = stream.set_read_timeout(Some(Duration::from_secs(60)));
        let _ = stream.set_write_timeout(Some(Duration::from_secs(60)));
        let pm = Arc::clone(&pm);
        let models = cfg.models.clone();
        thread::spawn(move || handle_conn(stream, pm, &models));
    }
}

fn handle_conn(mut client: TcpStream, pm: Arc<Mutex<ProcessManager>>, models: &[ModelEntry]) {
    let _peer = client
        .peer_addr()
        .map(|a| a.to_string())
        .unwrap_or_default();

    let mut req_buf = Vec::with_capacity(65536);
    let mut tmp = [0u8; 8192];

    loop {
        match client.read(&mut tmp) {
            Ok(0) => return,
            Ok(n) => {
                req_buf.extend_from_slice(&tmp[..n]);
                if req_buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
                if n < 8192 {
                    break;
                }
            }
            Err(_) => return,
        }
    }

    let header_end = req_buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .unwrap_or(req_buf.len());
    let (method, path, content_length) = {
        let req_str = String::from_utf8_lossy(&req_buf[..header_end]);
        let first_line = req_str.lines().next().unwrap_or("");
        let parts: Vec<&str> = first_line.split_whitespace().collect();
        let method = parts.get(0).unwrap_or(&"").to_string();
        let path = parts.get(1).unwrap_or(&"/").to_string();
        let cl: usize = req_str
            .lines()
            .skip(1)
            .find_map(|l| {
                let lo = l.to_lowercase();
                if lo.starts_with("content-length:") {
                    lo["content-length:".len()..].trim().parse().ok()
                } else {
                    None
                }
            })
            .unwrap_or(0);
        (method, path, cl)
    };
    let body_start = header_end + 4;
    let body_len = req_buf.len().saturating_sub(body_start);

    if body_len < content_length {
        let mut remaining = content_length - body_len;
        while remaining > 0 {
            match client.read(&mut tmp) {
                Ok(0) => break,
                Ok(n) => {
                    let take = n.min(remaining);
                    req_buf.extend_from_slice(&tmp[..take]);
                    remaining -= take;
                }
                Err(_) => break,
            }
        }
    }

    // ── /health ────────────────────────────────────────────────────────────
    if path == "/health" {
        let pm = pm.lock().unwrap();
        let running = pm.running_ports();
        let status: HashMap<String, bool> = models
            .iter()
            .map(|m| (m.id.clone(), running.contains(&m.port)))
            .collect();
        let body = serde_json::json!({"status": "ok", "models": status});
        send_json(&client, 200, &body);
        return;
    }

    // ── /v1/models ───────────────────────────────────────────────────────────
    if path == "/v1/models" && method == "GET" {
        let models_list: Vec<_> = models
            .iter()
            .map(|m| {
                serde_json::json!({
                    "id": m.id,
                    "object": "model",
                    "created": 0,
                    "owned_by": "local",
                })
            })
            .collect();
        send_json(
            &client,
            200,
            &serde_json::json!({
                "object": "list",
                "data": models_list,
            }),
        );
        return;
    }

    // ── /unload/{port} ─────────────────────────────────────────────────────
    if path.starts_with("/unload/") {
        let port: u16 = path["/unload/".len()..].parse().unwrap_or(0);
        if port > 0 {
            match pm.lock().unwrap().stop(port) {
                Ok(msg) => send_json(
                    &client,
                    200,
                    &serde_json::json!({"status":"ok","message":msg}),
                ),
                Err(e) => send_json(
                    &client,
                    404,
                    &serde_json::json!({"status":"error","message":e}),
                ),
            }
        } else {
            send_json(
                &client,
                400,
                &serde_json::json!({"status":"error","message":"Invalid port"}),
            );
        }
        return;
    }

    // ── OpenAI routes ──────────────────────────────────────────────────────
    let body_bytes = &req_buf[body_start..];
    let model_id: Option<String> = if !body_bytes.is_empty() {
        serde_json::from_slice::<serde_json::Value>(body_bytes)
            .ok()
            .and_then(|v| v.get("model").and_then(|v| v.as_str().map(String::from)))
    } else {
        None
    };

    let model_id = match model_id {
        Some(id) => id,
        None => {
            send_json(
                &client,
                400,
                &serde_json::json!({"error":"Missing model field in request body"}),
            );
            return;
        }
    };

    let (base_id, profile_name) = if let Some(pos) = model_id.rfind('/') {
        (&model_id[..pos], Some(&model_id[pos + 1..]))
    } else {
        (model_id.as_str(), None)
    };

    let model_entry = match models.iter().find(|m| m.id == base_id) {
        Some(m) => m,
        None => {
            send_json(
                &client,
                404,
                &serde_json::json!({"error":format!("Unknown model: {base_id}")}),
            );
            return;
        }
    };
    let target_port = model_entry.port;

    let profile = profile_name.and_then(|pn| find_profile(model_entry, pn));

    let final_req_buf: Vec<u8> = if let Some(p) = profile {
        let new_body = apply_profile_to_body(body_bytes, p);
        if new_body != body_bytes {
            let mut new_buf = req_buf[..body_start].to_vec();
            new_buf.extend_from_slice(&new_body);
            new_buf
        } else {
            req_buf.clone()
        }
    } else {
        req_buf.clone()
    };

    // Check if already running
    let already_running = {
        let pm = pm.lock().unwrap();
        pm.is_running(target_port)
    };

    if !already_running {
        log!("Cold start: model {model_id} on port {target_port}");
        let mut pm = pm.lock().unwrap();
        if let Err(e) = pm.start(target_port) {
            log!("Start failed for {model_id}: {e}");
            send_json(
                &client,
                503,
                &serde_json::json!({"error":format!("Failed to start model: {e}")}),
            );
            return;
        }
        drop(pm);

        // Đợi upstream sẵn sàng (model load xong)
        log!("Waiting for model to finish loading on port {target_port}...");
        for i in 0..120 {
            if server_is_ready(target_port) {
                log!("Model {} fully loaded on port {} ({}s)", model_id, target_port, i);
                break;
            }
            if i >= 119 {
                log!("Model {} load timeout on port {}", model_id, target_port);
                send_json(
                    &client,
                    503,
                    &serde_json::json!({"error": "Model loading timeout after 120s"}),
                );
                return;
            }
            thread::sleep(Duration::from_secs(1));
        }
    }

    // Connect upstream
    let mut upstream = match TcpStream::connect_timeout(
        &format!("127.0.0.1:{target_port}").parse().unwrap(),
        Duration::from_secs(10),
    ) {
        Ok(u) => u,
        Err(e) => {
            send_json(
                &client,
                502,
                &serde_json::json!({"error":format!("Cannot connect to upstream: {e}")}),
            );
            return;
        }
    };
    let _ = upstream.set_read_timeout(Some(Duration::from_secs(60)));
    let _ = upstream.set_write_timeout(Some(Duration::from_secs(60)));

    // Forward request
    let fixed_req = rebuild_request(&final_req_buf, header_end, target_port);
    let _ = upstream.write_all(&fixed_req);
    let _ = upstream.flush();

    // Pipe response back - read from upstream, write to client
    let mut client_out = match client.try_clone() {
        Ok(c) => c,
        Err(_) => return,
    };

    let _ = thread::spawn(move || {
        let mut buf = [0u8; 16384];
        loop {
            match upstream.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let _ = client_out.write_all(&buf[..n]);
                    let _ = client_out.flush();
                }
                Err(_) => break,
            }
        }
    });

    pm.lock().unwrap().touch(target_port);
}

fn rebuild_request(req_buf: &[u8], header_end: usize, target_port: u16) -> Vec<u8> {
    let header_bytes = &req_buf[..header_end];
    let body = &req_buf[header_end + 4..];
    let header_str = String::from_utf8_lossy(header_bytes);
    let mut result = Vec::with_capacity(req_buf.len());

    for line in header_str.lines() {
        let lower = line.to_lowercase();
        if lower.starts_with("host:") {
            result.extend_from_slice(format!("Host: 127.0.0.1:{target_port}\r\n").as_bytes());
        } else if lower.starts_with("content-length:") || lower.starts_with("connection:") {
            continue;
        } else {
            result.extend_from_slice(line.as_bytes());
            result.extend_from_slice(b"\r\n");
        }
    }

    result.extend_from_slice(format!("Content-Length: {}\r\n", body.len()).as_bytes());
    result.extend_from_slice(b"Connection: close\r\n\r\n");
    result.extend_from_slice(body);
    result
}

fn apply_profile_to_body(body_bytes: &[u8], profile: &ModelProfile) -> Vec<u8> {
    let mut json: serde_json::Value = match serde_json::from_slice(body_bytes) {
        Ok(v) => v,
        Err(_) => return body_bytes.to_vec(),
    };

    if let Some(obj) = json.as_object_mut() {
        if let Some(temp) = profile.temperature {
            if !obj.contains_key("temperature") {
                obj.insert("temperature".to_string(), serde_json::json!(temp));
            }
        }
        if let Some(top_p) = profile.top_p {
            if !obj.contains_key("top_p") {
                obj.insert("top_p".to_string(), serde_json::json!(top_p));
            }
        }
        if let Some(top_k) = profile.top_k {
            if !obj.contains_key("top_k") {
                obj.insert("top_k".to_string(), serde_json::json!(top_k));
            }
        }
        if let Some(presence) = profile.presence_penalty {
            if !obj.contains_key("presence_penalty") {
                obj.insert("presence_penalty".to_string(), serde_json::json!(presence));
            }
        }
    }

    serde_json::to_vec(&json).unwrap_or_else(|_| body_bytes.to_vec())
}

fn find_profile<'a>(model: &'a ModelEntry, profile_name: &str) -> Option<&'a ModelProfile> {
    model.profiles.iter().find(|p| p.name == profile_name)
}

fn send_json(mut stream: &TcpStream, status: u16, body: &serde_json::Value) {
    let text = body.to_string();
    let status_text = match status {
        200 => "200 OK",
        400 => "400 Bad Request",
        404 => "404 Not Found",
        502 => "502 Bad Gateway",
        503 => "503 Service Unavailable",
        _ => "500 Internal Server Error",
    };
    let resp = format!(
        "HTTP/1.1 {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
        status_text, text.len()
    );
    let _ = stream.write_all(resp.as_bytes());
}

// ── Idle Monitor ────────────────────────────────────────────────────────────

fn start_idle_monitor(pm: Arc<Mutex<ProcessManager>>) {
    thread::spawn(move || loop {
        thread::sleep(Duration::from_secs(60));
        pm.lock().unwrap().stop_idle();
    });
}

fn get_profile_file_path() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .as_ref()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .map(|p| p.join("profile-selections.json"))
}

fn load_profile_selections() -> HashMap<String, String> {
    let path = match get_profile_file_path() {
        Some(p) => p,
        None => return HashMap::new(),
    };
    if let Ok(data) = std::fs::read_to_string(&path) {
        if let Ok(map) = serde_json::from_str(&data) {
            return map;
        }
    }
    HashMap::new()
}

fn save_profile_selection(
    model_id: &str,
    profile: Option<&str>,
    all_selections: &mut HashMap<String, String>,
) {
    match profile {
        Some(p) => {
            all_selections.insert(model_id.to_string(), p.to_string());
        }
        None => {
            all_selections.remove(model_id);
        }
    }
    let path = match get_profile_file_path() {
        Some(p) => p,
        None => return,
    };
    if let Ok(json) = serde_json::to_string_pretty(all_selections) {
        let _ = std::fs::write(&path, json);
    }
}

fn get_running_models_path() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .as_ref()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .map(|p| p.join("running-models.json"))
}

fn load_running_models() -> Vec<String> {
    let path = match get_running_models_path() {
        Some(p) => p,
        None => return Vec::new(),
    };
    if let Ok(data) = std::fs::read_to_string(&path) {
        if let Ok(models) = serde_json::from_str(&data) {
            return models;
        }
    }
    Vec::new()
}

fn save_running_models(model_ids: &[String]) {
    let path = match get_running_models_path() {
        Some(p) => p,
        None => return,
    };
    if let Ok(json) = serde_json::to_string_pretty(model_ids) {
        let _ = std::fs::write(&path, json);
    }
}

// ── GUI ─────────────────────────────────────────────────────────────────────

static SHOW_WINDOW: AtomicBool = AtomicBool::new(true);
static MINIMIZE_REQUESTED: AtomicBool = AtomicBool::new(false);

#[derive(Clone)]
struct ModelDisplay {
    id: String,
    port: u16,
    display_name: String,
    alias: String,
    owned: bool,
    external: bool,
    warming: bool,
    profiles: Vec<String>,
    selected_profile: Option<String>,
    ctx_size: u32,
    vision: bool,
    flash_attn: bool,
    is_moe: bool,
}

struct WakeupApp {
    models: Vec<ModelDisplay>,
    message: String,
    proxy_port: u16,
    pm: Arc<Mutex<ProcessManager>>,
    models_cfg: Vec<ModelEntry>,
    quit: Arc<AtomicBool>,
    idle_timeout_mins: u64,
    warming_ports: Arc<Mutex<HashSet<u16>>>,
    repaint: Arc<AtomicBool>,
    profile_selections: HashMap<String, String>,
    auto_warm_models: Vec<String>,
    tray_rx: std::sync::mpsc::Receiver<TrayAction>,
}

impl WakeupApp {
    fn refresh(&mut self) {
        let pm = self.pm.lock().unwrap();
        let mut warming = self.warming_ports.lock().unwrap();
        let running_ports = pm.running_ports();

        for port in running_ports.iter() {
            warming.remove(port);
        }
        let orphaned: Vec<u16> = warming
            .iter()
            .filter(|p| !pm.is_running(**p))
            .cloned()
            .collect();
        for port in orphaned {
            warming.remove(&port);
        }

        self.models = self
            .models_cfg
            .iter()
            .map(|m| {
                let name = m.id.rsplit('/').next().unwrap_or(&m.id);
                let owned = pm.is_running(m.port);
                let external = !owned && port_is_alive(m.port);
                let profile_names: Vec<String> =
                    m.profiles.iter().map(|p| p.name.clone()).collect();
                let selected = self.profile_selections.get(&m.id).cloned();

                let ctx_size = get_arg_value(&m.arguments, "--ctx-size")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0);
                let vision = has_arg(&m.arguments, "--mmproj");
                let flash_attn = has_arg(&m.arguments, "--flash-attn");
                let is_moe = has_arg(&m.arguments, "--n-cpu-moe");

                ModelDisplay {
                    id: m.id.clone(),
                    port: m.port,
                    display_name: name.to_string(),
                    alias: m.alias.clone().unwrap_or_default(),
                    owned,
                    external,
                    warming: warming.contains(&m.port) && !owned && !external,
                    profiles: profile_names,
                    selected_profile: selected,
                    ctx_size,
                    vision,
                    flash_attn,
                    is_moe,
                }
            })
            .collect();
        drop(warming);

        let running_model_ids: Vec<String> = self
            .models
            .iter()
            .filter(|m| m.owned)
            .map(|m| m.id.clone())
            .collect();
        save_running_models(&running_model_ids);

        let running_count = self.models.iter().filter(|m| m.owned).count();
        self.message = format!(
            "{} running | Proxy :{} | Idle: {}m",
            running_count, self.proxy_port, self.idle_timeout_mins
        );
    }

    fn warmup_model(&self, model_id: &str) {
        if let Some(model) = self.models_cfg.iter().find(|m| m.id == model_id) {
            let pm = Arc::clone(&self.pm);
            let wports = Arc::clone(&self.warming_ports);
            let rp = Arc::clone(&self.repaint);
            let port = model.port;
            wports.lock().unwrap().insert(port);
            rp.store(true, Ordering::SeqCst);
            thread::spawn(move || {
                log!("[auto] Warmup port {port}");
                let _ = pm.lock().unwrap().start(port);
                wports.lock().unwrap().remove(&port);
                rp.store(true, Ordering::SeqCst);
            });
        }
    }
}

impl eframe::App for WakeupApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.request_repaint_after(Duration::from_secs(5));

        if self.repaint.swap(false, Ordering::SeqCst) {
            self.refresh();
            ctx.request_repaint();
        }

        if SHOW_WINDOW.swap(false, Ordering::SeqCst) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }

        if MINIMIZE_REQUESTED.swap(false, Ordering::SeqCst) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
        }

        // Handle tray actions
        while let Ok(action) = self.tray_rx.try_recv() {
            match action {
                TrayAction::Minimize => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                }
                TrayAction::UnloadAll => {
                    let ports: Vec<u16> = self.models.iter()
                        .filter(|m| m.owned)
                        .map(|m| m.port)
                        .collect();
                    if !ports.is_empty() {
                        let pm = Arc::clone(&self.pm);
                        let rp = Arc::clone(&self.repaint);
                        thread::spawn(move || {
                            for port in ports {
                                let _ = pm.lock().unwrap().stop(port);
                            }
                            rp.store(true, Ordering::SeqCst);
                        });
                    }
                }
                TrayAction::Refresh => {
                    self.refresh();
                    ctx.request_repaint();
                }
                TrayAction::Quit => {
                    self.quit.store(true, Ordering::SeqCst);
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }

        egui::TopBottomPanel::bottom("footer").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("⬇ Unload All").clicked() {
                        let ports: Vec<u16> = self.models.iter()
                            .filter(|m| m.owned)
                            .map(|m| m.port)
                            .collect();
                        if !ports.is_empty() {
                            let pm = Arc::clone(&self.pm);
                            let rp = Arc::clone(&self.repaint);
                            thread::spawn(move || {
                                for port in ports {
                                    let _ = pm.lock().unwrap().stop(port);
                                }
                                rp.store(true, Ordering::SeqCst);
                            });
                        }
                    }
                });
            });
        });

        egui::CentralPanel::default()
            .frame(egui::Frame::default().inner_margin(egui::Margin::symmetric(16, 12)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                        ui.heading("wakeupLLM");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add(egui::Button::new("🔄").frame(false)).clicked() {
                            self.refresh();
                        }
                    });
                });
                ui.add_space(4.0);
                ui.colored_label(egui::Color32::from_rgb(180, 180, 180), &self.message);
                ui.add_space(12.0);

                let pending = Arc::new(Mutex::new(None::<(String, Option<String>)>));
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for model in &self.models {
                        let model_id = model.id.clone();
                        let pending_clone = Arc::clone(&pending);
                        let on_profile_change = move |new_profile: Option<&str>| {
                            let profile_owned = new_profile.map(|s| s.to_string());
                            pending_clone
                                .lock()
                                .unwrap()
                                .replace((model_id.clone(), profile_owned));
                        };
                        model_card(
                            ui,
                            model,
                            &self.models_cfg,
                            &self.pm,
                            &self.warming_ports,
                            &self.repaint,
                            on_profile_change,
                        );
                        ui.add_space(6.0);
                    }
                });

                {
                    let mut guard = pending.lock().unwrap();
                    if let Some((model_id, new_profile)) = guard.take() {
                        drop(guard);
                        match new_profile {
                            Some(ref p) => {
                                self.profile_selections.insert(model_id.clone(), p.clone());
                            }
                            None => {
                                self.profile_selections.remove(&model_id);
                            }
                        }
                        save_profile_selection(
                            &model_id,
                            new_profile.as_deref(),
                            &mut self.profile_selections,
                        );
                        self.refresh();
                    }
                }
            });

    }
}

impl Drop for WakeupApp {
    fn drop(&mut self) {
        log!("App closing, unloading all model processes...");
        self.pm.lock().unwrap().kill_all();
    }
}

fn get_arg_value(args: &[String], name: &str) -> Option<String> {
    args.windows(2).find(|w| w[0] == name).and_then(|w| w.get(1).cloned())
}

fn has_arg(args: &[String], name: &str) -> bool {
    args.iter().any(|a| a == name)
}

fn badge(ui: &mut egui::Ui, text: &str, color: egui::Color32, tooltip: &str) {
    let resp = ui.add(
        egui::Label::new(
            egui::RichText::new(text).size(10.0).color(color).weak(),
        )
        .sense(egui::Sense::click()),
    );
    resp.on_hover_text(tooltip);
}

fn model_card(
    ui: &mut egui::Ui,
    model: &ModelDisplay,
    models_cfg: &[ModelEntry],
    pm: &Arc<Mutex<ProcessManager>>,
    warming_ports: &Arc<Mutex<HashSet<u16>>>,
    repaint: &Arc<AtomicBool>,
    on_profile_change: impl Fn(Option<&str>),
) {
    let (status_text, status_color, dot_char) = if model.owned {
        ("Ready", egui::Color32::GREEN, "●")
    } else if model.external {
        ("External", egui::Color32::YELLOW, "●")
    } else if model.warming {
        ("Warming", egui::Color32::YELLOW, "◐")
    } else {
        ("Offline", egui::Color32::GRAY, "○")
    };

    egui::Frame::NONE
        .inner_margin(egui::Margin::symmetric(12, 10))
        .corner_radius(6)
        .show(ui, |ui| {
            // ── Title line ──────────────────────────────────────────────
            ui.horizontal(|ui| {
                ui.colored_label(
                    dot_char
                        .chars()
                        .next()
                        .map_or(egui::Color32::GRAY, |_| status_color),
                    dot_char,
                );
                ui.add_space(4.0);
                ui.label(egui::RichText::new(&model.display_name).size(14.0).strong());
                if !model.alias.is_empty() {
                    ui.colored_label(
                        egui::Color32::from_gray(140),
                        format!("({})", model.alias),
                    );
                }
                if model.vision {
                    ui.colored_label(egui::Color32::from_rgb(60, 140, 80), "📷");
                }
                ui.colored_label(status_color, format!(" {}", status_text));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.colored_label(egui::Color32::GRAY, format!(":{}", model.port));
                });
            });

            // ── Badges line ─────────────────────────────────────────────
            let mut has_badges = false;
            ui.horizontal(|ui| {
                if model.ctx_size > 0 {
                    let ctx_k = model.ctx_size / 1024;
                    badge(ui, &format!("{}K ctx", ctx_k), egui::Color32::from_rgb(70, 130, 180), "Context window size");
                    has_badges = true;
                }
                if model.vision {
                    badge(ui, "Vision", egui::Color32::from_rgb(60, 140, 80), "Multimodal / vision support");
                    has_badges = true;
                }
                if model.flash_attn {
                    badge(ui, "FlashAttn", egui::Color32::from_rgb(180, 130, 50), "Flash attention enabled");
                    has_badges = true;
                }
                if model.is_moe {
                    badge(ui, "MoE", egui::Color32::from_rgb(130, 100, 180), "Mixture of Experts");
                    has_badges = true;
                }
                if has_badges {
                    ui.add_space(0.0);
                }
            });

            // ── Action buttons ──────────────────────────────────────────
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let can_warmup = !model.owned && !model.external && !model.warming;
                if ui
                    .add_enabled(can_warmup, egui::Button::new("🔥 Warmup"))
                    .clicked()
                {
                    let pm = Arc::clone(pm);
                    let port = model.port;
                    let wports = Arc::clone(warming_ports);
                    let rp = Arc::clone(repaint);
                    wports.lock().unwrap().insert(port);
                    rp.store(true, Ordering::SeqCst);
                    thread::spawn(move || {
                        log!("[gui] Warmup port {port}");
                        let _ = pm.lock().unwrap().start(port);
                        wports.lock().unwrap().remove(&port);
                        rp.store(true, Ordering::SeqCst);
                    });
                }
                if ui
                    .add_enabled(
                        model.owned || model.external,
                        egui::Button::new(if model.external {
                            "Unload external"
                        } else {
                            "Unload"
                        }),
                    )
                    .clicked()
                {
                    let pm = Arc::clone(pm);
                    let port = model.port;
                    let rp = Arc::clone(repaint);
                    thread::spawn(move || {
                        log!("[gui] Unload port {port}");
                        match pm.lock().unwrap().stop(port) {
                            Ok(msg) => log!("[gui] {msg}"),
                            Err(e) => log!("[gui] Unload failed port {port}: {e}"),
                        }
                        rp.store(true, Ordering::SeqCst);
                    });
                }

                // ── Profile selector + params ───────────────────────────
                if !model.profiles.is_empty() {
                    ui.add_space(16.0);
                    let current_selection = model
                        .selected_profile
                        .clone()
                        .unwrap_or_else(|| model.profiles.first().cloned().unwrap_or_default());
                    let mut selected = current_selection.clone();
                    let combo = egui::ComboBox::from_id_salt(format!("profile_{}", model.port))
                        .selected_text(&selected);
                    let changed = combo
                        .show_ui(ui, |ui| {
                            for profile in &model.profiles {
                                if ui
                                    .selectable_value(&mut selected, profile.clone(), profile)
                                    .clicked()
                                {
                                    ui.close_menu();
                                }
                            }
                        })
                        .inner;
                    if changed.is_some() && selected != current_selection {
                        on_profile_change(Some(selected.as_str()));
                    }
                }
            });

            // ── Profile / default params ────────────────────────────────
            let mut parts: Vec<String> = Vec::new();
            if !model.profiles.is_empty() {
                let current = model.selected_profile.as_deref()
                    .or_else(|| model.profiles.first().map(|s| s.as_str()));
                if let Some(profile_name) = current {
                    let cfg_entry = models_cfg.iter().find(|m| m.id == model.id);
                    if let Some(entry) = cfg_entry {
                        if let Some(p) = entry.profiles.iter().find(|p| p.name == profile_name) {
                            if let Some(t) = p.temperature {
                                parts.push(format!("temp={}", t));
                            }
                            if let Some(t) = p.top_p {
                                parts.push(format!("top_p={}", t));
                            }
                            if let Some(t) = p.top_k {
                                parts.push(format!("top_k={}", t));
                            }
                            if let Some(t) = p.presence_penalty {
                                parts.push(format!("pp={}", t));
                            }
                        }
                    }
                }
            } else {
                let cfg_entry = models_cfg.iter().find(|m| m.id == model.id);
                if let Some(entry) = cfg_entry {
                    if let Some(t) = get_arg_value(&entry.arguments, "--temp").and_then(|v| v.parse::<f32>().ok()) {
                        parts.push(format!("temp={}", t));
                    }
                    if let Some(t) = get_arg_value(&entry.arguments, "--top-p").and_then(|v| v.parse::<f32>().ok()) {
                        parts.push(format!("top_p={}", t));
                    }
                    if let Some(t) = get_arg_value(&entry.arguments, "--top-k").and_then(|v| v.parse::<u32>().ok()) {
                        parts.push(format!("top_k={}", t));
                    }
                    if let Some(t) = get_arg_value(&entry.arguments, "--presence-penalty").and_then(|v| v.parse::<f32>().ok()) {
                        parts.push(format!("pp={}", t));
                    }
                }
            }
            if !parts.is_empty() {
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    ui.add_space(12.0);
                    ui.colored_label(
                        egui::Color32::from_gray(140),
                        parts.join(" · "),
                    );
                });
            }
        });
    ui.separator();
}

// ── Main ────────────────────────────────────────────────────────────────────

fn main() {
    let cfg = AppConfig::load().expect("Cannot load model-config.json");
    log!(
        "Config loaded: {} models, out_port={:?}, idle={}s",
        cfg.models.len(),
        cfg.out_port,
        cfg.idle_timeout_seconds
    );
    let out_port = cfg.out_port.unwrap_or(8000);
    let idle_secs = cfg.idle_timeout_seconds;

    let pm = Arc::new(Mutex::new(ProcessManager::new(idle_secs, &cfg.models)));
    let cfg = Arc::new(cfg);

    // Start proxy
    let pm_proxy = Arc::clone(&pm);
    let cfg_proxy = Arc::clone(&cfg);
    thread::spawn(move || start_proxy(pm_proxy, cfg_proxy, out_port));

    // Start idle monitor
    let pm_idle = Arc::clone(&pm);
    thread::spawn(move || start_idle_monitor(pm_idle));

    let models_cfg = cfg.models.clone();
    let quit = Arc::new(AtomicBool::new(false));
    let idle_mins = idle_secs / 60;
    let warming_ports = Arc::new(Mutex::new(HashSet::new()));
    let repaint = Arc::new(AtomicBool::new(false));
    let profile_selections = load_profile_selections();
    let auto_warm_models = load_running_models();

    // Tray icon with right-click menu (raw Win32 on Windows)
    let icon_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("icons/icon.ico");

    // Channel for tray thread -> main thread communication
    let (tray_tx, tray_rx) = std::sync::mpsc::channel::<TrayAction>();
    let tray_tx_clone = tray_tx.clone();
    thread::spawn(move || {
        #[cfg(windows)]
        unsafe {
            // Get module handle for hInstance
            let hinstance = GetModuleHandleW(std::ptr::null());

            // Load icon (must be .ico file for LoadImageW with IMAGE_ICON + LR_LOADFROMFILE)
            let hicon: *mut std::ffi::c_void = LoadImageW(
                std::ptr::null_mut(),
                to_wide(icon_path.to_str().unwrap()).as_ptr(),
                1, // IMAGE_ICON
                64, 64,
                0x00000010, // LR_LOADFROMFILE
            ) as *mut _;
            if hicon.is_null() {
                log!("LoadImageW failed, GetLastError={}", GetLastError());
                return;
            }

            // Register window class
            let class_name = to_wide("wakeupLLM_TrayClass");
            let wc = WNDCLASSW {
                lpfnWndProc: Some(tray_wnd_proc),
                hInstance: hinstance,
                lpszClassName: class_name.as_ptr(),
                ..std::mem::zeroed()
            };
            if RegisterClassW(&wc) == 0 {
                log!("RegisterClassW failed, GetLastError={}", GetLastError());
                return;
            }

            // Create hidden window
            let hwnd = CreateWindowExW(
                0,
                class_name.as_ptr(),
                std::ptr::null(),
                WS_OVERLAPPED,
                0, 0, 0, 0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                hinstance,
                std::ptr::null_mut(),
            );
            if hwnd.is_null() {
                log!("CreateWindowExW failed, GetLastError={}", GetLastError());
                return;
            }

            // Store channel sender in window user data
            let sender_box = Box::new(tray_tx_clone);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(sender_box) as _);

            // Add tray icon
            let mut nid = NOTIFYICONDATAW {
                cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
                hWnd: hwnd,
                uID: 1,
                uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
                uCallbackMessage: WM_USER + 1, // Custom message for tray events
                hIcon: hicon,
                szTip: [0; 128],
                ..std::mem::zeroed()
            };
            let tip = to_wide("wakeupLLM");
            for (i, &c) in tip.iter().enumerate().take(127) {
                nid.szTip[i] = c;
            }
            if Shell_NotifyIconW(NIM_ADD, &mut nid) == 0 {
                log!("Shell_NotifyIconW NIM_ADD failed");
                return;
            }

            log!("Tray icon created (raw Win32)");

            // Message loop
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }

            // Cleanup
            Shell_NotifyIconW(NIM_DELETE, &mut nid);
            DestroyWindow(hwnd);
        }
    });

    // Move tray_rx to main thread for handling in update()
    let tray_rx_main = tray_rx;

    let mut app = WakeupApp {
        models: Vec::new(),
        message: format!("Proxy on :{out_port}"),
        proxy_port: out_port,
        pm: Arc::clone(&pm),
        models_cfg,
        quit: Arc::clone(&quit),
        idle_timeout_mins: idle_mins,
        warming_ports: Arc::clone(&warming_ports),
        repaint: Arc::clone(&repaint),
        profile_selections,
        auto_warm_models: auto_warm_models.clone(),
        tray_rx: std::sync::mpsc::channel().1, // dummy, will be replaced in closure
    };
    app.refresh();

    for model_id in auto_warm_models.iter() {
        if !app
            .models
            .iter()
            .any(|m| m.id == *model_id && (m.owned || m.external))
        {
            app.warmup_model(model_id);
        }
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([440.0, 580.0])
            .with_min_inner_size([360.0, 420.0])
            .with_icon(std::sync::Arc::new(
                eframe::icon_data::from_png_bytes(APP_ICON_PNG)
                    .unwrap_or(egui::IconData {
                        rgba: vec![0; 16],
                        width: 2,
                        height: 2,
                    }),
            )),
        ..Default::default()
    };

    eframe::run_native(
        "wakeupLLM",
        options,
        Box::new(move |_cc| {
            Ok(Box::new(WakeupApp {
                models: app.models.clone(),
                message: app.message.clone(),
                proxy_port: app.proxy_port,
                pm: Arc::clone(&pm),
                models_cfg: app.models_cfg.clone(),
                quit: Arc::clone(&quit),
                idle_timeout_mins: idle_mins,
                warming_ports: Arc::clone(&warming_ports),
                repaint: Arc::clone(&repaint),
                profile_selections: app.profile_selections.clone(),
                auto_warm_models: app.auto_warm_models.clone(),
                tray_rx: tray_rx_main,
            }))
        }),
    )
    .expect("eframe failed");
}
