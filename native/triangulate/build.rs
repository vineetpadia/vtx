fn main() {
    println!("cargo:rerun-if-changed=../../VERSION");
    println!("cargo:rerun-if-env-changed=VTX_VERSION");
    println!("cargo:rerun-if-env-changed=VTX_UPDATE_REPO");
    println!("cargo:rerun-if-env-changed=VTX_UPDATE_PRIVATE");
    let version = std::env::var("VTX_VERSION")
        .unwrap_or_else(|_| std::fs::read_to_string("../../VERSION").expect("VTX version"));
    let version = version.trim().trim_start_matches('v');
    assert!(
        version.split('.').count() == 3
            && version
                .split('.')
                .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit())),
        "VTX_VERSION must be major.minor.patch"
    );
    println!("cargo:rustc-env=VTX_VERSION={version}");
    println!(
        "cargo:rustc-env=VTX_UPDATE_REPO={}",
        std::env::var("VTX_UPDATE_REPO").unwrap_or_else(|_| "vineetpadia/vtx".into())
    );
    println!(
        "cargo:rustc-env=VTX_UPDATE_PRIVATE={}",
        std::env::var("VTX_UPDATE_PRIVATE").unwrap_or_else(|_| "true".into())
    );
    println!("cargo:rerun-if-changed=../../assets/vtx.ico");
    #[cfg(windows)]
    if std::env::var_os("CARGO_FEATURE_DESKTOP").is_some() {
        let root = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
        let icon = root
            .join("../../assets/vtx.ico")
            .canonicalize()
            .expect("VTX icon asset");
        let resource =
            std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("vtx.rc");
        let icon = icon
            .to_string_lossy()
            .trim_start_matches(r"\\?\")
            .replace('\\', "/");
        std::fs::write(&resource, format!("1 ICON \"{icon}\"\n")).expect("Write VTX icon resource");
        embed_resource::compile_for_examples(resource, embed_resource::NONE)
            .manifest_required()
            .expect("Compile VTX Windows icon");
    }
}
