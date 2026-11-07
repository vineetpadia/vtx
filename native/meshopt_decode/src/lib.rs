/// Bindings for the bundled meshoptimizer 1.3 codec.
pub mod ffi {
    use std::ffi::{c_int, c_void};

    extern "C" {
        pub fn meshopt_decodeIndexBuffer(
            destination: *mut c_void, count: usize, stride: usize, source: *const u8, size: usize,
        ) -> c_int;
        pub fn meshopt_decodeIndexSequence(
            destination: *mut c_void, count: usize, stride: usize, source: *const u8, size: usize,
        ) -> c_int;
        pub fn meshopt_decodeVertexBuffer(
            destination: *mut c_void, count: usize, stride: usize, source: *const u8, size: usize,
        ) -> c_int;
        pub fn meshopt_decodeFilterOct(buffer: *mut c_void, count: usize, stride: usize);
        pub fn meshopt_decodeFilterQuat(buffer: *mut c_void, count: usize, stride: usize);
        pub fn meshopt_decodeFilterExp(buffer: *mut c_void, count: usize, stride: usize);
        pub fn meshopt_decodeFilterColor(buffer: *mut c_void, count: usize, stride: usize);
    }
}
