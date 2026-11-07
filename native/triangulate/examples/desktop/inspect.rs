//! Reproducible, headless visual evidence for humans and multimodal agents.
use super::{
    measure::{self, Request},
    model::{self, UploadTarget},
    render::{self, Camera, Renderer},
    Result,
};
use image::{Rgba, RgbaImage};
use serde_json::json;
use std::{ffi::OsString, fs, path::PathBuf};

pub const USAGE: &str = "vtx --inspect MODEL --output DIR [--views front,back,left,right,top,bottom,iso,plan] [--size 768] [--unit model|mm|cm|m|in|ft] [--region xmin,ymin,zmin,xmax,ymax,zmax] [--section nx,ny,nz,offset] [--query view,x,y] [--distance query0,query1] [--angle query0,vertex,query2] [--diameter query0,query1,query2,...] [--measure-unit mm|cm|m|in|ft]";
const VIEWS: &[&str] = &["front", "back", "left", "right", "top", "bottom", "iso"];
const FOOTER: u32 = 132;
const INK: Rgba<u8> = Rgba([225, 235, 245, 255]);
const BG: Rgba<u8> = Rgba([12, 19, 29, 255]);

struct Options {
    model: PathBuf,
    output: PathBuf,
    views: Vec<String>,
    size: u32,
    unit: String,
    region: Option<[f32; 6]>,
    section: Option<[f32; 4]>,
    queries: Vec<(String, [u32; 2])>,
    measurements: Vec<Request>,
    measure_unit: Option<String>,
}
impl Options {
    fn parse(args: Vec<OsString>) -> Result<Self> {
        let mut args = args.into_iter();
        let mut views_explicit = false;
        let model = PathBuf::from(args.next().ok_or(USAGE)?);
        let mut options = Self {
            model,
            output: PathBuf::new(),
            views: VIEWS.iter().map(|s| s.to_string()).collect(),
            size: 768,
            unit: "model".into(),
            region: None,
            section: None,
            queries: Vec::new(),
            measurements: Vec::new(),
            measure_unit: None,
        };
        while let Some(key) = args.next() {
            let value = args.next().ok_or(USAGE)?;
            match key.to_str().ok_or(USAGE)? {
                "--output" => options.output = PathBuf::from(value),
                "--size" => options.size = value.to_str().ok_or(USAGE)?.parse()?,
                "--unit" => options.unit = value.to_str().ok_or(USAGE)?.into(),
                "--region" => options.region = Some(numbers(value.to_str().ok_or(USAGE)?)?),
                "--section" => options.section = Some(numbers(value.to_str().ok_or(USAGE)?)?),
                "--distance" | "--angle" | "--diameter" => options.measurements.push(
                    Request::parse(key.to_str().unwrap(), value.to_str().ok_or(USAGE)?)?,
                ),
                "--measure-unit" => {
                    options.measure_unit = Some(value.to_str().ok_or(USAGE)?.into())
                }
                "--query" => {
                    let fields: Vec<_> = value.to_str().ok_or(USAGE)?.split(',').collect();
                    if fields.len() != 3 {
                        return Err("Query must be view,x,y".into());
                    }
                    options
                        .queries
                        .push((fields[0].into(), [fields[1].parse()?, fields[2].parse()?]));
                }
                "--views" => {
                    views_explicit = true;
                    options.views = value
                        .to_str()
                        .ok_or(USAGE)?
                        .split(',')
                        .map(str::to_owned)
                        .collect()
                }
                _ => return Err(format!("Unknown option {:?}. {}", key, USAGE).into()),
            }
        }
        if !views_explicit
            && options
                .model
                .extension()
                .and_then(|s| s.to_str())
                .is_some_and(|s| s.eq_ignore_ascii_case("dxf") || s.eq_ignore_ascii_case("dwg"))
        {
            options.views = vec!["plan".into()];
        }
        if options.output.as_os_str().is_empty()
            || !(256..=2048).contains(&options.size)
            || options.views.is_empty()
        {
            return Err(format!(
                "Output directory required; size must be 256..2048. {}",
                USAGE
            )
            .into());
        }
        if !["model", "mm", "cm", "m", "in", "ft"].contains(&options.unit.as_str()) {
            return Err(
                "Unit must be model, mm, cm, m, in, or ft (a label override, not a conversion)"
                    .into(),
            );
        }
        let mut seen = std::collections::HashSet::new();
        for view in &options.views {
            if (!VIEWS.contains(&view.as_str()) && view != "plan") || !seen.insert(view.clone()) {
                return Err(format!("Unknown or duplicate view: {}", view).into());
            }
        }
        if let Some(r) = options.region {
            if (0..3).any(|i| r[i] > r[i + 3]) || (0..3).all(|i| r[i] == r[i + 3]) {
                return Err("Region requires ordered bounds and a nonzero extent".into());
            }
            let diagonal = (0..3)
                .map(|i| (f64::from(r[i + 3]) - f64::from(r[i])).powi(2))
                .sum::<f64>()
                .sqrt();
            if diagonal < 1e-7 || diagonal > f32::MAX as f64 {
                return Err("Region extent exceeds supported range".into());
            }
        }
        if let Some(plane) = options.section {
            let norm = plane[..3]
                .iter()
                .map(|v| f64::from(*v).powi(2))
                .sum::<f64>()
                .sqrt();
            if norm < 1e-12 {
                return Err("Section normal must be nonzero".into());
            }
            let normalized = plane.map(|v| (f64::from(v) / norm) as f32);
            if normalized.iter().any(|v| !v.is_finite()) {
                return Err("Section offset exceeds supported range".into());
            }
            options.section = Some(normalized);
        }
        for (view, [x, y]) in &options.queries {
            if !options.views.contains(view) || *x >= options.size || *y >= options.size {
                return Err(
                    "Query requires an exported view and a pixel within its model viewport".into(),
                );
            }
        }
        if options.queries.len() > 1024 {
            return Err("At most 1024 queries per inspection".into());
        }
        if options.measurements.len() > 128 {
            return Err("At most 128 measurements per inspection".into());
        }
        if options
            .measurements
            .iter()
            .any(|m| m.indices().iter().any(|&i| i >= options.queries.len()))
        {
            return Err(
                "Measurement indices refer to zero-based --query options; index is out of range"
                    .into(),
            );
        }
        if options
            .measure_unit
            .as_deref()
            .is_some_and(|s| super::units::length_factor(s).is_none())
        {
            return Err("Measurement unit must be mm, cm, m, in, or ft".into());
        }
        Ok(options)
    }
}

fn numbers<const N: usize>(value: &str) -> Result<[f32; N]> {
    let values: Vec<f32> = value
        .split(',')
        .map(str::parse)
        .collect::<std::result::Result<_, _>>()?;
    if values.len() != N || values.iter().any(|v| !v.is_finite()) {
        return Err(format!("Expected {} finite comma-separated numbers", N).into());
    }
    let mut result = [0.0; N];
    result.copy_from_slice(&values);
    Ok(result)
}

pub fn plan_camera() -> Camera {
    camera("plan")
}

fn camera(view: &str) -> Camera {
    use std::f32::consts::{FRAC_PI_2, FRAC_PI_4, PI};
    let (yaw, pitch) = match view {
        "back" => (PI, 0.0),
        "right" => (-FRAC_PI_2, 0.0),
        "left" => (FRAC_PI_2, 0.0),
        "top" => (0.0, FRAC_PI_2),
        "bottom" => (0.0, -FRAC_PI_2),
        "iso" => (-FRAC_PI_4, (1.0_f32 / 3.0_f32.sqrt()).asin()),
        _ => (0.0, 0.0),
    };
    Camera {
        yaw,
        pitch,
        zoom: 0.9,
        orthographic: true,
        target: None,
        section: None,
    }
}

/// World vectors for screen right, screen up, and from model toward camera.
fn basis(camera: &Camera) -> [[f32; 3]; 3] {
    let (sy, cy) = camera.yaw.sin_cos();
    let (sx, cx) = camera.pitch.sin_cos();
    [
        [cy, 0.0, sy],
        [sx * sy, cx, -sx * cy],
        [-cx * sy, sx, cx * cy],
    ]
}

fn nice_length(target: f64) -> f64 {
    let power = 10.0_f64.powf(target.log10().floor());
    [5.0, 2.0, 1.0]
        .iter()
        .find(|&&v| v * power <= target)
        .unwrap_or(&1.0)
        * power
}

pub fn run(args: Vec<OsString>) -> Result<()> {
    let options = Options::parse(args)?;
    // Never mix stale images with a new manifest, or overwrite an existing inspection.
    if options.output.exists() && fs::read_dir(&options.output)?.next().is_some() {
        return Err("Inspection output directory must be empty; choose a new directory".into());
    }
    let (adapter, renderer) = render::initialize(|instance| {
        let adapter = render::adapter(instance, None)?;
        let renderer = Renderer::new(&adapter, wgpu::TextureFormat::Rgba8UnormSrgb)?;
        Ok((adapter, renderer))
    })?;
    let scene = model::load(
        &options.model,
        &UploadTarget::ready(renderer.uploader.clone()),
    )?;
    let units = scene.units.overridden(&options.unit);
    measure::validate_unit(&units, options.measure_unit.as_deref())?;
    fs::create_dir_all(&options.output)?;
    let target = options.region.map(|r| {
        let center = [0, 1, 2].map(|i| r[i] * 0.5 + r[i + 3] * 0.5);
        let diameter = (0..3)
            .map(|i| (f64::from(r[i + 3]) - f64::from(r[i])).powi(2))
            .sum::<f64>()
            .sqrt() as f32;
        (center, diameter)
    });
    let frame_diameter = target.map_or(scene.diameter, |(_, d)| d);
    let spans = [0, 1, 2].map(|i| f64::from(scene.bounds[1][i]) - f64::from(scene.bounds[0][i]));
    let size = options.size;
    let unit = units.label.as_str();
    // Matches mesh.wgsl orthographic projection exactly, at a shared zoom in all views.
    let pixels_per_unit = f64::from(size) * 0.5 * 1.85 * 0.9 * 0.9 / f64::from(frame_diameter);
    let bar_length = nice_length(f64::from(size) * 0.24 / pixels_per_unit);
    let bar_pixels = (bar_length * pixels_per_unit).round() as u32;
    let columns = 3_u32.min(options.views.len() as u32);
    let rows = (options.views.len() as u32).div_ceil(columns);
    let mut sheet = RgbaImage::from_pixel(columns * size, rows * (size + FOOTER), BG);
    let mut views = Vec::new();
    let mut query_results = Vec::new();
    for (index, name) in options.views.iter().enumerate() {
        let mut camera = camera(name);
        camera.target = target;
        camera.section = options.section.map(|[x, y, z, offset]| [x, y, z, -offset]);
        let selected_queries: Vec<_> = options
            .queries
            .iter()
            .enumerate()
            .filter(|(_, (view, _))| view == name)
            .map(|(index, (_, p))| (index, *p))
            .collect();
        let coordinates: Vec<_> = selected_queries.iter().map(|(_, p)| *p).collect();
        for (query, mut hit) in selected_queries.iter().zip(render::query_pixels(
            &renderer,
            &scene,
            &camera,
            size,
            &coordinates,
        )?) {
            hit["query_index"] = json!(query.0);
            hit["view"] = json!(name);
            query_results.push(hit);
        }
        let axes = basis(&camera);
        let rendered = render::snapshot(&renderer, &scene, &camera, size, size)?;
        let mut image = RgbaImage::from_pixel(size, size + FOOTER, BG);
        image::imageops::replace(&mut image, &rendered, 0, 0);
        if !options.measurements.is_empty() {
            for &(query_index, [x, y]) in &selected_queries {
                if options
                    .measurements
                    .iter()
                    .any(|m| m.indices().contains(&query_index))
                {
                    let color = Rgba([255, 180, 70, 255]);
                    colored_line(
                        &mut image,
                        x as i32 - 4,
                        y as i32,
                        x as i32 + 4,
                        y as i32,
                        color,
                    );
                    colored_line(
                        &mut image,
                        x as i32,
                        y as i32 - 4,
                        x as i32,
                        y as i32 + 4,
                        color,
                    );
                    let label_x = (x + 7).min(size.saturating_sub(64));
                    let label_y = y.saturating_sub(18);
                    text(&mut image, label_x, label_y, &format!("Q{}", query_index));
                }
            }
        }

        text(
            &mut image,
            16,
            size + 10,
            &format!(
                "{} / ORTHO{}{}",
                name,
                if target.is_some() { " / DETAIL" } else { "" },
                if options.section.is_some() {
                    " / CUT"
                } else {
                    ""
                }
            ),
        );
        let length_label = if !(0.001..1e6).contains(&bar_length) {
            format!("{:.3e}", bar_length)
        } else {
            format!("{}", bar_length)
        };
        text(
            &mut image,
            16,
            size + 32,
            &format!("{} {}", length_label, unit),
        );
        line(&mut image, 16, size + 60, 16 + bar_pixels, size + 60);
        line(&mut image, 16, size + 55, 16, size + 65);
        line(
            &mut image,
            16 + bar_pixels,
            size + 55,
            16 + bar_pixels,
            size + 65,
        );
        // Axis triad: red X, green Y, blue Z. Same projection as the model.
        let origin = [size as i32 - 60, size as i32 + 54];
        for (axis, label, color) in [
            (0, "X", [255, 100, 100, 255]),
            (1, "Y", [100, 240, 150, 255]),
            (2, "Z", [100, 170, 255, 255]),
        ] {
            let x = origin[0] + (axes[0][axis] * 30.0).round() as i32;
            let y = origin[1] - (axes[1][axis] * 30.0).round() as i32;
            if (x - origin[0]).abs() + (y - origin[1]).abs() > 2 {
                colored_line(&mut image, origin[0], origin[1], x, y, Rgba(color));
                text(&mut image, (x + 3) as u32, (y - 5) as u32, label);
            }
        }
        text(
            &mut image,
            16,
            size + 82,
            &format!(
                "MODEL XYZ: {:.2e} {:.2e} {:.2e}",
                spans[0], spans[1], spans[2]
            ),
        );
        text(
            &mut image,
            16,
            size + 108,
            &format!("IMPORT ISSUES: {}", scene.issues),
        );
        let filename = format!("{}.png", name);
        image.save(options.output.join(&filename))?;
        image::imageops::replace(
            &mut sheet,
            &image,
            i64::from(index as u32 % columns * size),
            i64::from(index as u32 / columns * (size + FOOTER)),
        );
        views.push(json!({"name": name, "image": filename, "projection": "orthographic", "screen_right": axes[0], "screen_up": axes[1], "toward_camera": axes[2], "pixels_per_model_unit": pixels_per_unit, "scale_bar": {"length_model_units": bar_length, "pixels": bar_pixels}, "model_viewport": [0,0,size,size], "image_center_px": [size as f64/2.0,size as f64/2.0], "target_center":target.map_or(scene.center, |(c,_)| c)}));
    }
    sheet.save(options.output.join("contact-sheet.png"))?;
    query_results.sort_by_key(|q| q["query_index"].as_u64().unwrap());
    let measurements = measure::evaluate(
        &options.measurements,
        &query_results,
        &units,
        options.measure_unit.as_deref(),
    );
    let report = json!({
        "schema_version": 3, "source": fs::canonicalize(&options.model)?.to_string_lossy(),
        "detail_region":options.region,
        "section":options.section.map(|plane| json!({"plane_normal_offset":plane,"keep":"dot(normal, position) <= offset", "caps":false, "note":"Clipped surfaces are open. Orange marks intersections; no solid cap or new surface is generated."})),
        "queries":query_results,
        "source_info":scene.source_info,
        "coordinate_convention": "Y up. Front looks from +Z; right from +X; top from +Y. Source coordinates are preserved; view names do not infer semantic front.",
        "units": units,
        "measurements":measurements,
        "bounds": {"min": scene.bounds[0], "max": scene.bounds[1], "size_xyz": spans, "center": scene.center},
        "parts": scene.part_metadata,
        "geometry": {"line_segments":scene.line_segments, "triangles": scene.triangles, "points": scene.points, "import_issues": scene.issues, "notice": scene.notice},
        "limitations": ["Images show visible surfaces only; hidden and internal geometry cannot be established from these views.", "Bounds describe imported display geometry, not exact CAD dimensions. Import issues may indicate missing geometry.", "Units may be unknown or ambiguous. Measurements use visible tessellated geometry, not exact CAD topology; circle fits do not identify features automatically."],
        "gpu": adapter.get_info().name, "contact_sheet": "contact-sheet.png", "views": views
    });
    fs::write(
        options.output.join("manifest.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    // stdout is one JSON object; diagnostics stay on stderr.
    println!("{}", serde_json::to_string(&report)?);
    Ok(())
}

fn line(image: &mut RgbaImage, x0: u32, y0: u32, x1: u32, y1: u32) {
    colored_line(image, x0 as i32, y0 as i32, x1 as i32, y1 as i32, INK);
}
fn colored_line(image: &mut RgbaImage, mut x: i32, mut y: i32, x1: i32, y1: i32, color: Rgba<u8>) {
    let dx = (x1 - x).abs();
    let dy = -(y1 - y).abs();
    let sx = if x < x1 { 1 } else { -1 };
    let sy = if y < y1 { 1 } else { -1 };
    let mut error = dx + dy;
    loop {
        if x >= 0 && y >= 0 && x < image.width() as i32 && y < image.height() as i32 {
            image.put_pixel(x as u32, y as u32, color);
        }
        if x == x1 && y == y1 {
            break;
        }
        let e = 2 * error;
        if e >= dy {
            error += dy;
            x += sx;
        }
        if e <= dx {
            error += dx;
            y += sy;
        }
    }
}

// Small embedded 5x7 font: no OS fonts, font loading, or extra runtime dependencies.
fn text(image: &mut RgbaImage, x: u32, y: u32, value: &str) {
    let scale = if image.width() < 512 { 1 } else { 2 };
    for (i, c) in value.to_ascii_uppercase().chars().enumerate() {
        let rows = glyph(c);
        for (row, bits) in rows.iter().enumerate() {
            for col in 0..5 {
                if bits & (1 << (4 - col)) != 0 {
                    for dy in 0..scale {
                        for dx in 0..scale {
                            let px = x + i as u32 * 6 * scale + col * scale + dx;
                            let py = y + row as u32 * scale + dy;
                            if px < image.width() && py < image.height() {
                                image.put_pixel(px, py, INK);
                            }
                        }
                    }
                }
            }
        }
    }
}
fn glyph(c: char) -> [u8; 7] {
    match c {
        'A' => [14, 17, 17, 31, 17, 17, 17],
        'B' => [30, 17, 17, 30, 17, 17, 30],
        'C' => [14, 17, 16, 16, 16, 17, 14],
        'D' => [30, 17, 17, 17, 17, 17, 30],
        'E' => [31, 16, 16, 30, 16, 16, 31],
        'F' => [31, 16, 16, 30, 16, 16, 16],
        'G' => [14, 17, 16, 23, 17, 17, 14],
        'H' => [17, 17, 17, 31, 17, 17, 17],
        'I' => [14, 4, 4, 4, 4, 4, 14],
        'J' => [7, 2, 2, 2, 18, 18, 12],
        'K' => [17, 18, 20, 24, 20, 18, 17],
        'L' => [16, 16, 16, 16, 16, 16, 31],
        'M' => [17, 27, 21, 21, 17, 17, 17],
        'N' => [17, 25, 21, 19, 17, 17, 17],
        'O' => [14, 17, 17, 17, 17, 17, 14],
        'P' => [30, 17, 17, 30, 16, 16, 16],
        'Q' => [14, 17, 17, 17, 21, 18, 13],
        'R' => [30, 17, 17, 30, 20, 18, 17],
        'S' => [15, 16, 16, 14, 1, 1, 30],
        'T' => [31, 4, 4, 4, 4, 4, 4],
        'U' => [17, 17, 17, 17, 17, 17, 14],
        'V' => [17, 17, 17, 17, 17, 10, 4],
        'W' => [17, 17, 17, 21, 21, 21, 10],
        'X' => [17, 17, 10, 4, 10, 17, 17],
        'Y' => [17, 17, 10, 4, 4, 4, 4],
        'Z' => [31, 1, 2, 4, 8, 16, 31],
        '0' => [14, 17, 19, 21, 25, 17, 14],
        '1' => [4, 12, 4, 4, 4, 4, 14],
        '2' => [14, 17, 1, 2, 4, 8, 31],
        '3' => [30, 1, 1, 14, 1, 1, 30],
        '4' => [2, 6, 10, 18, 31, 2, 2],
        '5' => [31, 16, 16, 30, 1, 1, 30],
        '6' => [14, 16, 16, 30, 17, 17, 14],
        '7' => [31, 1, 2, 4, 8, 8, 8],
        '8' => [14, 17, 17, 14, 17, 17, 14],
        '9' => [14, 17, 17, 15, 1, 1, 14],
        '.' => [0, 0, 0, 0, 0, 12, 12],
        ':' => [0, 12, 12, 0, 12, 12, 0],
        '-' => [0, 0, 0, 31, 0, 0, 0],
        '/' => [1, 2, 2, 4, 8, 8, 16],
        _ => [0; 7],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn standard_views_are_right_handed_and_orthogonal() {
        for name in VIEWS {
            let [r, u, t] = basis(&camera(name));
            let dot = |a: [f32; 3], b: [f32; 3]| a.iter().zip(b).map(|(x, y)| x * y).sum::<f32>();
            assert!(dot(r, u).abs() < 1e-6);
            assert!((dot(t, t) - 1.0).abs() < 1e-6);
            let cross = [
                r[1] * u[2] - r[2] * u[1],
                r[2] * u[0] - r[0] * u[2],
                r[0] * u[1] - r[1] * u[0],
            ];
            assert!((dot(cross, t) - 1.0).abs() < 1e-6);
        }
        assert!((basis(&camera("top"))[1][2] + 1.0).abs() < 1e-6);
        assert!((basis(&camera("right"))[2][0] - 1.0).abs() < 1e-6);
    }
    #[test]
    fn scale_bars_are_positive_and_fit_target() {
        for target in [1e-9, 0.03, 0.9, 1.0, 4.9, 6.0, 12345.0] {
            let n = nice_length(target);
            assert!(n > 0.0 && n <= target && n >= target / 5.0);
        }
    }
    #[test]
    fn rejects_bad_arguments() {
        for args in [
            vec!["a.stl"],
            vec!["a.stl", "--output", "out", "--size", "0"],
            vec!["a.stl", "--output", "out", "--views", "front,front"],
            vec!["a.stl", "--output", "out", "--unit", "guess"],
        ] {
            assert!(Options::parse(args.into_iter().map(OsString::from).collect()).is_err());
        }
    }
}

#[cfg(test)]
mod investigation_tests {
    use super::*;
    #[test]
    fn validates_regions_sections_and_query_coordinates() {
        for flags in [
            vec!["--region", "0,0,0,-1,1,1"],
            vec!["--region", "0,0,0,0,0,0"],
            vec!["--section", "0,0,0,10"],
            vec!["--region", "0,0,0,inf,1,1"],
            vec!["--query", "front,768,0"],
            vec!["--query", "bad,0,0"],
        ] {
            let args = [vec!["box.obj", "--output", "out"], flags].concat();
            assert!(Options::parse(args.into_iter().map(OsString::from).collect()).is_err());
        }
        let args = ["drawing.dxf", "--output", "out", "--query", "plan,128,128"];
        let options = Options::parse(args.iter().map(OsString::from).collect()).unwrap();
        assert_eq!(options.views, vec!["plan"]);
        let args = ["box.obj", "--output", "out", "--section", "2,0,0,20"];
        let options = Options::parse(args.iter().map(OsString::from).collect()).unwrap();
        assert_eq!(options.section, Some([1.0, 0.0, 0.0, 10.0]));
    }
}

#[cfg(test)]
mod gpu_tests {
    use super::*;
    #[test]
    #[ignore = "requires a Vulkan/Metal/DX12 adapter"]
    fn export_known_box_at_odd_width() -> Result<()> {
        let root = std::env::temp_dir().join(format!("vtx-inspection-test-{}", std::process::id()));
        fs::create_dir_all(&root)?;
        let source = root.join("box.obj");
        let mut obj = String::new();
        for [x, y, z] in [
            [0, 0, 0],
            [40, 0, 0],
            [40, 20, 0],
            [0, 20, 0],
            [0, 0, 10],
            [40, 0, 10],
            [40, 20, 10],
            [0, 20, 10],
        ] {
            obj.push_str(&format!("v {} {} {}\n", x + 1000, y + 2000, z + 3000));
        }
        obj.push_str("f 1 4 3 2\nf 5 6 7 8\nf 1 2 6 5\nf 4 8 7 3\nf 1 5 8 4\nf 2 3 7 6\n");
        fs::write(&source, obj)?;
        let output = root.join("inspection");
        let args = vec![
            source.as_os_str().to_owned(),
            "--output".into(),
            output.as_os_str().to_owned(),
            "--size".into(),
            "257".into(),
        ];
        run(args.clone())?;
        let report: serde_json::Value =
            serde_json::from_slice(&fs::read(output.join("manifest.json"))?)?;
        assert_eq!(report["bounds"]["size_xyz"], json!([40.0, 20.0, 10.0]));
        assert_eq!(report["bounds"]["min"], json!([1000.0, 2000.0, 3000.0]));
        let ppu = report["views"][0]["pixels_per_model_unit"]
            .as_f64()
            .unwrap();
        for (view, width, height) in [
            ("front", 40.0, 20.0),
            ("right", 10.0, 20.0),
            ("top", 40.0, 10.0),
        ] {
            let image = image::open(output.join(format!("{}.png", view)))?.into_rgba8();
            assert_eq!(image.dimensions(), (257, 257 + FOOTER));
            let background = image.get_pixel(0, 0);
            let (mut lo_x, mut lo_y, mut hi_x, mut hi_y) = (257, 257, 0, 0);
            for y in 0..257 {
                for x in 0..257 {
                    if image.get_pixel(x, y) != background {
                        lo_x = lo_x.min(x);
                        lo_y = lo_y.min(y);
                        hi_x = hi_x.max(x);
                        hi_y = hi_y.max(y);
                    }
                }
            }
            assert!(
                lo_x > 0 && lo_y > 0 && hi_x < 256 && hi_y < 256,
                "clipped/empty {}",
                view
            );
            assert!(
                (f64::from(hi_x - lo_x + 1) - width * ppu).abs() <= 2.0,
                "incorrect scale {}",
                view
            );
            assert!(
                (f64::from(hi_y - lo_y + 1) - height * ppu).abs() <= 2.0,
                "incorrect height {}",
                view
            );
        }
        let renderer = render::initialize(|instance| {
            Renderer::new(
                &render::adapter(instance, None)?,
                wgpu::TextureFormat::Rgba8UnormSrgb,
            )
        })?;
        let scene = model::load(&source, &UploadTarget::ready(renderer.uploader.clone()))?;
        let mut front = camera("front");
        let hits = render::query_pixels(&renderer, &scene, &front, 257, &[[128, 128], [0, 0]])?;
        assert_eq!(hits[0]["hit"], true);
        assert_eq!(hits[1]["hit"], false);
        assert_eq!(hits[0]["part_id"], 1);
        for (axis, expected) in [1020.0, 2010.0, 3010.0].iter().enumerate() {
            assert!((hits[0]["position"][axis].as_f64().unwrap() - expected).abs() < 0.01);
        }
        assert!(hits[0]["normal"][2].as_f64().unwrap() > 0.99);
        let back = render::query_pixels(&renderer, &scene, &camera("back"), 257, &[[128, 128]])?;
        assert!((back[0]["position"][2].as_f64().unwrap() - 3000.0).abs() < 0.01);
        assert!(back[0]["normal"][2].as_f64().unwrap() < -0.99);
        front.section = Some([1.0, 0.0, 0.0, -1010.0]);
        let clipped =
            render::query_pixels(&renderer, &scene, &front, 257, &[[128, 128], [64, 128]])?;
        assert_eq!(clipped[0]["hit"], false);
        assert_eq!(clipped[1]["hit"], true);
        let cut_image = render::snapshot(&renderer, &scene, &front, 257, 257)?;
        assert_eq!(cut_image.get_pixel(128, 128), cut_image.get_pixel(0, 0));
        assert_ne!(cut_image.get_pixel(64, 128), cut_image.get_pixel(0, 0));
        front.section = None;
        front.target = Some(([1020.0, 2010.0, 3005.0], 30.0));
        let detail = render::query_pixels(&renderer, &scene, &front, 257, &[[128, 128]])?;
        assert_eq!(detail[0]["hit"], true);
        assert!((detail[0]["position"][2].as_f64().unwrap() - 3010.0).abs() < 0.01);
        assert_eq!(
            image::image_dimensions(output.join("contact-sheet.png"))?,
            (771, 3 * (257 + FOOTER))
        );
        assert!(
            run(args).is_err(),
            "must refuse to overwrite existing evidence"
        );
        // Query the actual instance transform, not just the shared local mesh.
        let instanced = root.join("instanced.gltf");
        let positions: [[f32; 3]; 3] = [[-1.0, -1.0, 0.0], [1.0, -1.0, 0.0], [0.0, 1.0, 0.0]];
        fs::write(root.join("positions.bin"), bytemuck::cast_slice(&positions))?;
        let gltf = json!({
            "asset":{"version":"2.0"}, "scene":0, "scenes":[{"nodes":[0,1]}],
            "nodes":[{"mesh":0,"translation":[-3,0,0]},{"mesh":0,"translation":[3,0,0]}],
            "meshes":[{"primitives":[{"attributes":{"POSITION":0}}]}],
            "buffers":[{"uri":"positions.bin","byteLength":36}],
            "bufferViews":[{"buffer":0,"byteLength":36}],
            "accessors":[{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3","min":[-1,-1,0],"max":[1,1,0]}]
        });
        fs::write(&instanced, serde_json::to_vec(&gltf)?)?;
        let instance_scene =
            model::load(&instanced, &UploadTarget::ready(renderer.uploader.clone()))?;
        let instance_hits = render::query_pixels(
            &renderer,
            &instance_scene,
            &camera("front"),
            257,
            &[[58, 128], [198, 128]],
        )?;
        assert_eq!(instance_hits[0]["hit"], true);
        assert_eq!(instance_hits[1]["hit"], true);
        assert!(instance_hits[0]["position"][0].as_f64().unwrap() < -2.9);
        assert!(instance_hits[1]["position"][0].as_f64().unwrap() > 2.9);
        assert_ne!(instance_hits[0]["part_id"], instance_hits[1]["part_id"]);
        // 3MF retains repeated meshes as GPU instances rather than separate draw groups.
        let three_mf = root.join("instanced.3mf");
        {
            use std::io::Write;
            let mut zip = zip::ZipWriter::new(fs::File::create(&three_mf)?);
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            zip.start_file("_rels/.rels", options)?;
            zip.write_all(r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Target="/3D/model.model" Id="rel0" Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel"/></Relationships>"#.as_bytes())?;
            zip.start_file("3D/model.model", options)?;
            zip.write_all(r#"<model unit="inch" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02"><resources><object id="1" type="model"><mesh><vertices><vertex x="-1" y="-1" z="0"/><vertex x="1" y="-1" z="0"/><vertex x="0" y="1" z="0"/></vertices><triangles><triangle v1="0" v2="1" v3="2"/></triangles></mesh></object></resources><build><item objectid="1" transform="1 0 0 0 1 0 0 0 1 -3 0 0"/><item objectid="1" transform="1 0 0 0 1 0 0 0 1 3 0 0"/></build></model>"#.as_bytes())?;
            zip.finish()?;
        }
        let gpu_instances =
            model::load(&three_mf, &UploadTarget::ready(renderer.uploader.clone()))?;
        let hits = render::query_pixels(
            &renderer,
            &gpu_instances,
            &camera("front"),
            257,
            &[[58, 128], [198, 128]],
        )?;
        assert_eq!(hits[0]["hit"], true);
        assert_eq!(hits[1]["hit"], true);
        assert_eq!(gpu_instances.units.label, "mm");
        assert_eq!(gpu_instances.units.source, "importer_normalization");
        assert!((gpu_instances.bounds[1][0] - gpu_instances.bounds[0][0] - 203.2).abs() < 0.001);
        assert_eq!(hits[0]["part_id"], hits[1]["part_id"]);
        assert_ne!(hits[0]["instance_index"], hits[1]["instance_index"]);
        assert!((hits[0]["position"][0].as_f64().unwrap() + 2.9976995 * 25.4).abs() < 0.01);
        assert!((hits[1]["position"][0].as_f64().unwrap() - 2.9976995 * 25.4).abs() < 0.01);
        let measured = root.join("measured");
        let args = vec![
            source.as_os_str().to_owned(),
            "--output".into(),
            measured.as_os_str().to_owned(),
            "--views".into(),
            "front,back".into(),
            "--size".into(),
            "257".into(),
            "--query".into(),
            "back,128,128".into(),
            "--query".into(),
            "front,128,128".into(),
            "--distance".into(),
            "0,1".into(),
            "--unit".into(),
            "mm".into(),
            "--measure-unit".into(),
            "cm".into(),
        ];
        run(args)?;
        let report: serde_json::Value =
            serde_json::from_slice(&fs::read(measured.join("manifest.json"))?)?;
        assert_eq!(report["queries"][0]["view"], "back");
        assert_eq!(report["queries"][1]["view"], "front");
        assert_eq!(report["units"]["source"], "user_override");
        assert_eq!(report["measurements"][0]["status"], "ok");
        assert!((report["measurements"][0]["value"].as_f64().unwrap() - 1.0).abs() < 1e-5);
        assert_eq!(report["measurements"][0]["unit"], "cm");
        fs::remove_dir_all(root)?;
        Ok(())
    }
}
