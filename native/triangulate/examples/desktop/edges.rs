//! Screen-space edge acceleration and finite-edge measurements.
//! Built only after interaction, reused until the view changes.
use super::{
    camera::Camera,
    features::{Feature, Index},
    model::Scene,
};
use nalgebra_glm as glm;
type P = [f64; 3];
fn add(a: P, b: P) -> P {
    [0, 1, 2].map(|i| a[i] + b[i])
}
fn sub(a: P, b: P) -> P {
    [0, 1, 2].map(|i| a[i] - b[i])
}
fn mul(a: P, s: f64) -> P {
    a.map(|v| v * s)
}
fn dot(a: P, b: P) -> f64 {
    (0..3).map(|i| a[i] * b[i]).sum()
}
fn norm(a: P) -> f64 {
    dot(a, a).sqrt()
}
pub fn closest_segments(p: P, q: P, r: P, s: P) -> (P, P) {
    let u = sub(q, p);
    let v = sub(s, r);
    let w = sub(p, r);
    let a = dot(u, u);
    let e = dot(v, v);
    let f = dot(v, w);
    let (mut t, mut h);
    if a < 1e-28 && e < 1e-28 {
        return (p, r);
    }
    if a < 1e-28 {
        t = 0.;
        h = (f / e).clamp(0., 1.);
    } else {
        let c = dot(u, w);
        if e < 1e-28 {
            h = 0.;
            t = (-c / a).clamp(0., 1.);
        } else {
            let b = dot(u, v);
            let determinant = a * e - b * b;
            t = if determinant > a * e * 1e-14 {
                ((b * f - c * e) / determinant).clamp(0., 1.)
            } else {
                0.
            };
            h = (b * t + f) / e;
            if h < 0. {
                h = 0.;
                t = (-c / a).clamp(0., 1.);
            } else if h > 1. {
                h = 1.;
                t = ((b - c) / a).clamp(0., 1.);
            }
        }
    }
    (add(p, mul(u, t)), add(r, mul(v, h)))
}
#[derive(Clone, Debug)]
pub struct Gap {
    pub a: P,
    pub b: P,
    pub distance: f64,
    pub angle: Option<f64>,
    pub curved: bool,
}
struct Node {
    lo: P,
    hi: P,
    range: std::ops::Range<usize>,
    children: Option<[usize; 2]>,
}
fn bounds(segments: &[[P; 2]]) -> (P, P) {
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for s in segments {
        for p in s {
            for i in 0..3 {
                lo[i] = lo[i].min(p[i]);
                hi[i] = hi[i].max(p[i]);
            }
        }
    }
    (lo, hi)
}
fn box_distance(a: (P, P), b: (P, P)) -> f64 {
    (0..3)
        .map(|i| (a.0[i] - b.1[i]).max(b.0[i] - a.1[i]).max(0.).powi(2))
        .sum()
}
fn tree(segments: &mut [[P; 2]], offset: usize, nodes: &mut Vec<Node>) -> usize {
    let (lo, hi) = bounds(segments);
    let id = nodes.len();
    nodes.push(Node {
        lo,
        hi,
        range: offset..offset + segments.len(),
        children: None,
    });
    if segments.len() > 8 {
        let axis = (0..3)
            .max_by(|&a, &b| (hi[a] - lo[a]).total_cmp(&(hi[b] - lo[b])))
            .unwrap();
        let mid = segments.len() / 2;
        segments.select_nth_unstable_by(mid, |a, b| {
            (a[0][axis] + a[1][axis]).total_cmp(&(b[0][axis] + b[1][axis]))
        });
        let (a, b) = segments.split_at_mut(mid);
        let left = tree(a, offset, nodes);
        let right = tree(b, offset + mid, nodes);
        nodes[id].children = Some([left, right]);
    }
    id
}
fn nearest_pair(
    segment: [P; 2],
    query: (P, P),
    id: usize,
    nodes: &[Node],
    segments: &[[P; 2]],
    result: &mut Gap,
) {
    let node = &nodes[id];
    if box_distance(query, (node.lo, node.hi)) >= result.distance * result.distance {
        return;
    }
    if let Some([a, b]) = node.children {
        let da = box_distance(query, (nodes[a].lo, nodes[a].hi));
        let db = box_distance(query, (nodes[b].lo, nodes[b].hi));
        let order = if da <= db { [a, b] } else { [b, a] };
        for child in order {
            nearest_pair(segment, query, child, nodes, segments, result);
        }
    } else {
        for s in &segments[node.range.clone()] {
            let (a, b) = closest_segments(segment[0], segment[1], s[0], s[1]);
            let distance = norm(sub(a, b));
            if distance < result.distance {
                result.a = a;
                result.b = b;
                result.distance = distance;
            }
        }
    }
}
pub fn gap(a: &Feature, b: &Feature) -> Gap {
    let mut result = Gap {
        a: [0.; 3],
        b: [0.; 3],
        distance: f64::INFINITY,
        angle: None,
        curved: !matches!((a, b), (Feature::Line { .. }, Feature::Line { .. })),
    };
    let mut segments: Vec<_> = b
        .paths()
        .iter()
        .flat_map(|p| p.windows(2).map(|w| [w[0], w[1]]))
        .collect();
    if !segments.is_empty() {
        let mut nodes = Vec::new();
        let root = tree(&mut segments, 0, &mut nodes);
        for left in a.paths() {
            for w in left.windows(2) {
                let segment = [w[0], w[1]];
                nearest_pair(
                    segment,
                    bounds(&[segment]),
                    root,
                    &nodes,
                    &segments,
                    &mut result,
                );
                if result.distance == 0. {
                    break;
                }
            }
        }
    }
    if let (Feature::Line { a: p, b: q }, Feature::Line { a: r, b: s }) = (a, b) {
        let u = sub(*q, *p);
        let v = sub(*s, *r);
        let length = norm(u) * norm(v);
        if length > 1e-28 {
            result.angle = Some((dot(u, v).abs() / length).clamp(0., 1.).acos().to_degrees());
        }
    }
    result
}
pub fn key(camera: &Camera, size: [u32; 2]) -> Vec<u32> {
    camera
        .rotation
        .coords
        .iter()
        .copied()
        .chain([camera.zoom, camera.explode, f32::from(u8::from(camera.legacy_explode))])
        .chain(camera.pan)
        .chain(camera.clip)
        .map(f32::to_bits)
        .chain(size)
        .chain([u32::from(camera.orthographic)])
        .collect()
}
fn project(camera: &Camera, scene: &Scene, size: [u32; 2], p: P) -> Option<[f64; 2]> {
    camera
        .project(p.map(|v| v as f32), scene, size)
        .map(|p| p.map(f64::from))
}
fn clipped(camera: &Camera, p: P) -> bool {
    camera.clip[..3].iter().any(|v| *v != 0.)
        && (0..3)
            .map(|i| f64::from(camera.clip[i]) * p[i])
            .sum::<f64>()
            > f64::from(camera.clip[3])
}
struct Segment {
    a: [f64; 2],
    b: [f64; 2],
    p: P,
    q: P,
    id: usize,
    wa: f64,
    wb: f64,
}
#[derive(Clone)]
pub struct Candidate {
    pub id: usize,
    pub at: [u32; 2],
    pub world: P,
    pub pointer: [u32; 2],
    pub view: Vec<u32>,
    pub tolerance: f64,
}
pub struct ScreenEdges {
    view: Vec<u32>,
    segments: Vec<Segment>,
    cells: Vec<Vec<usize>>,
    cols: usize,
    rows: usize,
    size: [u32; 2],
    pub build_ms: f64,
}
const CELL: f64 = 32.;
impl ScreenEdges {
    pub fn matches(&self, camera: &Camera, size: [u32; 2]) -> bool {
        self.view == key(camera, size)
    }
    pub fn new(index: &Index, camera: &Camera, scene: &Scene, size: [u32; 2]) -> Self {
        let started = std::time::Instant::now();
        let cols = (size[0] as f64 / CELL).ceil().max(1.) as usize;
        let rows = (size[1] as f64 / CELL).ceil().max(1.) as usize;
        let mut cache = Self {
            view: key(camera, size),
            segments: Vec::new(),
            cells: vec![Vec::new(); cols * rows],
            cols,
            rows,
            size,
            build_ms: 0.,
        };
        let weight = |p: P| {
            let scale = 0.9 * (size[0] as f32 / size[1].max(1) as f32).min(1.) * camera.zoom
                / scene.diameter;
            let p = glm::vec3(
                p[0] as f32 - scene.center[0] - camera.pan[0],
                p[1] as f32 - scene.center[1] - camera.pan[1],
                p[2] as f32 - scene.center[2] - camera.pan[2],
            ) * scale;
            let p = glm::quat_rotate_vec3(&camera.rotation, &p);
            if camera.orthographic {
                1.
            } else {
                f64::from((4. - p.z) * 0.25)
            }
        };
        for (id, feature) in index.features.iter().enumerate() {
            if matches!(feature, Feature::Cylinder { .. }) {
                continue;
            }
            for path in feature.paths() {
                for pair in path.windows(2) {
                    let (mut p, mut q) = (pair[0], pair[1]);
                    let (pc, qc) = (clipped(camera, p), clipped(camera, q));
                    if pc && qc {
                        continue;
                    }
                    if pc != qc {
                        let d = |p: P| {
                            (0..3)
                                .map(|i| f64::from(camera.clip[i]) * p[i])
                                .sum::<f64>()
                                - f64::from(camera.clip[3])
                        };
                        let t = d(p) / (d(p) - d(q));
                        let cut = add(p, mul(sub(q, p), t));
                        if pc {
                            p = cut;
                        } else {
                            q = cut;
                        }
                    }
                    let (a, b) = match (
                        project(camera, scene, size, p),
                        project(camera, scene, size, q),
                    ) {
                        (Some(a), Some(b)) => (a, b),
                        _ => continue,
                    };
                    let x0 = a[0].min(b[0]) - 18.;
                    let y0 = a[1].min(b[1]) - 18.;
                    let x1 = a[0].max(b[0]) + 18.;
                    let y1 = a[1].max(b[1]) + 18.;
                    if x1 < 0. || y1 < 0. || x0 > size[0] as f64 || y0 > size[1] as f64 {
                        continue;
                    }
                    let first = [(x0.max(0.) / CELL) as usize, (y0.max(0.) / CELL) as usize];
                    let last = [
                        ((x1.max(0.) / CELL) as usize).min(cols - 1),
                        ((y1.max(0.) / CELL) as usize).min(rows - 1),
                    ];
                    let s = cache.segments.len();
                    cache.segments.push(Segment {
                        a,
                        b,
                        p,
                        q,
                        id,
                        wa: weight(p),
                        wb: weight(q),
                    });
                    for y in first[1]..=last[1] {
                        for x in first[0]..=last[0] {
                            cache.cells[y * cols + x].push(s);
                        }
                    }
                }
            }
        }
        cache.build_ms = started.elapsed().as_secs_f64() * 1000.;
        cache
    }
    pub fn nearest(
        &self,
        index: &Index,
        at: [u32; 2],
        radius: f64,
        circular: bool,
        scene: &Scene,
        camera: &Camera,
    ) -> Option<Candidate> {
        let cell = [
            ((at[0] as f64 / CELL) as usize).min(self.cols - 1),
            ((at[1] as f64 / CELL) as usize).min(self.rows - 1),
        ];
        let pointer = at.map(|v| v as f64 + 0.5);
        let mut best: Option<(f64, f64, &Segment, f64)> = None;
        for &s in &self.cells[cell[1] * self.cols + cell[0]] {
            let s = &self.segments[s];
            if circular && index.features[s.id].circle().is_none() {
                continue;
            }
            let d = [s.b[0] - s.a[0], s.b[1] - s.a[1]];
            let t = (((pointer[0] - s.a[0]) * d[0] + (pointer[1] - s.a[1]) * d[1])
                / (d[0] * d[0] + d[1] * d[1]).max(1e-20))
            .clamp(0., 1.);
            let distance = (pointer[0] - s.a[0] - t * d[0]).hypot(pointer[1] - s.a[1] - t * d[1]);
            if distance > radius {
                continue;
            }
            let z = s.wa * (1. - t) + s.wb * t;
            if best
                .as_ref()
                .is_none_or(|b| distance < b.0 - 0.2 || ((distance - b.0).abs() <= 0.2 && z < b.1))
            {
                best = Some((distance, z, s, t));
            }
        }
        let (_, _, s, t) = best?;
        let u = (t / s.wb) / ((1. - t) / s.wa + t / s.wb);
        let world = add(s.p, mul(sub(s.q, s.p), u));
        let expected = project(camera, scene, self.size, world)?;
        let at = [
            expected[0]
                .floor()
                .clamp(0., self.size[0].saturating_sub(1) as f64) as u32,
            expected[1]
                .floor()
                .clamp(0., self.size[1].saturating_sub(1) as f64) as u32,
        ];
        let tolerance = index.features[s.id].circle().map_or(0., |c| {
            c.radius * (1. - (c.step * 0.5).cos()) + c.deviation * 2.
        }) + f64::from(scene.diameter)
            / f64::from(camera.zoom.max(1e-6))
            / f64::from(self.size[1].max(1))
            * 1.5;
        Some(Candidate {
            id: s.id,
            at,
            world,
            pointer: at_from(pointer),
            view: self.view.clone(),
            tolerance,
        })
    }
}
fn at_from(p: [f64; 2]) -> [u32; 2] {
    p.map(|v| (v - 0.5).max(0.) as u32)
}
pub fn visible(
    candidate: &Candidate,
    hit: Option<[f32; 3]>,
    camera: &Camera,
    scene: &Scene,
) -> bool {
    let hit = match hit {
        Some(hit) => hit,
        None => return true,
    };
    let d = sub(candidate.world, hit.map(f64::from));
    let eye = glm::quat_rotate_vec3(
        &camera.rotation,
        &glm::vec3(d[0] as f32, d[1] as f32, d[2] as f32),
    );
    let _ = scene;
    f64::from(eye.z) >= -candidate.tolerance
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn finite_segments_handle_parallel_skew_crossing_and_degenerate_edges() {
        let (a, b) = closest_segments([0., 0., 0.], [10., 0., 0.], [3., 4., 0.], [8., 4., 0.]);
        assert!((norm(sub(a, b)) - 4.).abs() < 1e-12);
        let f = |a, b| Feature::Line { a, b };
        let g = gap(&f([0.; 3], [10., 0., 0.]), &f([5., -5., 2.], [5., 5., 2.]));
        assert!((g.distance - 2.).abs() < 1e-12);
        assert_eq!(g.angle, Some(90.));
        let g = gap(&f([0.; 3], [1., 0., 0.]), &f([3., 4., 0.], [3., 4., 0.]));
        assert!((g.distance - 20f64.sqrt()).abs() < 1e-12);
    }
    #[test]
    fn reversing_an_edge_does_not_change_its_angle() {
        let a = Feature::Line {
            a: [0.; 3],
            b: [1., 0., 0.],
        };
        let b = Feature::Line {
            a: [0.; 3],
            b: [1., 1., 0.],
        };
        let reverse = Feature::Line {
            a: [1., 1., 0.],
            b: [0.; 3],
        };
        assert!((gap(&a, &b).angle.unwrap() - 45.).abs() < 1e-10);
        assert_eq!(gap(&a, &b).angle, gap(&a, &reverse).angle);
    }
    #[test]
    fn long_curves_use_finite_segment_bounds_and_preserve_the_gap() {
        let a = Feature::Curve {
            points: (0..10000)
                .map(|i| [i as f64 * 0.01, (i as f64 * 0.01).sin(), 0.])
                .collect(),
        };
        let b = Feature::Curve {
            points: (0..10000)
                .map(|i| [i as f64 * 0.01, (i as f64 * 0.01).sin(), 3.])
                .collect(),
        };
        let result = gap(&a, &b);
        assert!((result.distance - 3.).abs() < 1e-12);
        assert!(result.curved);
        assert_eq!(result.angle, None);
    }
}
