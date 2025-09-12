#[path = "src_build/android.rs"] mod android;
#[path = "src_build/desktop.rs"] mod desktop;

fn main() {
    let target = std::env::var("TARGET").expect("TARGET unset");
    if target.contains("android") {
        android::build();
    } else {
        desktop::build();
    }

    println!("cargo:rerun-if-changed=src_build/android.rs");
    println!("cargo:rerun-if-changed=src_build/desktop.rs");
    println!("cargo:rerun-if-changed=src_build/common.rs");
    println!("cargo:rerun-if-changed=valhalla");
    println!("cargo:rerun-if-changed=src/libvalhalla.cpp");
}

