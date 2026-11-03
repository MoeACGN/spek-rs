#[derive(Clone, Debug)]
pub struct CpuInfo {
    pub performance_cores: usize,
    pub logical: usize,
    /// True when the processor reports more than one efficiency class.
    pub hybrid: bool,
    pub counted_physical: bool,
}

pub fn detect() -> CpuInfo {
    let logical = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .max(1);
    if let Some((cores, hybrid)) = physical_performance_cores() {
        CpuInfo {
            performance_cores: cores.max(1).min(logical.max(cores)),
            logical: logical.max(cores.max(1)),
            hybrid,
            counted_physical: true,
        }
    } else {
        CpuInfo {
            performance_cores: logical,
            logical,
            hybrid: false,
            counted_physical: false,
        }
    }
}

fn physical_performance_cores() -> Option<(usize, bool)> {
    #[cfg(windows)]
    {
        windows_performance_cores()
    }
    #[cfg(not(windows))]
    {
        linux_performance_cores()
    }
}

#[cfg(not(windows))]
fn linux_performance_cores() -> Option<(usize, bool)> {
    let mut rows = Vec::new();
    for index in 0..4096 {
        let base = format!("/sys/devices/system/cpu/cpu{index}");
        if !std::path::Path::new(&base).exists() {
            if index == 0 {
                continue;
            }
            break;
        }
        let Some(core_id) = read_u32(&format!("{base}/topology/core_id")) else {
            continue;
        };
        let package = read_u32(&format!("{base}/topology/physical_package_id")).unwrap_or(0);
        let capacity = read_u32(&format!("{base}/cpu_capacity")).unwrap_or(1024);
        rows.push((capacity, package, core_id));
    }
    if rows.is_empty() {
        return None;
    }
    let best = rows.iter().map(|row| row.0).max().unwrap_or(0);
    let mut classes = std::collections::BTreeSet::new();
    let mut cores = std::collections::BTreeSet::new();
    for (capacity, package, core_id) in &rows {
        classes.insert(*capacity);
        if *capacity == best {
            cores.insert((*package, *core_id));
        }
    }
    Some((cores.len().max(1), classes.len() > 1))
}

#[cfg(not(windows))]
fn read_u32(path: &str) -> Option<u32> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| text.trim().parse().ok())
}

#[cfg(windows)]
fn windows_performance_cores() -> Option<(usize, bool)> {
    use std::os::raw::c_void;
    #[link(name = "kernel32")]
    extern "system" {
        fn GetLogicalProcessorInformationEx(
            relationship: u32,
            buffer: *mut c_void,
            returned: *mut u32,
        ) -> i32;
    }
    const RELATION_PROCESSOR_CORE: u32 = 0;
    let mut bytes: u32 = 0;
    unsafe {
        GetLogicalProcessorInformationEx(RELATION_PROCESSOR_CORE, std::ptr::null_mut(), &mut bytes);
    }
    if bytes < 8 {
        return None;
    }
    let mut buffer = vec![0u8; bytes as usize];
    let ok = unsafe {
        GetLogicalProcessorInformationEx(
            RELATION_PROCESSOR_CORE,
            buffer.as_mut_ptr() as *mut c_void,
            &mut bytes,
        )
    };
    if ok == 0 {
        return None;
    }
    buffer.truncate(bytes as usize);
    let mut offset = 0usize;
    let mut classes = Vec::new();
    while offset + 8 <= buffer.len() {
        let relationship = u32::from_le_bytes(buffer[offset..offset + 4].try_into().ok()?);
        let size = u32::from_le_bytes(buffer[offset + 4..offset + 8].try_into().ok()?) as usize;
        if size < 8 || offset + size > buffer.len() {
            break;
        }
        // PROCESSOR_RELATIONSHIP starts after the 8-byte header.
        // BYTE Flags; BYTE EfficiencyClass; ...
        if relationship == RELATION_PROCESSOR_CORE && size >= 10 {
            classes.push(buffer[offset + 9]);
        }
        offset += size;
    }
    if classes.is_empty() {
        return None;
    }
    let best = *classes.iter().max().unwrap_or(&0);
    let hybrid = classes.iter().any(|class| *class != best);
    let count = classes.iter().filter(|class| **class == best).count();
    Some((count.max(1), hybrid))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_at_least_one_core() {
        let cpu = detect();
        assert!(cpu.performance_cores >= 1);
        assert!(cpu.logical >= cpu.performance_cores || !cpu.counted_physical || cpu.logical >= 1);
        assert!(cpu.logical >= 1);
    }
}
