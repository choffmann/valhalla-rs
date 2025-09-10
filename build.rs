use miniserde::{Deserialize, json};
use std::{env, fs, path::{Path, PathBuf}};

fn main() {
    let build_type = match (env::var("PROFILE").as_deref(), env::var("DEBUG").as_deref()) {
        (Ok("debug"), _) => "Debug",
        (Ok("release"), Ok("true")) => "RelWithDebInfo",
        _ => "Release",
    };

    let target = env::var("TARGET").unwrap_or_default();
    let triple_us = target.replace('-', "_");

    // -------- Helpers --------------------------------------------------------
    fn first_env(names: &[String]) -> Option<String> {
        for k in names {
            if let Ok(v) = env::var(k) {
                if !v.is_empty() { return Some(v); }
            }
        }
        None
    }
    fn keys(base: &str, triple_us: &str) -> Vec<String> {
        vec![
            format!("{base}_{triple_us}"),
            base.to_string(),
        ]
    }
    fn dir_of(p: &str) -> String {
        Path::new(p).parent().unwrap_or(Path::new(p)).display().to_string()
    }
    fn stem_from_lib(path: &Path) -> Option<String> {
        let fname = path.file_name()?.to_string_lossy();
        // expect "lib<stem>.a" or "lib<stem>.so"
        let s = fname.strip_prefix("lib")?;
        if let Some(t) = s.strip_suffix(".a") { return Some(t.to_string()); }
        if let Some(t) = s.strip_suffix(".so") { return Some(t.to_string()); }
        None
    }
    fn find_lib_with_prefix(dir: &Path, prefix: &str) -> Option<PathBuf> {
        // Pick static .a if available, otherwise .so
        let mut best_a: Option<PathBuf> = None;
        let mut best_so: Option<PathBuf> = None;
        if let Ok(rd) = fs::read_dir(dir) {
            for e in rd.flatten() {
                let p = e.path();
                if !p.file_name().and_then(|n| n.to_str()).unwrap_or("").starts_with(&format!("lib{prefix}")) {
                    continue;
                }
                if p.extension().and_then(|e| e.to_str()) == Some("a") { best_a = Some(p.clone()); }
                if p.extension().and_then(|e| e.to_str()) == Some("so") { best_so = Some(p); }
            }
        }
        best_a.or(best_so)
    }
    fn add_rerun_env(var: &str) { println!("cargo:rerun-if-env-changed={var}"); }

    // -------- Read environment (with target-specific fallbacks) --------------
    let boost_root = first_env(&keys("Boost_ROOT", &triple_us));
    let boost_inc  = first_env(&keys("Boost_INCLUDE_DIR", &triple_us));
    let boost_lib  = first_env(&keys("Boost_LIBRARY_DIR", &triple_us));

    let pb_dir  = first_env(&keys("Protobuf_DIR", &triple_us));
    let pb_inc  = first_env(&keys("Protobuf_INCLUDE_DIR", &triple_us));
    let pb_lib  = first_env(&keys("Protobuf_LIBRARY", &triple_us))
        .or_else(|| first_env(&keys("Protobuf_LIBRARIES", &triple_us)));
    let pb_protoc = first_env(&vec!["Protobuf_PROTOC_EXECUTABLE".into(), "PROTOC".into()]);

    let lz4_dir = first_env(&keys("LZ4_DIR", &triple_us));
    let lz4_inc = first_env(&keys("LZ4_INCLUDE_DIR", &triple_us))
        .or_else(|| lz4_dir.as_ref().map(|d| format!("{d}/include")));
    let lz4_lib = first_env(&keys("LZ4_LIBRARY", &triple_us))
        .or_else(|| lz4_dir.as_ref().map(|d| format!("{d}/lib/liblz4.a")));

    // optional: extend CMAKE_PREFIX_PATH from env plus detected prefixes
    let mut cmake_prefix: Vec<String> = env::var(format!("CMAKE_PREFIX_PATH_{triple_us}"))
        .or_else(|_| env::var("CMAKE_PREFIX_PATH"))
        .unwrap_or_default()
        .split(':')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect();

    if let Some(br) = &boost_root { cmake_prefix.push(br.clone()); }
    if let Some(pd) = &pb_dir     { cmake_prefix.push(pd.clone()); }

    // -------- CMake configuration for Valhalla -------------------------------
    let mut cfg = cmake::Config::new("valhalla");
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
        .define("Boost_USE_STATIC_LIBS", "ON")
        .define("Boost_NO_SYSTEM_PATHS", "ON")
        .define("Protobuf_USE_STATIC_LIBS", "ON");

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

    // Build Valhalla static libs via CMake
    let dst = cfg.build_target("valhalla").build();

    // Clean up temporary tz file (Valhalla quirk)
    let _ = fs::remove_file("valhalla/third_party/tz/leapseconds");

    // Extract include dirs for cxx bridge
    let valhalla_includes = extract_includes(&dst.join("build/compile_commands.json"), "config.cc");

    // -------- Linker configuration (manual, robust) --------------------------
    // Valhalla static artifacts
    let dst_s = dst.display().to_string();
    println!("cargo:rustc-link-search={dst_s}/build/src/");

    // Boost (auto-detect suffixed names produced by Boost-for-Android)
    if let Some(bl) = &boost_lib {
        println!("cargo:rustc-link-search=native={bl}");
        let bdir = Path::new(bl);

        // Prefer static libs; fall back to shared if only .so present.
        let components = ["filesystem", "system", "regex", "date_time", "chrono", "thread"];
        for comp in components {
            if let Some(p) = find_lib_with_prefix(bdir, &format!("boost_{comp}")) {
                if let Some(stem) = stem_from_lib(&p) {
                    println!("cargo:rustc-link-lib=static={stem}");
                }
            } else {
                // Fallback to generic name (works if generic symlink exists)
                println!("cargo:rustc-link-lib=static=boost_{comp}");
            }
        }
    }

    // LZ4
    if let Some(ll) = &lz4_lib {
        let p = Path::new(ll);
        if p.exists() {
            println!("cargo:rustc-link-search=native={}", dir_of(ll));
            let stem = stem_from_lib(p).unwrap_or_else(|| "lz4".into());
            // Most builds produce liblz4.a, so:
            println!("cargo:rustc-link-lib=static={stem}");
        } else if let Some(ld) = &lz4_dir {
            println!("cargo:rustc-link-search=native={ld}/lib");
            println!("cargo:rustc-link-lib=static=lz4");
        }
    } else if let Some(ld) = &lz4_dir {
        println!("cargo:rustc-link-search=native={ld}/lib");
        println!("cargo:rustc-link-lib=static=lz4");
    } else {
        // As a last resort: rely on pkg-config / system — typically not available in cross.
        println!("cargo:warning=LZ4 not explicitly provided; make sure the linker can find it.");
    }

    // Protobuf (lite)
    if let Some(pl) = &pb_lib {
        let p = Path::new(pl);
        if p.exists() {
            println!("cargo:rustc-link-search=native={}", dir_of(pl));
            let stem = stem_from_lib(p).unwrap_or_else(|| "protobuf-lite".into());
            println!("cargo:rustc-link-lib=static={stem}");
        } else if let Some(pd) = &pb_dir {
            println!("cargo:rustc-link-search=native={pd}/lib");
            println!("cargo:rustc-link-lib=static=protobuf-lite");
        } else if let Some(pi) = &pb_inc {
            println!("cargo:rustc-link-search=native={}", dir_of(&format!("{pi}/../lib")));
            println!("cargo:rustc-link-lib=static=protobuf-lite");
        } else {
            println!("cargo:rustc-link-lib=static=protobuf-lite");
        }
    } else {
        println!("cargo:rustc-link-lib=static=protobuf-lite");
    }

    // System libs from NDK
    let cxx_stdlib = env::var("CXX_STDLIB").unwrap_or_else(|_| "c++_shared".into());
    println!("cargo:rustc-link-lib={cxx_stdlib}");
    println!("cargo:rustc-link-lib=z");

    // ARMv7 needs -latomic in many setups
    if target.contains("armv7") || target.contains("androideabi") {
        println!("cargo:rustc-link-lib=atomic");
    }

    // -------- cxx bridge ------------------------------------------------------
    cxx_build::bridges(["src/lib.rs", "src/config.rs", "src/actor.rs"])
        .file("src/libvalhalla.cpp")
        .file("valhalla/src/baldr/datetime.cc")
        .std("c++17")
        .includes(valhalla_includes)
        .define("ENABLE_THREAD_SAFE_TILE_REF_COUNT", None)
        .compile("libvalhalla-cxxbridge");

    // Rerun triggers (sources + env)
    println!("cargo:rerun-if-changed=src/actor.hpp");
    println!("cargo:rerun-if-changed=src/config.hpp");
    println!("cargo:rerun-if-changed=src/libvalhalla.hpp");
    println!("cargo:rerun-if-changed=src/libvalhalla.cpp");
    println!("cargo:rerun-if-changed=src/lib.rs");
    println!("cargo:rerun-if-changed=valhalla");

    for k in [
        "Boost_ROOT","Boost_INCLUDE_DIR","Boost_LIBRARY_DIR",
        "Protobuf_DIR","Protobuf_INCLUDE_DIR","Protobuf_LIBRARY","Protobuf_LIBRARIES","Protobuf_PROTOC_EXECUTABLE","PROTOC",
        "LZ4_DIR","LZ4_INCLUDE_DIR","LZ4_LIBRARY",
        "CMAKE_PREFIX_PATH","CMAKE_PREFIX_PATH_","CXX_STDLIB"
    ] {
        add_rerun_env(k);
        add_rerun_env(&format!("{k}_{triple_us}"));
    }

    // ---- prost: compile Valhalla proto (host protoc used via prost) ----------
    let proto_files: Vec<_> = fs::read_dir("valhalla/proto")
        .expect("Failed to read valhalla/proto")
        .map(|e| e.expect("Bad fs entry").path())
        .filter(|p| p.extension().map(|e| e == "proto").unwrap_or(false))
        .collect();
    prost_build::compile_protos(&proto_files, &["valhalla/proto/"])
        .expect("Failed to compile proto files");
}

/// https://clang.llvm.org/docs/JSONCompilationDatabase.html
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

