fn main() {
    let target = std::env::var("TARGET").unwrap_or_default();
    if !target.contains("windows") {
        return;
    }
    let out_dir = std::env::var("OUT_DIR").unwrap();
    let bin = "/opt/toolchains/llvm-mingw-20261020-ucrt-ubuntu-22.04-x86_64/bin";
    let clang = format!("{bin}/x86_64-w64-mingw32-clang");
    let windres = format!("{bin}/x86_64-w64-mingw32-windres");

    let res = format!("{out_dir}/spek-res.o");
    let status = std::process::Command::new(&windres)
        .args(["-O", "coff", "-i", "spek.rc", "-o", &res])
        .status()
        .expect("windres");
    if !status.success() {
        panic!("windres failed");
    }

    let load_cfg = format!("{out_dir}/win-load-config.o");
    let status = std::process::Command::new(&clang)
        .args([
            "-c",
            "-O2",
            "-std=c11",
            "win-load-config.c",
            "-o",
            &load_cfg,
        ])
        .status()
        .expect("clang");
    if !status.success() {
        panic!("win-load-config compile failed");
    }

    println!("cargo:rustc-link-arg={load_cfg}");
    println!("cargo:rustc-link-arg={res}");
    println!("cargo:rustc-link-arg=-Wl,-subsystem,windows:10.0");
    println!("cargo:rerun-if-changed=spek.rc");
    println!("cargo:rerun-if-changed=spek.manifest");
    println!("cargo:rerun-if-changed=assets/spek.ico");
    println!("cargo:rerun-if-changed=win-load-config.c");
}
