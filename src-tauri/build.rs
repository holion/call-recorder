fn main() {
    #[cfg(target_os = "macos")]
    {
        // Use libtool instead of ar to create the static lib — macOS ar doesn't
        // support the --D flag that the cc crate passes, so it silently produces
        // an empty archive. libtool is the correct macOS archiver.
        let out_dir = std::env::var("OUT_DIR").unwrap();
        let obj = format!("{}/mic_permission.o", out_dir);
        let lib = format!("{}/libmic_permission.a", out_dir);

        let ok = std::process::Command::new("clang")
            .args(["-fobjc-arc", "-c", "mic_permission.m", "-o", &obj])
            .status()
            .expect("clang not found")
            .success();
        assert!(ok, "Failed to compile mic_permission.m");

        let ok = std::process::Command::new("libtool")
            .args(["-static", "-o", &lib, &obj])
            .status()
            .expect("libtool not found")
            .success();
        assert!(ok, "Failed to create libmic_permission.a");

        println!("cargo:rustc-link-lib=static=mic_permission");
        println!("cargo:rustc-link-search=native={}", out_dir);
        println!("cargo:rustc-link-lib=framework=AVFoundation");
        println!("cargo:rerun-if-changed=mic_permission.m");
    }
    // Load .env from project root so env!() macros can read secrets at compile time
    let env_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join(".env");
    if env_path.exists() {
        for item in dotenvy::from_path_iter(&env_path).unwrap() {
            let (key, val) = item.unwrap();
            println!("cargo:rustc-env={}={}", key, val);
        }
        println!("cargo:rerun-if-changed={}", env_path.display());
    }

    tauri_build::build()
}
