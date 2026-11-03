fn main() {
    let target = std::env::var("TARGET").unwrap_or_default();
    let windows = target.contains("windows");
    let prefix = if windows {
        "/opt/prefix/ffmpeg-win"
    } else if std::path::Path::new("/opt/prefix/ffmpeg-host-full/lib/libavcodec.a").exists() {
        "/opt/prefix/ffmpeg-host-full"
    } else {
        "/opt/prefix/ffmpeg-host"
    };

    let mut build = cc::Build::new();
    build.file("src/ffmpeg_decode.c");
    build.include(format!("{prefix}/include"));
    build.flag("-std=c11");
    build.opt_level(2);
    if windows {
        let bin = "/opt/toolchains/llvm-mingw-20261020-ucrt-ubuntu-22.04-x86_64/bin";
        build.compiler(format!("{bin}/x86_64-w64-mingw32-clang"));
        build.archiver(format!("{bin}/x86_64-w64-mingw32-llvm-ar"));
        build.define("_WIN32_WINNT", "0x0A00");
        build.define("WINVER", "0x0A00");
    }
    build.compile("spek_ffmpeg");

    println!("cargo:rustc-link-search=native={prefix}/lib");
    let swr = std::path::Path::new(prefix).join("lib/libswresample.a");
    println!("cargo:rustc-link-lib=static=avformat");
    println!("cargo:rustc-link-lib=static=avcodec");
    if swr.exists() {
        println!("cargo:rustc-link-lib=static=swresample");
    }
    println!("cargo:rustc-link-lib=static=avutil");
    println!("cargo:rustc-link-lib=static=avcodec");
    println!("cargo:rustc-link-lib=static=avformat");
    println!("cargo:rustc-link-lib=static=avutil");
    if windows {
        println!("cargo:rustc-link-search=native=/opt/prefix/zlib-win/lib");
        println!("cargo:rustc-link-lib=static=z");
        println!("cargo:rustc-link-lib=bcrypt");
        println!("cargo:rustc-link-lib=user32");
        println!("cargo:rustc-link-lib=ole32");
    } else {
        println!("cargo:rustc-link-lib=m");
        println!("cargo:rustc-link-lib=pthread");
        println!("cargo:rustc-link-lib=atomic");
        println!("cargo:rustc-link-lib=dl");
    }
    println!("cargo:rerun-if-changed=src/ffmpeg_decode.c");
    println!("cargo:rerun-if-env-changed=TARGET");
}
