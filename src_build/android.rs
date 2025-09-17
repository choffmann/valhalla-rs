use miniserde::{Deserialize, json};
use std::{env, fs, io, path::{Path, PathBuf}};

pub fn build() {
    let build_type = match (env::var("PROFILE").as_deref(), env::var("DEBUG").as_deref()) {
        (Ok("debug"), _) => "Debug",
        (Ok("release"), Ok("true")) => "RelWithDebInfo",
        _ => "Release",
    };

    let target = env::var("TARGET").unwrap_or_default();
    let triple_us = target.replace('-', "_");
    let abi = target_to_abi(&target).map(|s| s.to_string());

    // ---------- helpers ----------
    fn first_env(names: &[String]) -> Option<String> {
        for k in names {
            if let Ok(v) = std::env::var(k) {
                if !v.is_empty() { return Some(v); }
            }
        }
        None
    }
    fn keys(base: &str, triple_us: &str) -> Vec<String> {
        vec![format!("{base}_{triple_us}"), base.to_string()]
    }
    fn dir_of(p: &str) -> String {
        Path::new(p).parent().unwrap_or(Path::new(p)).display().to_string()
    }
    #[derive(Clone, Copy)]
    enum LibKind { Static, Dylib }
    fn lib_kind(path: &Path) -> Option<LibKind> {
        match path.extension().and_then(|e| e.to_str()) {
            Some("a") => Some(LibKind::Static),
            Some("so") => Some(LibKind::Dylib),
            _ => None,
        }
    }
    fn stem_from_lib(path: &Path) -> Option<String> {
        let fname = path.file_name()?.to_string_lossy();
        let s = fname.strip_prefix("lib")?;
        if let Some(t) = s.strip_suffix(".a")  { return Some(t.to_string()); }
        if let Some(t) = s.strip_suffix(".so") { return Some(t.to_string()); }
        None
    }
    fn find_lib_with_prefix(dir: &Path, prefix: &str) -> Option<PathBuf> {
        let prefer_dynamic = env::var("ANDROID_PREFER_DYNAMIC").ok().as_deref() == Some("1");
        let mut a: Option<PathBuf> = None;
        let mut so: Option<PathBuf> = None;
        if let Ok(rd) = fs::read_dir(dir) {
            for e in rd.flatten() {
                let p = e.path();
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if !name.starts_with(&format!("lib{prefix}")) { continue; }
                match p.extension().and_then(|e| e.to_str()) {
                    Some("a")  => a = Some(p),
                    Some("so") => so = Some(p),
                    _ => {}
                }
            }
        }
        if prefer_dynamic { so.or(a) } else { a.or(so) }
    }
    fn add_rerun_env(var: &str) { println!("cargo:rerun-if-env-changed={var}"); }

    fn target_to_abi(target: &str) -> Option<&'static str> {
        match target {
            t if t.contains("armv7-linux-androideabi") => Some("armeabi-v7a"),
            t if t.contains("aarch64-linux-android")   => Some("arm64-v8a"),
            t if t.contains("i686-linux-android")      => Some("x86"),
            t if t.contains("x86_64-linux-android")    => Some("x86_64"),
            _ => None,
        }
    }

    fn normalize_android_platform(raw: &str) -> Option<String> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return None;
        }
        if let Some(rest) = trimmed.strip_prefix("android-") {
            if rest.chars().all(|c| c.is_ascii_digit()) {
                return Some(trimmed.to_string());
            }
        }
        if let Some(rest) = trimmed.strip_prefix("android") {
            if rest.chars().all(|c| c == '-' || c.is_ascii_digit()) {
                let mut normalized = String::from("android-");
                normalized.push_str(rest.trim_start_matches('-'));
                return Some(normalized);
            }
        }
        if trimmed.chars().all(|c| c.is_ascii_digit()) {
            return Some(format!("android-{trimmed}"));
        }
        None
    }

    fn find_toolchain_from_compiler(raw: &str) -> Option<PathBuf> {
        for token in raw.split_whitespace() {
            let trimmed = token.trim_matches(|c| c == '"' || c == '\'');
            if trimmed.is_empty() { continue; }
            let mut current = PathBuf::from(trimmed);
            loop {
                let candidate = current.join("build").join("cmake").join("android.toolchain.cmake");
                if candidate.exists() {
                    return Some(candidate);
                }
                if !current.pop() {
                    break;
                }
            }
        }
        None
    }

    fn remove_path_if_exists(path: &Path) -> io::Result<()> {
        match fs::symlink_metadata(path) {
            Ok(meta) => {
                if meta.is_dir() {
                    fs::remove_dir_all(path)
                } else {
                    fs::remove_file(path)
                }
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(err),
        }
    }

    fn copy_dir_recursive(src: &Path, dst: &Path) -> io::Result<()> {
        fs::create_dir_all(dst)?;
        for entry in fs::read_dir(src)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            let src_path = entry.path();
            let dst_path = dst.join(entry.file_name());
            if file_type.is_dir() {
                copy_dir_recursive(&src_path, &dst_path)?;
            } else if file_type.is_symlink() {
                let meta = fs::metadata(&src_path)?;
                if meta.is_dir() {
                    remove_path_if_exists(&dst_path)?;
                    copy_dir_recursive(&src_path, &dst_path)?;
                } else {
                    remove_path_if_exists(&dst_path)?;
                    fs::copy(&src_path, &dst_path)?;
                }
            } else {
                remove_path_if_exists(&dst_path)?;
                fs::copy(&src_path, &dst_path)?;
            }
        }
        Ok(())
    }

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR unset"));
    let valhalla_src = out_dir.join("valhalla-src");
    if valhalla_src.exists() {
        fs::remove_dir_all(&valhalla_src).expect("remove stale valhalla copy");
    }
    copy_dir_recursive(Path::new("valhalla"), &valhalla_src).expect("copy valhalla sources");

    let boost_base = env::var("BOOST_BASE").ok();
    let proto_base = env::var("PROTO_BASE").ok();
    let lz4_base   = env::var("LZ4_BASE").ok();
    let protoc_arg = env::var("PROTOC").ok();

    struct Mapped {
        boost_root: Option<String>,
        boost_inc:  Option<String>,
        boost_lib:  Option<String>,
        pb_dir:     Option<String>, // cmake package dir
        pb_inc:     Option<String>,
        pb_lib:     Option<String>,
        pb_protoc:  Option<String>,
        lz4_dir:    Option<String>,
        lz4_inc:    Option<String>,
        lz4_lib:    Option<String>,
        cmake_prefix_extra: Vec<String>,
    }

    let mapped = (|| -> Mapped {
        let mut out = Mapped {
            boost_root: None, boost_inc: None, boost_lib: None,
            pb_dir: None, pb_inc: None, pb_lib: None, pb_protoc: None,
            lz4_dir: None, lz4_inc: None, lz4_lib: None,
            cmake_prefix_extra: vec![],
        };
        let Some(abi) = abi.as_deref() else { return out };

        if let Some(base) = &boost_base {
            let root = format!("{}/{}", base, abi);
            out.boost_root = Some(root.clone());
            out.boost_inc  = Some(format!("{root}/include"));
            out.boost_lib  = Some(format!("{root}/lib"));
            out.cmake_prefix_extra.push(root);
        }
        if let Some(base) = &proto_base {
            let root = format!("{}/{}", base, abi);
            out.pb_inc = Some(format!("{root}/include"));
            out.pb_dir = Some(format!("{root}/lib/cmake/protobuf"));
            let lite = format!("{root}/lib/libprotobuf-lite.a");
            let full = format!("{root}/lib/libprotobuf.a");
            out.pb_lib = Some(if Path::new(&lite).exists() { lite } else { full });
            out.cmake_prefix_extra.push(root);
        }
        if let Some(base) = &lz4_base {
            let root = format!("{}/{}", base, abi);
            out.lz4_dir = Some(root.clone());
            out.lz4_inc = Some(format!("{root}/include"));
            out.lz4_lib = Some(format!("{root}/lib/liblz4.a"));
        }
        out.pb_protoc = protoc_arg.clone();
        out
    })();

    let toolchain_file = (|| -> Option<PathBuf> {
        if let Some(path) = first_env(&keys("CMAKE_TOOLCHAIN_FILE", &triple_us)) {
            let candidate = PathBuf::from(&path);
            if candidate.exists() {
                return Some(candidate);
            }
        }

        let ndk_env_keys = [
            "CARGO_NDK_CMAKE_TOOLCHAIN_PATH",
            "CARGO_NDK_ANDROID_NDK_HOME",
            "ANDROID_NDK_HOME",
            "ANDROID_NDK_ROOT",
            "ANDROID_NDK",
            "NDK_HOME",
            "NDK_ROOT",
        ];
        for key in ndk_env_keys {
            if let Ok(dir) = env::var(key) {
                if dir.is_empty() { continue; }
                let path = Path::new(&dir);
                let candidate = if path.ends_with("android.toolchain.cmake") {
                    path.to_path_buf()
                } else {
                    path.join("build").join("cmake").join("android.toolchain.cmake")
                };
                if candidate.exists() {
                    return Some(candidate);
                }
            }
        }

        let cc_keys = [
            format!("CMAKE_C_COMPILER_{}", triple_us),
            format!("CC_{}", triple_us),
            format!("CARGO_TARGET_{}_CC", triple_us.to_uppercase()),
            "CMAKE_C_COMPILER".into(),
            "CC".into(),
        ];
        for key in cc_keys {
            if let Ok(value) = env::var(&key) {
                if let Some(candidate) = find_toolchain_from_compiler(&value) {
                    return Some(candidate);
                }
            }
        }

        None
    })();

    // ---------- finale Inputs ----------
    let boost_root = first_env(&keys("Boost_ROOT", &triple_us)).or(mapped.boost_root);
    let boost_inc  = first_env(&keys("Boost_INCLUDE_DIR", &triple_us)).or(mapped.boost_inc);
    let boost_lib  = first_env(&keys("Boost_LIBRARY_DIR", &triple_us)).or(mapped.boost_lib);

    let pb_dir     = first_env(&keys("Protobuf_DIR", &triple_us)).or(mapped.pb_dir);
    let pb_inc     = first_env(&keys("Protobuf_INCLUDE_DIR", &triple_us)).or(mapped.pb_inc);
    let pb_lib     = first_env(&keys("Protobuf_LIBRARY", &triple_us))
                        .or_else(|| first_env(&keys("Protobuf_LIBRARIES", &triple_us)))
                        .or(mapped.pb_lib);
    let pb_protoc  = first_env(&vec!["Protobuf_PROTOC_EXECUTABLE".into(), "PROTOC".into()])
                        .or(mapped.pb_protoc);

    let pb_component = env::var("PROTOBUF_COMPONENT").ok().unwrap_or_else(|| {
        if target.contains("android") { "protobuf-lite".into() } else { "protobuf".into() }
    });

    let lz4_dir    = first_env(&keys("LZ4_DIR", &triple_us)).or(mapped.lz4_dir);
    let lz4_inc    = first_env(&keys("LZ4_INCLUDE_DIR", &triple_us))
                        .or(lz4_dir.as_ref().map(|d| format!("{d}/include")))
                        .or(mapped.lz4_inc);
    let lz4_lib    = first_env(&keys("LZ4_LIBRARY", &triple_us))
                        .or(lz4_dir.as_ref().map(|d| format!("{d}/lib/liblz4.a")))
                        .or(mapped.lz4_lib);

    // compose CMAKE_PREFIX_PATH (env + Mapping)
    let mut cmake_prefix: Vec<String> = env::var(format!("CMAKE_PREFIX_PATH_{triple_us}"))
        .or_else(|_| env::var("CMAKE_PREFIX_PATH"))
        .unwrap_or_default()
        .split(':')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect();
    if let Some(br) = &boost_root { cmake_prefix.push(br.clone()); }
    if let Some(pd) = &pb_dir     { cmake_prefix.push(pd.clone()); }
    cmake_prefix.extend(mapped.cmake_prefix_extra.into_iter());

    // ---------- CMake ----------
    let mut cfg = cmake::Config::new(&valhalla_src);
    cfg.define("CMAKE_BUILD_TYPE", build_type)
        .define("CMAKE_EXPORT_COMPILE_COMMANDS", "ON")
        .define("ENABLE_TOOLS", "OFF")
        .define("ENABLE_DATA_TOOLS", "OFF")
        .define("ENABLE_SERVICES", "OFF")
        .define("ENABLE_HTTP", "OFF")
        .define("ENABLE_PYTHON_BINDINGS", "OFF")
        .define("ENABLE_TESTS", "OFF")
        .define("ENABLE_GDAL", "OFF")
        .define("ENABLE_SINGLE_FILES_WERROR", "OFF")
        .define("ENABLE_THREAD_SAFE_TILE_REF_COUNT", "ON")
        .define("LOGGING_LEVEL", "WARN")
        .define("Boost_NO_SYSTEM_PATHS", "ON");

    if let Some(toolchain) = toolchain_file.as_ref() {
        if let Some(path) = toolchain.to_str() {
            cfg.define("CMAKE_TOOLCHAIN_FILE", path);
        }
    }

    if let Some(abi) = abi.as_deref() {
        cfg.define("ANDROID_ABI", abi);
    }

    if let Some(platform) = first_env(&vec![
        format!("ANDROID_PLATFORM_{triple_us}"),
        "CARGO_NDK_ANDROID_PLATFORM".into(),
        "ANDROID_PLATFORM".into(),
    ]).and_then(|value| normalize_android_platform(&value)) {
        cfg.define("ANDROID_PLATFORM", platform);
    }

    if !cmake_prefix.is_empty() {
        cfg.define("CMAKE_PREFIX_PATH", cmake_prefix.join(":"));
    }
    if let Some(r) = &boost_root { cfg.define("Boost_ROOT", r); }
    if let Some(i) = &boost_inc  { cfg.define("Boost_INCLUDE_DIR", i); }
    if let Some(l) = &boost_lib  { cfg.define("Boost_LIBRARY_DIR", l); }

    if let Some(i) = &pb_inc     { cfg.define("Protobuf_INCLUDE_DIR", i); }
    if let Some(l) = &pb_lib     { cfg.define("Protobuf_LIBRARY", l); }
    if let Some(p) = &pb_protoc  {
        cfg.define("Protobuf_PROTOC_EXECUTABLE", p);
        cfg.define("PROTOBUF_PROTOC_EXECUTABLE", p);
    }

    if let Some(li) = &lz4_inc {
        cfg.define("CMAKE_REQUIRED_INCLUDES", li);
        cfg.cflag(format!("-I{li}"));
        cfg.cxxflag(format!("-I{li}"));
    }

    let dst = cfg.build_target("valhalla").build();
    let _ = fs::remove_file(valhalla_src.join("third_party/tz/leapseconds"));

    let valhalla_includes = extract_includes(&dst.join("build/compile_commands.json"), "config.cc");

    // ---------- Linker ----------
    let dst_s = dst.display().to_string();
    println!("cargo:rustc-link-search={dst_s}/build/src/");
    println!("cargo:rustc-link-lib=static=valhalla");

    if let Some(bl) = &boost_lib {
        println!("cargo:rustc-link-search=native={bl}");
        let bdir = Path::new(bl);
        for comp in ["filesystem","system","regex","date_time","chrono","thread"] {
            if let Some(p) = find_lib_with_prefix(bdir, &format!("boost_{comp}")) {
                if let (Some(kind), Some(stem)) = (lib_kind(&p), stem_from_lib(&p)) {
                    match kind {
                        LibKind::Static => println!("cargo:rustc-link-lib=static={stem}"),
                        LibKind::Dylib  => println!("cargo:rustc-link-lib={stem}"),
                    }
                }
            } else {
                println!("cargo:rustc-link-lib=boost_{comp}");
            }
        }
    }

    // LZ4
    match (&lz4_lib, &lz4_dir) {
        (Some(path), _) => {
            let p = Path::new(path);
            if p.exists() {
                println!("cargo:rustc-link-search=native={}", dir_of(path));
                if let (Some(kind), Some(stem)) = (lib_kind(p), stem_from_lib(p)) {
                    match kind {
                        LibKind::Static => println!("cargo:rustc-link-lib=static={stem}"),
                        LibKind::Dylib  => println!("cargo:rustc-link-lib={stem}"),
                    }
                }
            } else {
                println!("cargo:warning=LZ4_LIBRARY set but file not found: {path}");
            }
        }
        (None, Some(dir)) => {
            let libdir = Path::new(dir).join("lib");
            println!("cargo:rustc-link-search=native={}", libdir.display());
            if let Some(p) = find_lib_with_prefix(&libdir, "lz4") {
                if let (Some(kind), Some(stem)) = (lib_kind(&p), stem_from_lib(&p)) {
                    match kind {
                        LibKind::Static => println!("cargo:rustc-link-lib=static={stem}"),
                        LibKind::Dylib  => println!("cargo:rustc-link-lib={stem}"),
                    }
                }
            } else {
                println!("cargo:rustc-link-lib=lz4");
            }
        }
        (None, None) => println!("cargo:rustc-link-lib=lz4"),
    }

    // Protobuf
    if let Some(file) = &pb_lib {
        let p = Path::new(file);
        println!("cargo:rustc-link-search=native={}", dir_of(file));
        if p.exists() {
            if let (Some(kind), Some(stem)) = (lib_kind(p), stem_from_lib(p)) {
                match kind {
                    LibKind::Static => println!("cargo:rustc-link-lib=static={stem}"),
                    LibKind::Dylib  => println!("cargo:rustc-link-lib={stem}"),
                }
            }
        } else {
            println!("cargo:rustc-link-lib={}", pb_component);
        }
    } else if let Some(dir) = &pb_dir {
        let libdir = Path::new(dir).join("lib");
        println!("cargo:rustc-link-search=native={}", libdir.display());
        if let Some(p) = find_lib_with_prefix(&libdir, &pb_component) {
            if let (Some(kind), Some(stem)) = (lib_kind(&p), stem_from_lib(&p)) {
                match kind {
                    LibKind::Static => println!("cargo:rustc-link-lib=static={stem}"),
                    LibKind::Dylib  => println!("cargo:rustc-link-lib={stem}"),
                }
            }
        } else {
            println!("cargo:rustc-link-lib={}", pb_component);
        }
    } else {
        println!("cargo:rustc-link-lib={}", pb_component);
    }

    let cxx_stdlib = env::var("CXX_STDLIB").ok().unwrap_or_else(|| {
        if target.contains("android")      { "c++_shared".into() }
        else if target.contains("apple")   { "c++".into() }
        else                               { "stdc++".into() }
    });
    println!("cargo:rustc-link-lib={cxx_stdlib}");
    println!("cargo:rustc-link-lib=z");

    if target.contains("armv7") || target.contains("androideabi") {
        println!("cargo:rustc-link-lib=atomic");
    }

    // ---------- cxx bridge ----------
    cxx_build::bridges(["src/lib.rs", "src/config.rs", "src/actor.rs"])
        .file("src/libvalhalla.cpp")
        .file(valhalla_src.join("src/baldr/datetime.cc"))
        .std("c++17")
        .includes(valhalla_includes)
        .define("ENABLE_THREAD_SAFE_TILE_REF_COUNT", None)
        .compile("libvalhalla-cxxbridge");

    println!("cargo:rerun-if-changed=src/actor.hpp");
    println!("cargo:rerun-if-changed=src/config.hpp");
    println!("cargo:rerun-if-changed=src/libvalhalla.hpp");
    println!("cargo:rerun-if-changed=src/libvalhalla.cpp");
    println!("cargo:rerun-if-changed=src/lib.rs");
    println!("cargo:rerun-if-changed=valhalla");

    for k in [
        "Boost_ROOT","Boost_INCLUDE_DIR","Boost_LIBRARY_DIR",
        "Protobuf_DIR","Protobuf_INCLUDE_DIR","Protobuf_LIBRARY","Protobuf_LIBRARIES",
        "Protobuf_PROTOC_EXECUTABLE","PROTOC","PROTOBUF_COMPONENT",
        "LZ4_DIR","LZ4_INCLUDE_DIR","LZ4_LIBRARY",
        "CMAKE_TOOLCHAIN_FILE","CMAKE_PREFIX_PATH","CXX_STDLIB","ANDROID_PREFER_DYNAMIC",
        "ANDROID_ABI","ANDROID_PLATFORM",
        "BOOST_BASE","PROTO_BASE","LZ4_BASE",
    ] {
        add_rerun_env(k);
        add_rerun_env(&format!("{k}_{triple_us}"));
    }

    let proto_dir = valhalla_src.join("proto");
    let proto_files: Vec<_> = fs::read_dir(&proto_dir)
        .expect("Failed to read valhalla/proto")
        .map(|e| e.expect("Bad fs entry").path())
        .filter(|p| p.extension().map(|e| e == "proto").unwrap_or(false))
        .collect();
    prost_build::compile_protos(&proto_files, &[proto_dir])
        .expect("Failed to compile proto files");
}

#[derive(Deserialize)]
struct CompileCommand { command: String, file: String }

fn extract_includes(compile_commands: &Path, cpp_source: &str) -> Vec<String> {
    assert!(compile_commands.exists(), "compile_commands.json not found");
    let content = fs::read_to_string(compile_commands).expect("read compile_commands.json");
    let commands: Vec<CompileCommand> = json::from_str(&content).expect("parse compile_commands.json");
    let command = commands.into_iter()
        .find(|cmd| cmd.file.ends_with(cpp_source))
        .expect("reference cpp not found in compile_commands.json");

    let args: Vec<&str> = command.command.split_whitespace().collect();
    let mut includes = Vec::new();
    for i in 0..args.len() {
        if let Some(rest) = args[i].strip_prefix("-I") {
            includes.push(rest.to_string());
        } else if args[i] == "-isystem" && i + 1 < args.len() {
            includes.push(args[i + 1].to_string());
        }
    }
    includes
}
