use std::{collections::HashMap, convert::TryInto, fs::File, io::Read, path::Path, time::Instant};

use ifc_lite_processing::{
    process, run_on_geometry_pool, Features, MeshData, ProcessingRequest, ProcessingResult,
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
type Point = [f32; 3];
type Triangle = [u32; 3];
type Placement = ([[f32; 4]; 3], bool);
const MAX_INPUT: u64 = 2 * 1024 * 1024 * 1024;
const MAX_BATCH: usize = 8_000_000;
const TYPE_LIBRARY_GEOMETRY: u8 = 2;
const IDENTITY: [f32; 16] = [
    1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
];

fn bounded_bytes(reader: impl Read, size: u64) -> Result<Vec<u8>> {
    if size > MAX_INPUT {
        return Err("IFC data exceeds the decode limit".into());
    }
    let mut bytes = Vec::with_capacity(size as usize);
    reader.take(MAX_INPUT + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_INPUT {
        return Err("IFC data exceeds the decode limit".into());
    }
    Ok(bytes)
}

fn read(path: &Path) -> Result<Vec<u8>> {
    let file = File::open(path)?;
    if !path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("ifczip"))
    {
        let size = file.metadata()?.len();
        return bounded_bytes(file, size);
    }
    let mut archive = zip::ZipArchive::new(file)?;
    let mut model = None;
    for index in 0..archive.len() {
        let entry = archive.by_index_raw(index)?;
        if !entry.is_file()
            || !Path::new(&*entry.name()?)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("ifc"))
        {
            continue;
        }
        if model.replace(index).is_some() {
            return Err("IFC ZIP contains more than one IFC model".into());
        }
    }
    let entry = archive.by_index(model.ok_or("IFC ZIP contains no STEP-encoded IFC model")?)?;
    let size = entry.size();
    bounded_bytes(entry, size)
}

pub fn load(path: &Path) -> Result<ProcessingResult> {
    let start = Instant::now();
    let bytes = read(path)?;
    let read_ms = start.elapsed().as_secs_f64() * 1000.0;
    let result = decode(&bytes)?;
    if result.meshes.is_empty() {
        return Err("IFC contains no supported triangle geometry".into());
    }
    if std::env::var_os("STEPFLASH_PROFILE").is_some() {
        let stats = &result.stats;
        eprintln!(
            "IFC phases: read={read_ms:.2} ms, scan={} ms, lookup={} ms, preprocess={} ms, geometry={} ms, total={:.2} ms; meshes={}, triangles={}, instances={}, boolean_failures={}",
            stats.entity_scan_time_ms, stats.lookup_time_ms, stats.preprocess_time_ms,
            stats.geometry_time_ms, start.elapsed().as_secs_f64() * 1000.0,
            stats.total_meshes, stats.total_triangles, result.instances.len(), stats.total_csg_failures
        );
    }
    Ok(result)
}

fn decode(bytes: &[u8]) -> Result<ProcessingResult> {
    let request = ProcessingRequest::full_pass()
        .features(
            Features::default()
                .properties(false)
                .presentation_layers(false),
        )
        .instancing(true);
    let work = || process(bytes, &request, &mut ());
    #[cfg(feature = "rayon")]
    if std::env::var_os("RAYON_NUM_THREADS").is_none() {
        use std::sync::OnceLock;
        static SMALL_POOL: OnceLock<rayon::ThreadPool> = OnceLock::new();
        static LARGE_POOL: OnceLock<rayon::ThreadPool> = OnceLock::new();
        let (slot, limit) = if bytes.len() <= 2 * 1024 * 1024 {
            (&SMALL_POOL, 4)
        } else {
            (&LARGE_POOL, 8)
        };
        let pool = match slot.get() {
            Some(pool) => pool,
            None => {
                let threads = std::thread::available_parallelism()
                    .map_or(1, usize::from)
                    .min(limit);
                let pool = ifc_lite_processing::geometry_pool_builder()
                    .num_threads(threads)
                    .build()?;
                slot.get_or_init(|| pool)
            }
        };
        return Ok(pool.install(work));
    }
    Ok(run_on_geometry_pool(work)?)
}

pub fn issues(model: &ProcessingResult) -> usize {
    let stats = &model.stats;
    stats
        .products_with_failures
        .saturating_add(stats.refused_input_elements)
        .saturating_add(stats.refused_prepass_records)
        .saturating_add(
            stats
                .geometry_diagnostics
                .as_ref()
                .map_or(0, |info| info.total_unsupported_items),
        ) as usize
}

fn visible(mesh: &MeshData) -> bool {
    !mesh.indices.is_empty()
        && mesh.geometry_class != TYPE_LIBRARY_GEOMETRY
        && !mesh.ifc_type.eq_ignore_ascii_case("IfcOpeningElement")
        && !mesh.ifc_type.eq_ignore_ascii_case("IfcOpeningStandardCase")
}

#[derive(Clone, Copy)]
pub struct Geometry<'a> {
    pub points: &'a [Point],
    pub triangles: &'a [Triangle],
    origin: [f64; 3],
}

impl<'a> Geometry<'a> {
    fn from_mesh(mesh: &'a MeshData) -> Result<Self> {
        let points =
            bytemuck::try_cast_slice(&mesh.positions).map_err(|_| "Invalid IFC vertex layout")?;
        let triangles =
            bytemuck::try_cast_slice(&mesh.indices).map_err(|_| "Invalid IFC index layout")?;
        if mesh
            .indices
            .iter()
            .any(|&index| index as usize >= points.len())
        {
            return Err("Invalid IFC vertex index".into());
        }
        Ok(Self {
            points,
            triangles,
            origin: mesh.origin,
        })
    }
}

pub struct Batch<'a> {
    pub color: [f32; 4],
    geometry: Vec<Geometry<'a>>,
    pub vertices: usize,
    pub triangles: usize,
}

impl Batch<'_> {
    pub fn write_positions(
        &self,
        mut target: &mut [Point],
        anchor: [f64; 3],
    ) -> Result<(Point, Point)> {
        let mut min = [f32::INFINITY; 3];
        let mut max = [f32::NEG_INFINITY; 3];
        for geometry in &self.geometry {
            let delta: [f64; 3] = std::array::from_fn(|axis| geometry.origin[axis] - anchor[axis]);
            let (head, rest) = target.split_at_mut(geometry.points.len());
            for (dest, &point) in head.iter_mut().zip(geometry.points) {
                let [x, y, z] =
                    std::array::from_fn(|axis| (f64::from(point[axis]) + delta[axis]) as f32);
                *dest = [x, z, -y];
                for axis in 0..3 {
                    if !dest[axis].is_finite() {
                        return Err("Invalid IFC vertex position".into());
                    }
                    min[axis] = min[axis].min(dest[axis]);
                    max[axis] = max[axis].max(dest[axis]);
                }
            }
            target = rest;
        }
        Ok((min, max))
    }

    pub fn write_indices(&self, mut target: &mut [Triangle]) {
        let mut base = 0;
        for geometry in &self.geometry {
            let (head, rest) = target.split_at_mut(geometry.triangles.len());
            for (dest, &triangle) in head.iter_mut().zip(geometry.triangles) {
                *dest = triangle.map(|index| index + base);
            }
            base += geometry.points.len() as u32;
            target = rest;
        }
    }
}

pub struct InstanceBatch<'a> {
    pub geometry: Geometry<'a>,
    pub color: [f32; 4],
    pub placements: Vec<Placement>,
}

pub struct DrawPlan<'a> {
    pub anchor: [f64; 3],
    pub flat: Vec<Batch<'a>>,
    pub instanced: Vec<InstanceBatch<'a>>,
}

// Apply the instance to the template's Z-up coordinates before converting to
// Y-up. Fold the per-mesh origin and camera anchor into the translation in f64.
fn placement(matrix: [f32; 16], origin: [f64; 3], anchor: [f64; 3]) -> Result<Placement> {
    if matrix[12..] != [0.0, 0.0, 0.0, 1.0] {
        return Err("Invalid IFC instance transform".into());
    }
    let rows = std::array::from_fn(|axis| {
        let offset = [0, 8, 4][axis];
        let mut row: [f32; 4] = matrix[offset..offset + 4].try_into().unwrap();
        let translation = (0..3).map(|i| f64::from(row[i]) * origin[i]).sum::<f64>();
        row[3] = (translation + f64::from(row[3]) - anchor[offset / 4]) as f32;
        if axis == 2 {
            row.map(|value| -value)
        } else {
            row
        }
    });
    if rows.iter().flatten().any(|value| !value.is_finite()) {
        return Err("Invalid IFC instance transform".into());
    }
    // Match the flat IFC path's winding when the relative placement mirrors.
    let [a, b, c, _, d, e, f, _, g, h, i, ..] = matrix.map(f64::from);
    let determinant = a * (e * i - f * h) - b * (d * i - f * g) + c * (d * h - e * g);
    Ok((rows, determinant < 0.0))
}

pub fn draw_plan(model: &ProcessingResult) -> Result<DrawPlan<'_>> {
    use std::convert::TryFrom;

    let meshes: Vec<_> = model.meshes.iter().filter(|mesh| visible(mesh)).collect();
    let anchor = meshes
        .first()
        .ok_or("IFC contains no visible triangle geometry")?
        .origin;
    let mut flat = Vec::<Batch>::new();
    let mut instanced = Vec::<InstanceBatch>::new();
    let mut colors = HashMap::new();
    let mut instance_groups = HashMap::new();
    let mut templates = HashMap::new();
    for item in &model.instances {
        templates.insert((item.template_express_id, item.rep_identity), None);
    }
    for mesh in meshes {
        let geometry = Geometry::from_mesh(mesh)?;
        let color = mesh.color;
        let color_key = color.map(f32::to_bits);
        let template = mesh
            .instance
            .as_ref()
            .map(|info| (mesh.express_id, info.rep_identity));
        if let Some((key, target)) = template.and_then(|key| Some((key, templates.get_mut(&key)?)))
        {
            if target.replace(geometry).is_some() {
                return Err("Ambiguous IFC mesh template".into());
            }
            instance_groups.insert((key, color_key), instanced.len());
            instanced.push(InstanceBatch {
                geometry,
                color,
                placements: vec![placement(IDENTITY, geometry.origin, anchor)?],
            });
            continue;
        }
        let index = colors.entry(color_key).or_insert(usize::MAX);
        let vertices = geometry.points.len();
        let triangles = geometry.triangles.len();
        if *index == usize::MAX
            || flat[*index].vertices + vertices > MAX_BATCH
            || flat[*index].triangles + triangles > MAX_BATCH
        {
            *index = flat.len();
            flat.push(Batch {
                color,
                geometry: Vec::new(),
                vertices: 0,
                triangles: 0,
            });
        }
        let batch = &mut flat[*index];
        batch.geometry.push(geometry);
        batch.vertices += vertices;
        batch.triangles += triangles;
        u32::try_from(batch.vertices)?;
    }
    for item in &model.instances {
        let key = (item.template_express_id, item.rep_identity);
        let geometry = templates[&key].ok_or("Missing IFC mesh template")?;
        let color = item.color;
        let index = *instance_groups
            .entry((key, color.map(f32::to_bits)))
            .or_insert_with(|| {
                let index = instanced.len();
                instanced.push(InstanceBatch {
                    geometry,
                    color,
                    placements: Vec::new(),
                });
                index
            });
        instanced[index]
            .placements
            .push(placement(item.transform, geometry.origin, anchor)?);
    }
    Ok(DrawPlan {
        anchor,
        flat,
        instanced,
    })
}

#[cfg(test)]
mod tests {
    use super::{placement, IDENTITY};

    #[test]
    fn reflected_placement_preserves_local_origin_and_reverses_winding() {
        let mut transform = IDENTITY;
        transform[0] = -1.0;
        transform[3] = 12.0;
        transform[7] = 30.0;
        transform[11] = 5.0;
        let (rows, mirrored) = placement(transform, [2.0, 1.0, 3.0], [10.0, 20.0, 0.0]).unwrap();
        assert_eq!(
            rows,
            [
                [-1.0, 0.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 8.0],
                [0.0, -1.0, 0.0, -11.0]
            ]
        );
        assert!(mirrored);
    }
}
