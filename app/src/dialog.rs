use std::path::PathBuf;

pub fn pick_open() -> Option<PathBuf> {
    dialog(false, "開啟音訊檔", "spek")
}

pub fn pick_save(suggested: &str) -> Option<PathBuf> {
    dialog(true, "另存頻譜圖", suggested)
}

#[cfg(windows)]
fn dialog(save: bool, title: &str, suggested: &str) -> Option<PathBuf> {
    use std::ffi::{c_void, OsString};
    use std::os::windows::ffi::{OsStrExt, OsStringExt};

    #[repr(C)]
    struct OpenFileNameW {
        struct_size: u32,
        hwnd_owner: *mut c_void,
        instance: *mut c_void,
        filter: *const u16,
        custom_filter: *mut u16,
        max_custom_filter: u32,
        filter_index: u32,
        file: *mut u16,
        max_file: u32,
        file_title: *mut u16,
        max_file_title: u32,
        initial_dir: *const u16,
        title: *const u16,
        flags: u32,
        file_offset: u16,
        file_extension: u16,
        def_ext: *const u16,
        cust_data: usize,
        hook: *mut c_void,
        template_name: *const u16,
        reserved: *mut c_void,
        reserved_flags: u32,
        flags_ex: u32,
    }

    #[link(name = "comdlg32")]
    extern "system" {
        fn GetOpenFileNameW(ofn: *mut OpenFileNameW) -> i32;
        fn GetSaveFileNameW(ofn: *mut OpenFileNameW) -> i32;
    }

    const OFN_OVERWRITEPROMPT: u32 = 0x2;
    const OFN_NOCHANGEDIR: u32 = 0x8;
    const OFN_PATHMUSTEXIST: u32 = 0x800;
    const OFN_FILEMUSTEXIST: u32 = 0x1000;
    const OFN_EXPLORER: u32 = 0x80000;

    fn wide(text: &str) -> Vec<u16> {
        OsString::from(text).encode_wide().chain(Some(0)).collect()
    }

    let filter_text = "音訊\0*.wav;*.flac;*.mp3;*.ogg;*.opus;*.m4a;*.aac;*.wma;*.ape;*.wv;*.ac3;*.dts;*.mpc;*.aiff;*.aif;*.caf;*.mka;*.mkv;*.tta;*.tak;*.mp2;*.m4b\0PNG\0*.png\0所有檔案\0*.*\0";
    let filter: Vec<u16> = filter_text.encode_utf16().chain([0, 0]).collect();
    let title = wide(title);
    let def_ext = wide("png");
    let mut file = vec![0u16; 4096];
    let stem: Vec<u16> = OsString::from(suggested).encode_wide().take(4000).collect();
    file[..stem.len()].copy_from_slice(&stem);

    let mut ofn = OpenFileNameW {
        struct_size: 0,
        hwnd_owner: std::ptr::null_mut(),
        instance: std::ptr::null_mut(),
        filter: filter.as_ptr(),
        custom_filter: std::ptr::null_mut(),
        max_custom_filter: 0,
        filter_index: if save { 2 } else { 1 },
        file: file.as_mut_ptr(),
        max_file: file.len() as u32,
        file_title: std::ptr::null_mut(),
        max_file_title: 0,
        initial_dir: std::ptr::null(),
        title: title.as_ptr(),
        flags: OFN_EXPLORER
            | OFN_NOCHANGEDIR
            | OFN_PATHMUSTEXIST
            | if save {
                OFN_OVERWRITEPROMPT
            } else {
                OFN_FILEMUSTEXIST
            },
        file_offset: 0,
        file_extension: 0,
        def_ext: if save {
            def_ext.as_ptr()
        } else {
            std::ptr::null()
        },
        cust_data: 0,
        hook: std::ptr::null_mut(),
        template_name: std::ptr::null(),
        reserved: std::ptr::null_mut(),
        reserved_flags: 0,
        flags_ex: 0,
    };
    ofn.struct_size = std::mem::size_of::<OpenFileNameW>() as u32;

    let ok = unsafe {
        if save {
            GetSaveFileNameW(&mut ofn)
        } else {
            GetOpenFileNameW(&mut ofn)
        }
    };
    if ok == 0 {
        return None;
    }
    let end = file.iter().position(|unit| *unit == 0).unwrap_or(file.len());
    Some(PathBuf::from(OsString::from_wide(&file[..end])))
}

#[cfg(not(windows))]
fn dialog(save: bool, title: &str, suggested: &str) -> Option<PathBuf> {
    let mut command = std::process::Command::new("zenity");
    command.arg("--file-selection").arg(format!("--title={title}"));
    if save {
        command
            .arg("--save")
            .arg("--confirm-overwrite")
            .arg(format!("--filename={suggested}"));
    }
    let output = command.output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let path = text.trim();
    if path.is_empty() {
        None
    } else {
        Some(PathBuf::from(path))
    }
}
