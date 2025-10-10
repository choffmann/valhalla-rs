use miniserde::{Deserialize, json};
use std::{
    env, fs, io,
    path::{Path, PathBuf},
    process::Command,
};

pub fn build() {
    let build_type = match (env::var("PROFILE").as_deref(), env::var("DEBUG").as_deref()) {
        (Ok("debug"), _) => "Debug",
        (Ok("release"), Ok("true")) => "RelWithDebInfo",
        _ => "Release",
    };

    let target = env::var("TARGET").expect("TARGET unset");
    let variant = target_variant(&target).expect("Unsupported Apple iOS target triple");
    let triple_us = target.replace('-', "_");
    let ios_min = env::var("IOS_DEPLOYMENT_TARGET").unwrap_or_else(|_| "13.0".into());

    let sysroot = resolve_sysroot(&variant, &triple_us)
        .unwrap_or_else(|| panic!("Failed to determine SDK path for {}", variant.sdk));
    let sysroot_s = sysroot.display().to_string();

    if env::var("SDKROOT").is_err() {
        unsafe {
            env::set_var("SDKROOT", &sysroot_s);
        }
    }

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR unset"));
    let valhalla_src = out_dir.join("valhalla-src");
    if valhalla_src.exists() {
        fs::remove_dir_all(&valhalla_src).expect("remove stale valhalla copy");
    }
    copy_dir_recursive(Path::new("valhalla"), &valhalla_src).expect("copy valhalla sources");

    let boost_base = env::var("BOOST_BASE").ok();
    let proto_base = env::var("PROTO_BASE").ok();
    let lz4_base = env::var("LZ4_BASE").ok();
    let protoc_arg = env::var("PROTOC").ok();

    struct Mapped {
        boost_root: Option<String>,
        boost_inc: Option<String>,
        boost_lib: Option<String>,
        pb_dir: Option<String>,
        pb_inc: Option<String>,
        pb_lib: Option<String>,
        pb_protoc: Option<String>,
        lz4_dir: Option<String>,
        lz4_inc: Option<String>,
        lz4_lib: Option<String>,
        cmake_prefix_extra: Vec<String>,
    }

    let mapped = (|| -> Mapped {
        let mut out = Mapped {
            boost_root: None,
            boost_inc: None,
            boost_lib: None,
            pb_dir: None,
            pb_inc: None,
            pb_lib: None,
            pb_protoc: None,
            lz4_dir: None,
            lz4_inc: None,
            lz4_lib: None,
            cmake_prefix_extra: vec![],
        };

        if let Some(base) = &boost_base {
            let root = format!("{}/{}", base, variant.flavor);
            out.boost_root = Some(root.clone());
            out.boost_inc = Some(format!("{root}/include"));
            out.boost_lib = Some(format!("{root}/lib"));
            out.cmake_prefix_extra.push(root);
        }
        if let Some(base) = &proto_base {
            let root = format!("{}/{}", base, variant.flavor);
            out.pb_inc = Some(format!("{root}/include"));
            out.pb_dir = Some(format!("{root}/lib/cmake/protobuf"));
            let full = format!("{root}/lib/libprotobuf.a");
            let lite = format!("{root}/lib/libprotobuf-lite.a");
            out.pb_lib = Some(if Path::new(&full).exists() {
                full
            } else {
                lite
            });
            out.cmake_prefix_extra.push(root);
        }
        if let Some(base) = &lz4_base {
            let root = format!("{}/{}", base, variant.flavor);
            out.lz4_dir = Some(root.clone());
            out.lz4_inc = Some(format!("{root}/include"));
            out.lz4_lib = Some(format!("{root}/lib/liblz4.a"));
        }
        out.pb_protoc = protoc_arg.clone();
        out
    })();

    let boost_root = first_env(&keys("Boost_ROOT", &triple_us)).or(mapped.boost_root);
    let boost_inc = first_env(&keys("Boost_INCLUDE_DIR", &triple_us)).or(mapped.boost_inc);
    let boost_lib = first_env(&keys("Boost_LIBRARY_DIR", &triple_us)).or(mapped.boost_lib);

    let pb_dir = first_env(&keys("Protobuf_DIR", &triple_us)).or(mapped.pb_dir);
    let pb_inc = first_env(&keys("Protobuf_INCLUDE_DIR", &triple_us)).or(mapped.pb_inc);
    let pb_lib = first_env(&keys("Protobuf_LIBRARY", &triple_us))
        .or_else(|| first_env(&keys("Protobuf_LIBRARIES", &triple_us)))
        .or(mapped.pb_lib);
    let pb_protoc =
        first_env(&vec!["Protobuf_PROTOC_EXECUTABLE".into(), "PROTOC".into()]).or(mapped.pb_protoc);

    let pb_component = env::var("PROTOBUF_COMPONENT")
        .ok()
        .unwrap_or_else(|| "protobuf".into());

    let lz4_dir = first_env(&keys("LZ4_DIR", &triple_us)).or(mapped.lz4_dir);
    let lz4_inc = first_env(&keys("LZ4_INCLUDE_DIR", &triple_us))
        .or(lz4_dir.as_ref().map(|d| format!("{d}/include")))
        .or(mapped.lz4_inc);
    let lz4_lib = first_env(&keys("LZ4_LIBRARY", &triple_us))
        .or(lz4_dir.as_ref().map(|d| format!("{d}/lib/liblz4.a")))
        .or(mapped.lz4_lib);

    let mut cmake_prefix: Vec<String> = env::var(format!("CMAKE_PREFIX_PATH_{triple_us}"))
        .or_else(|_| env::var("CMAKE_PREFIX_PATH"))
        .unwrap_or_default()
        .split(':')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect();
    if let Some(br) = &boost_root {
        cmake_prefix.push(br.clone());
    }
    if let Some(pd) = &pb_dir {
        cmake_prefix.push(pd.clone());
    }
    cmake_prefix.extend(mapped.cmake_prefix_extra.into_iter());

    let min_flag = if variant.is_simulator {
        format!("-mios-simulator-version-min={ios_min}")
    } else {
        format!("-mios-version-min={ios_min}")
    };

    let mut cfg = cmake::Config::new(&valhalla_src);
    cfg.define("CMAKE_BUILD_TYPE", build_type)
        .define("CMAKE_EXPORT_COMPILE_COMMANDS", "ON")
        .define("CMAKE_SYSTEM_NAME", "iOS")
        .define("CMAKE_OSX_ARCHITECTURES", variant.arch)
        .define("CMAKE_OSX_DEPLOYMENT_TARGET", &ios_min)
        .define("CMAKE_OSX_SYSROOT", &sysroot_s)
        .define("CMAKE_TRY_COMPILE_TARGET_TYPE", "STATIC_LIBRARY")
        .define("CMAKE_POSITION_INDEPENDENT_CODE", "ON")
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
        .define("Boost_NO_SYSTEM_PATHS", "ON")
        .cflag(&min_flag)
        .cxxflag(&min_flag)
        .cflag(format!("-isysroot {sysroot_s}"))
        .cxxflag(format!("-isysroot {sysroot_s}"))
        .cflag("-fembed-bitcode")
        .cxxflag("-fembed-bitcode");

    if !cmake_prefix.is_empty() {
        cfg.define("CMAKE_PREFIX_PATH", cmake_prefix.join(":"));
    }
    if let Some(r) = &boost_root {
        cfg.define("Boost_ROOT", r);
    }
    if let Some(i) = &boost_inc {
        cfg.define("Boost_INCLUDE_DIR", i);
    }
    if let Some(l) = &boost_lib {
        cfg.define("Boost_LIBRARY_DIR", l);
    }
    if let Some(i) = &pb_inc {
        cfg.define("Protobuf_INCLUDE_DIR", i);
    }
    if let Some(l) = &pb_lib {
        cfg.define("Protobuf_LIBRARY", l);
    }
    if let Some(p) = &pb_protoc {
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

    let dst_s = dst.display().to_string();
    println!("cargo:rustc-link-search={dst_s}/build/src/");
    println!("cargo:rustc-link-lib=static=valhalla");

    if let Some(bl) = &boost_lib {
        println!("cargo:rustc-link-search=native={bl}");
        let bdir = Path::new(bl);
        for comp in [
            "filesystem",
            "system",
            "regex",
            "date_time",
            "chrono",
            "thread",
        ] {
            if let Some(p) = find_lib_with_prefix(bdir, &format!("boost_{comp}")) {
                if let (Some(kind), Some(stem)) = (lib_kind(&p), stem_from_lib(&p)) {
                    match kind {
                        LibKind::Static => println!("cargo:rustc-link-lib=static={stem}"),
                        LibKind::Dylib => println!("cargo:rustc-link-lib={stem}"),
                    }
                }
            } else {
                if comp == "system" {
                    println!(
                        "cargo:warning=Boost system library not found under {} (skipping)",
                        bdir.display()
                    );
                } else {
                    println!("cargo:rustc-link-lib=boost_{comp}");
                }
            }
        }
    }

    println!("cargo:rustc-link-lib=framework=CoreFoundation");
    println!("cargo:rustc-link-lib=framework=Foundation");

    match (&lz4_lib, &lz4_dir) {
        (Some(path), _) => {
            let p = Path::new(path);
            println!("cargo:rustc-link-search=native={}", dir_of(path));
            if p.exists() {
                if let (Some(kind), Some(stem)) = (lib_kind(p), stem_from_lib(p)) {
                    match kind {
                        LibKind::Static => println!("cargo:rustc-link-lib=static={stem}"),
                        LibKind::Dylib => println!("cargo:rustc-link-lib={stem}"),
                    }
                }
            } else {
                println!("cargo:warning=LZ4 library not found at {path}");
            }
        }
        (None, Some(dir)) => {
            let libdir = Path::new(dir).join("lib");
            println!("cargo:rustc-link-search=native={}", libdir.display());
            if let Some(p) = find_lib_with_prefix(&libdir, "lz4") {
                if let (Some(kind), Some(stem)) = (lib_kind(&p), stem_from_lib(&p)) {
                    match kind {
                        LibKind::Static => println!("cargo:rustc-link-lib=static={stem}"),
                        LibKind::Dylib => println!("cargo:rustc-link-lib={stem}"),
                    }
                }
            } else {
                println!("cargo:rustc-link-lib=lz4");
            }
        }
        (None, None) => println!("cargo:rustc-link-lib=lz4"),
    }

    if let Some(file) = &pb_lib {
        let p = Path::new(file);
        println!("cargo:rustc-link-search=native={}", dir_of(file));
        if p.exists() {
            if let (Some(kind), Some(stem)) = (lib_kind(p), stem_from_lib(p)) {
                match kind {
                    LibKind::Static => println!("cargo:rustc-link-lib=static={stem}"),
                    LibKind::Dylib => println!("cargo:rustc-link-lib={stem}"),
                }
            }
        } else {
            println!("cargo:rustc-link-lib={pb_component}");
        }
    } else if let Some(dir) = &pb_dir {
        let libdir = Path::new(dir).join("../lib");
        println!("cargo:rustc-link-search=native={}", libdir.display());
        if let Some(p) = find_lib_with_prefix(&libdir, &pb_component) {
            if let (Some(kind), Some(stem)) = (lib_kind(&p), stem_from_lib(&p)) {
                match kind {
                    LibKind::Static => println!("cargo:rustc-link-lib=static={stem}"),
                    LibKind::Dylib => println!("cargo:rustc-link-lib={stem}"),
                }
            }
        } else {
            println!("cargo:rustc-link-lib={pb_component}");
        }
    } else {
        println!("cargo:rustc-link-lib={pb_component}");
    }

    let cxx_stdlib = env::var("CXX_STDLIB").ok().unwrap_or_else(|| "c++".into());
    println!("cargo:rustc-link-lib={cxx_stdlib}");
    println!("cargo:rustc-link-lib=z");

    let mut bridge = cxx_build::bridges(["src/lib.rs", "src/config.rs", "src/actor.rs"]);
    bridge
        .file("src/libvalhalla.cpp")
        .file(valhalla_src.join("src/baldr/datetime.cc"))
        .std("c++17")
        .includes(valhalla_includes.iter())
        .define("ENABLE_THREAD_SAFE_TILE_REF_COUNT", None)
        .flag_if_supported(&min_flag)
        .flag_if_supported("-fembed-bitcode");
    if let Some(sysroot_str) = sysroot.to_str() {
        bridge.flag_if_supported(&format!("-isysroot{sysroot_str}"));
        bridge.flag_if_supported(&format!("-isysroot {sysroot_str}"));
    }
    bridge.compile("libvalhalla-cxxbridge");

    println!("cargo:rerun-if-changed=src/actor.hpp");
    println!("cargo:rerun-if-changed=src/config.hpp");
    println!("cargo:rerun-if-changed=src/libvalhalla.hpp");
    println!("cargo:rerun-if-changed=src/libvalhalla.cpp");
    println!("cargo:rerun-if-changed=src/lib.rs");
    println!("cargo:rerun-if-changed=valhalla");

    for key in [
        "Boost_ROOT",
        "Boost_INCLUDE_DIR",
        "Boost_LIBRARY_DIR",
        "Protobuf_DIR",
        "Protobuf_INCLUDE_DIR",
        "Protobuf_LIBRARY",
        "Protobuf_LIBRARIES",
        "Protobuf_PROTOC_EXECUTABLE",
        "PROTOC",
        "PROTOBUF_COMPONENT",
        "LZ4_DIR",
        "LZ4_INCLUDE_DIR",
        "LZ4_LIBRARY",
        "CMAKE_PREFIX_PATH",
        "CXX_STDLIB",
        "IOS_DEPLOYMENT_TARGET",
        "CMAKE_OSX_SYSROOT",
        "SDKROOT",
        "BOOST_BASE",
        "PROTO_BASE",
        "LZ4_BASE",
    ] {
        add_rerun_env(key);
        add_rerun_env(&format!("{key}_{triple_us}"));
    }

    let proto_dir = valhalla_src.join("proto");
    let proto_files: Vec<_> = fs::read_dir(&proto_dir)
        .expect("Failed to read valhalla/proto")
        .map(|entry| entry.expect("Bad fs entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "proto"))
        .collect();
    prost_build::compile_protos(&proto_files, &[proto_dir]).expect("Failed to compile proto files");
}

fn first_env(names: &[String]) -> Option<String> {
    for key in names {
        if let Ok(val) = env::var(key) {
            if !val.is_empty() {
                return Some(val);
            }
        }
    }
    None
}

fn keys(base: &str, triple_us: &str) -> Vec<String> {
    vec![format!("{base}_{triple_us}"), base.to_string()]
}

fn dir_of(path: &str) -> String {
    Path::new(path)
        .parent()
        .unwrap_or_else(|| Path::new(path))
        .display()
        .to_string()
}

#[derive(Clone, Copy)]
struct TargetVariant {
    flavor: &'static str,
    sdk: &'static str,
    arch: &'static str,
    is_simulator: bool,
}

fn target_variant(target: &str) -> Option<TargetVariant> {
    match target {
        "aarch64-apple-ios" => Some(TargetVariant {
            flavor: "iphoneos-arm64",
            sdk: "iphoneos",
            arch: "arm64",
            is_simulator: false,
        }),
        "aarch64-apple-ios-sim" | "aarch64-apple-ios-simulator" => Some(TargetVariant {
            flavor: "iphonesimulator-arm64",
            sdk: "iphonesimulator",
            arch: "arm64",
            is_simulator: true,
        }),
        "x86_64-apple-ios" => Some(TargetVariant {
            flavor: "iphonesimulator-x86_64",
            sdk: "iphonesimulator",
            arch: "x86_64",
            is_simulator: true,
        }),
        _ => None,
    }
}

fn resolve_sysroot(variant: &TargetVariant, triple_us: &str) -> Option<PathBuf> {
    if let Some(path) = first_env(&keys("CMAKE_OSX_SYSROOT", triple_us))
        .or_else(|| first_env(&keys("IOS_SYSROOT", triple_us)))
        .or_else(|| env::var("CMAKE_OSX_SYSROOT").ok())
        .or_else(|| env::var("IOS_SYSROOT").ok())
    {
        let pb = PathBuf::from(path);
        if pb.exists() {
            return Some(pb);
        }
    }
    sdk_path(variant.sdk)
}

fn sdk_path(sdk: &str) -> Option<PathBuf> {
    let output = Command::new("xcrun")
        .args(["--sdk", sdk, "--show-sdk-path"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let path = String::from_utf8(output.stdout).ok()?.trim().to_string();
    if path.is_empty() {
        None
    } else {
        Some(PathBuf::from(path))
    }
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else if ty.is_symlink() {
            remove_path_if_exists(&dst_path)?;
            let meta = fs::metadata(&src_path)?;
            if meta.is_dir() {
                copy_dir_recursive(&src_path, &dst_path)?;
            } else {
                fs::copy(&src_path, &dst_path)?;
            }
        } else {
            remove_path_if_exists(&dst_path)?;
            fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
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

#[derive(Clone, Copy)]
enum LibKind {
    Static,
    Dylib,
}

fn lib_kind(path: &Path) -> Option<LibKind> {
    match path.extension().and_then(|e| e.to_str()) {
        Some("a") => Some(LibKind::Static),
        Some("dylib") => Some(LibKind::Dylib),
        Some("so") => Some(LibKind::Dylib),
        _ => None,
    }
}

fn stem_from_lib(path: &Path) -> Option<String> {
    let fname = path.file_name()?.to_string_lossy();
    let s = fname.strip_prefix("lib")?;
    if let Some(t) = s.strip_suffix(".a") {
        return Some(t.to_string());
    }
    if let Some(t) = s.strip_suffix(".dylib") {
        return Some(t.to_string());
    }
    if let Some(t) = s.strip_suffix(".so") {
        return Some(t.to_string());
    }
    None
}

fn find_lib_with_prefix(dir: &Path, prefix: &str) -> Option<PathBuf> {
    let mut static_candidate: Option<PathBuf> = None;
    let mut dynamic_candidate: Option<PathBuf> = None;
    if let Ok(rd) = fs::read_dir(dir) {
        for entry in rd.flatten() {
            let path = entry.path();
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if !name.starts_with(&format!("lib{prefix}")) {
                continue;
            }
            match path.extension().and_then(|e| e.to_str()) {
                Some("a") => static_candidate = Some(path.clone()),
                Some("dylib") | Some("so") => dynamic_candidate = Some(path.clone()),
                _ => {}
            }
        }
    }
    static_candidate.or(dynamic_candidate)
}

fn add_rerun_env(key: &str) {
    println!("cargo:rerun-if-env-changed={key}");
}

#[derive(Deserialize)]
struct CompileCommand {
    command: String,
    file: String,
}

fn extract_includes(compile_commands: &Path, cpp_source: &str) -> Vec<String> {
    assert!(compile_commands.exists(), "compile_commands.json not found");
    let content = fs::read_to_string(compile_commands).expect("read compile_commands.json");
    let commands: Vec<CompileCommand> =
        json::from_str(&content).expect("parse compile_commands.json");
    let command = commands
        .into_iter()
        .find(|cmd| cmd.file.ends_with(cpp_source))
        .expect("reference cpp source not found");

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
