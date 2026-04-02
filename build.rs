fn main() {
    println!("cargo:rerun-if-changed=src/customlabels.c");
    println!("cargo:rerun-if-changed=src/customlabels.h");
    println!("cargo:rerun-if-changed=./dlist");

    cc::Build::new()
        .file("src/customlabels.c")
        .compile("customlabels");

    println!("cargo:rustc-link-lib=static=customlabels");

    // dynamic-list is Linux-only
    #[cfg(target_os = "linux")]
    println!("cargo:rustc-link-arg=-Wl,--dynamic-list=./dlist");

    // Generate bindings using bindgen
    let out_path = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());

    let bindings = bindgen::Builder::default()
        .header("src/customlabels.h")
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .clang_arg("-D__bindgen")
        .generate()
        .expect("Unable to generate bindings");
    bindings
        .write_to_file(out_path.join("bindings.rs"))
        .expect("Couldn't write bindings!");
}
