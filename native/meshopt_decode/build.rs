fn main() {
    println!("cargo:rerun-if-changed=vendor");
    // The codec sources use C++ syntax and the C runtime.
    cc::Build::new()
        .cpp(true)
        .cpp_link_stdlib(None)
        .flag_if_supported("-std=c++11")
        .files([
            "vendor/indexcodec.cpp",
            "vendor/vertexcodec.cpp",
            "vendor/vertexfilter.cpp",
        ])
        .compile("vtx_meshopt_decode");
}
