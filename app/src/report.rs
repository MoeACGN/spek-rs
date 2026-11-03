use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

static LOGS: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

pub fn install() {
    let paths = log_paths();
    for path in &paths {
        let _ = std::fs::write(path, "");
    }
    if let Ok(mut guard) = LOGS.lock() {
        *guard = paths;
    }
    let _ = log::set_logger(&LOGGER);
    log::set_max_level(log::LevelFilter::Info);
    std::panic::set_hook(Box::new(|info| {
        let message = format!("Spek 發生錯誤，程式無法繼續：\n{info}");
        note(&message);
        popup(&message);
    }));
    #[cfg(windows)]
    install_seh();
    note("Spek 1.5.0 啟動");
}

pub fn note(message: &str) {
    let line = format!("{message}\r\n");
    let Ok(paths) = LOGS.lock() else {
        return;
    };
    for path in paths.iter() {
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
            let _ = file.write_all(line.as_bytes());
            let _ = file.flush();
        }
    }
}

pub fn popup(message: &str) {
    note(message);
    #[cfg(windows)]
    message_box(message);
    #[cfg(not(windows))]
    eprintln!("{message}");
}

fn log_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            paths.push(dir.join("spek.log"));
        }
    }
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        let dir = PathBuf::from(local).join("Spek");
        let _ = std::fs::create_dir_all(&dir);
        paths.push(dir.join("spek.log"));
    }
    paths
}

struct Logger;

static LOGGER: Logger = Logger;

impl log::Log for Logger {
    fn enabled(&self, _: &log::Metadata<'_>) -> bool {
        true
    }

    fn log(&self, record: &log::Record<'_>) {
        let text = format!("{}", record.args());
        note(&format!("[{}] {text}", record.level()));
        if record.level() <= log::Level::Error && fatal_log(&text) {
            #[cfg(windows)]
            message_box(&format!("Spek 無法繼續：\n{text}"));
        }
    }

    fn flush(&self) {}
}

fn fatal_log(text: &str) -> bool {
    text.contains("Exiting because of error")
        || text.contains("Failed to initialize")
        || text.contains("Failed to create window")
}

#[cfg(windows)]
fn message_box(message: &str) {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    let wide: Vec<u16> = OsStr::new(message).encode_wide().chain(Some(0)).collect();
    let title: Vec<u16> = OsStr::new("Spek").encode_wide().chain(Some(0)).collect();
    unsafe {
        MessageBoxW(std::ptr::null_mut(), wide.as_ptr(), title.as_ptr(), 0x10);
    }
}

#[cfg(windows)]
fn install_seh() {
    unsafe {
        SetUnhandledExceptionFilter(Some(seh_handler));
    }
}

#[cfg(windows)]
unsafe extern "system" fn seh_handler(info: *mut ExceptionPointers) -> i32 {
    let code = if info.is_null() || (*info).record.is_null() {
        0
    } else {
        (*(*info).record).code
    };
    popup(&format!(
        "Spek 在啟動時發生系統例外 0x{code:08X}。\n詳情已寫入 spek.exe 旁邊的 spek.log。"
    ));
    1
}

#[cfg(windows)]
#[repr(C)]
struct ExceptionRecord {
    code: u32,
    flags: u32,
    next: *mut ExceptionRecord,
    address: *mut std::ffi::c_void,
    parameters: u32,
    info: [usize; 15],
}

#[cfg(windows)]
#[repr(C)]
struct ExceptionPointers {
    record: *mut ExceptionRecord,
    context: *mut std::ffi::c_void,
}

#[cfg(windows)]
#[link(name = "user32")]
unsafe extern "system" {
    fn MessageBoxW(hwnd: *mut std::ffi::c_void, text: *const u16, caption: *const u16, kind: u32) -> i32;
}

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn SetUnhandledExceptionFilter(
        filter: Option<unsafe extern "system" fn(*mut ExceptionPointers) -> i32>,
    ) -> Option<unsafe extern "system" fn(*mut ExceptionPointers) -> i32>;
}
