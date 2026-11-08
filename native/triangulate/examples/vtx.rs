#![cfg_attr(windows, windows_subsystem = "windows")]

mod desktop;
mod dxf_drawing;
mod fbx_mesh;
mod gltf_mesh;
mod ifc_mesh;
mod iges;
mod obj;
mod ply;
mod solidworks;
mod three_mf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    desktop::run()
}
