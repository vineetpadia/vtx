//! Lazy feature recognition. Opening a model retains no second CPU mesh.
//! Readback, welding and fitting happen on a cancellable worker, once requested.
use super::{camera::Camera, model::Scene};
use nalgebra_glm as glm;
use std::{
    collections::{HashMap, VecDeque},
    convert::TryInto,
    f64::consts::{PI, TAU},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
};
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
fn cross(a: P, b: P) -> P {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn length(a: P) -> f64 {
    dot(a, a).sqrt()
}
fn unit(a: P) -> Option<P> {
    let n = length(a);
    (n > 1e-20 && n.is_finite()).then(|| mul(a, 1. / n))
}
fn f32p(a: P) -> [f32; 3] {
    a.map(|v| v as f32)
}
fn f64p(a: [f32; 3]) -> P {
    a.map(f64::from)
}

#[derive(Clone, Debug, PartialEq)]
pub struct Circle {
    pub center: P,
    pub u: P,
    pub v: P,
    pub radius: f64,
    pub start: f64,
    pub sweep: f64,
    pub deviation: f64,
    pub samples: usize,
    pub exact: bool,
    pub step: f64,
}
impl Circle {
    pub fn normal(&self) -> P {
        cross(self.u, self.v)
    }
    pub fn at(&self, t: f64) -> P {
        add(
            self.center,
            mul(add(mul(self.u, t.cos()), mul(self.v, t.sin())), self.radius),
        )
    }
    pub fn polyline(&self) -> Vec<P> {
        let n = (self.sweep.abs() / TAU * 128.).ceil().max(16.) as usize;
        (0..=n)
            .map(|i| self.at(self.start + self.sweep * i as f64 / n as f64))
            .collect()
    }
    fn full(&self) -> bool {
        self.sweep.abs() > TAU - 0.02
    }
    fn fit_distance(&self, p: P) -> f64 {
        let d = sub(p, self.center);
        (dot(d, self.normal()).powi(2)
            + (dot(d, self.u).hypot(dot(d, self.v)) - self.radius).powi(2))
        .sqrt()
    }
    fn distance(&self, p: P) -> f64 {
        let d = sub(p, self.center);
        let x = dot(d, self.u);
        let y = dot(d, self.v);
        let angle = y.atan2(x);
        let progress = ((angle - self.start) * self.sweep.signum()).rem_euclid(TAU);
        if !self.full() && progress > self.sweep.abs() {
            return length(sub(p, self.at(self.start)))
                .min(length(sub(p, self.at(self.start + self.sweep))));
        }
        (dot(d, self.normal()).powi(2) + (x.hypot(y) - self.radius).powi(2)).sqrt()
    }
}
#[derive(Clone, Debug, PartialEq)]
pub enum Feature {
    Circle(Circle),
    Cylinder { rim: Circle, height: f64 },
    Line { a: P, b: P },
    Curve { points: Vec<P> },
}
impl Feature {
    pub fn circle(&self) -> Option<&Circle> {
        match self {
            Self::Circle(c) | Self::Cylinder { rim: c, .. } => Some(c),
            _ => None,
        }
    }
    pub fn length(&self) -> f64 {
        match self {
            Self::Circle(c) => c.radius * c.sweep.abs(),
            Self::Cylinder { height, .. } => height.abs(),
            Self::Line { a, b } => length(sub(*a, *b)),
            Self::Curve { points } => points.windows(2).map(|w| length(sub(w[1], w[0]))).sum(),
        }
    }
    pub fn paths(&self) -> Vec<Vec<P>> {
        match self {
            Self::Circle(c) => vec![c.polyline()],
            Self::Line { a, b } => vec![vec![*a, *b]],
            Self::Curve { points } => vec![points.clone()],
            Self::Cylinder { rim, height } => {
                let mut other = rim.clone();
                other.center = add(rim.center, mul(rim.normal(), *height));
                vec![rim.polyline(), other.polyline()]
            }
        }
    }
    pub fn anchors(&self) -> Vec<(P, &'static str)> {
        match self {
            Self::Circle(c) => {
                let mut a = vec![(c.center, "Center")];
                if !c.full() {
                    a.extend([
                        (c.at(c.start), "Endpoint"),
                        (c.at(c.start + c.sweep), "Endpoint"),
                    ]);
                }
                a
            }
            Self::Cylinder { rim, height } => vec![
                (rim.center, "Center"),
                (add(rim.center, mul(rim.normal(), *height)), "Center"),
            ],
            Self::Line { a, b } => vec![
                (*a, "Endpoint"),
                (*b, "Endpoint"),
                (mul(add(*a, *b), 0.5), "Midpoint"),
            ],
            Self::Curve { points } => {
                if points.len() < 2 || length(sub(points[0], *points.last().unwrap())) < 1e-10 {
                    return Vec::new();
                }
                let mut midpoint = points[0];
                let mut remaining = self.length() * 0.5;
                for pair in points.windows(2) {
                    let l = length(sub(pair[1], pair[0]));
                    if remaining <= l {
                        midpoint = add(
                            pair[0],
                            mul(sub(pair[1], pair[0]), remaining / l.max(1e-30)),
                        );
                        break;
                    }
                    remaining -= l;
                }
                vec![
                    (points[0], "Endpoint"),
                    (*points.last().unwrap(), "Endpoint"),
                    (midpoint, "Midpoint"),
                ]
            }
        }
    }
}
#[derive(Default)]
pub struct Index {
    pub features: Vec<Feature>,
    pub elapsed_ms: f64,
}

/// Planar least-squares fit, with explicit residual and angular support checks.
/// A three-point circumcircle alone is deliberately insufficient evidence.
fn fit_circle(points: &[P], epsilon: f64) -> Option<Circle> {
    fit_circle_min(points, epsilon, PI / 3.)
}
fn fit_circle_min(points: &[P], epsilon: f64, minimum_sweep: f64) -> Option<Circle> {
    if points.len() < 7 {
        return None;
    }
    let origin = mul(
        points.iter().copied().fold([0.; 3], add),
        1. / points.len() as f64,
    );
    let a = *points
        .iter()
        .max_by(|a, b| length(sub(**a, origin)).total_cmp(&length(sub(**b, origin))))?;
    let b = *points
        .iter()
        .max_by(|a0, b0| length(sub(**a0, a)).total_cmp(&length(sub(**b0, a))))?;
    let axis = unit(sub(b, a))?;
    let c = *points.iter().max_by(|a0, b0| {
        length(cross(sub(**a0, a), axis)).total_cmp(&length(cross(sub(**b0, a), axis)))
    })?;
    let n = unit(cross(axis, sub(c, a)))?;
    let u = axis;
    let v = cross(n, u);
    let mut matrix = glm::DMat3::zeros();
    let mut rhs = glm::DVec3::zeros();
    for &p in points {
        let d = sub(p, origin);
        let x = dot(d, u);
        let y = dot(d, v);
        let row = glm::vec3(x, y, 1.);
        matrix += row * row.transpose();
        rhs += row * (x * x + y * y);
    }
    let solution = matrix.lu().solve(&rhs)?;
    let center = add(
        origin,
        add(mul(u, solution.x * 0.5), mul(v, solution.y * 0.5)),
    );
    let radius = (solution.z + 0.25 * (solution.x * solution.x + solution.y * solution.y)).sqrt();
    if !radius.is_finite() || radius <= epsilon * 4. {
        return None;
    }
    let tolerance = (radius * 0.0003).max(epsilon * 8.);
    let mut deviation = 0f64;
    let mut angles = Vec::with_capacity(points.len());
    for &p in points {
        let d = sub(p, center);
        let x = dot(d, u);
        let y = dot(d, v);
        if dot(d, n).abs() > (radius * 0.00001).max(epsilon * 4.) {
            return None;
        }
        deviation = deviation
            .max((x.hypot(y) - radius).abs())
            .max(dot(d, n).abs());
        angles.push(y.atan2(x));
    }
    if deviation > tolerance {
        return None;
    }
    let mut sweep = 0.;
    let mut max_step = 0f64;
    for w in angles.windows(2) {
        let d = (w[1] - w[0] + PI).rem_euclid(TAU) - PI;
        if sweep * d < -1e-5 {
            return None;
        }
        sweep += d;
        max_step = max_step.max(d.abs());
    }
    if sweep.abs() < minimum_sweep || sweep.abs() > TAU + 0.02 || max_step > PI / 5. + 0.001 {
        return None;
    }
    Some(Circle {
        center,
        u,
        v,
        radius,
        start: angles[0],
        sweep,
        deviation,
        samples: points.len(),
        exact: false,
        step: max_step,
    })
}

fn path_features(path: &[P], epsilon: f64, out: &mut Vec<Feature>) {
    if let Some(c) = fit_circle(path, epsilon) {
        out.push(Feature::Circle(c));
        return;
    }
    // Split tangent lines/arcs rather than fitting an arbitrary whole outline.
    let mut i = 0;
    while i + 1 < path.len() {
        if i + 7 <= path.len() {
            if let Some(seed) = fit_circle_min(&path[i..i + 7], epsilon, 0.02) {
                let mut end = i + 7;
                while end < path.len()
                    && seed.fit_distance(path[end]) < (seed.radius * 0.0003).max(epsilon * 8.)
                {
                    end += 1;
                }
                if let Some(c) = fit_circle(&path[i..end], epsilon) {
                    out.push(Feature::Circle(c));
                    i = end - 1;
                    continue;
                }
            }
        }
        // A rejected arc may have consumed the last sample. Restart the line
        // fallback at this segment, rather than indexing one past the path.
        let mut end = i + 1;
        // Keep an unrecognized smooth curve as one edge. Its intermediate
        // display vertices must not become selectable CAD endpoints.
        let initial_turn = unit(sub(path[i + 1], path[i]))
            .zip(path.get(i + 2).and_then(|p| unit(sub(*p, path[i + 1]))))
            .map_or(1., |(a, b)| dot(a, b));
        let mut curve_end = i + 1;
        while initial_turn < 0.9999999999 && curve_end + 1 < path.len() {
            let a = unit(sub(path[curve_end], path[curve_end - 1]));
            let b = unit(sub(path[curve_end + 1], path[curve_end]));
            if !a.zip(b).is_some_and(|(a, b)| dot(a, b) > 0.866) {
                break;
            }
            curve_end += 1;
        }
        if curve_end >= i + 4 {
            let direction = unit(sub(path[i + 1], path[i]));
            let curved = direction.is_some_and(|d| {
                path[i + 2..=curve_end]
                    .iter()
                    .any(|p| length(cross(sub(*p, path[i]), d)) > epsilon * 8.)
            });
            if curved {
                out.push(Feature::Curve {
                    points: path[i..=curve_end].to_vec(),
                });
                i = curve_end;
                continue;
            }
        }
        let direction = unit(sub(path[i + 1], path[i]));
        if let Some(direction) = direction {
            while end + 1 < path.len()
                && length(cross(sub(path[end + 1], path[i]), direction)) < epsilon * 8.
                && dot(sub(path[end + 1], path[end]), direction) > 0.
            {
                end += 1;
            }
            if length(sub(path[end], path[i])) > epsilon * 20. {
                out.push(Feature::Line {
                    a: path[i],
                    b: path[end],
                });
            }
        }
        i = end;
    }
}

fn edge_paths(points: &[P], edges: &[(usize, usize)], epsilon: f64) -> Vec<Feature> {
    let mut graph = vec![Vec::new(); points.len()];
    for (i, &(a, b)) in edges.iter().enumerate() {
        graph[a].push(i);
        graph[b].push(i);
    }
    let mut used = vec![false; edges.len()];
    let mut features = Vec::new();
    for (e, &(a, b)) in edges.iter().enumerate() {
        if used[e] {
            continue;
        }
        used[e] = true;
        let mut path = VecDeque::from([a, b]);
        for front in [false, true] {
            loop {
                if path.len() > 2 && path.front() == path.back() {
                    break;
                }
                let n = path.len();
                let (last, previous) = if front {
                    (path[0], path[1])
                } else {
                    (path[n - 1], path[n - 2])
                };
                let direction = unit(sub(points[last], points[previous]));
                let best = graph[last]
                    .iter()
                    .filter(|&&edge| !used[edge])
                    .filter_map(|&edge| {
                        let (a, b) = edges[edge];
                        let other = if a == last { b } else { a };
                        let d = dot(direction?, unit(sub(points[other], points[last]))?);
                        (d > 0.4).then_some((d, edge, other))
                    })
                    .max_by(|a, b| a.0.total_cmp(&b.0));
                if let Some((_, edge, other)) = best {
                    used[edge] = true;
                    if front {
                        path.push_front(other);
                    } else {
                        path.push_back(other);
                    }
                } else {
                    break;
                }
            }
        }
        path_features(
            &path.iter().map(|&i| points[i]).collect::<Vec<_>>(),
            epsilon,
            &mut features,
        );
    }
    features
}

pub fn analyze_mesh(
    positions: &[P],
    triangles: &[[u32; 3]],
    cancel: &AtomicBool,
) -> std::result::Result<Vec<Feature>, String> {
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for p in positions {
        for i in 0..3 {
            lo[i] = lo[i].min(p[i]);
            hi[i] = hi[i].max(p[i]);
        }
    }
    let epsilon = length(sub(hi, lo)).max(1e-12) * 2e-7;
    let mut weld = HashMap::new();
    let mut points = Vec::new();
    let mut ids = vec![usize::MAX; positions.len()];
    let mut edges = HashMap::<(usize, usize), (P, bool, u32)>::new();
    for (t, triangle) in triangles.iter().enumerate() {
        if t % 8192 == 0 && cancel.load(Ordering::Relaxed) {
            return Err("Cancelled".into());
        }
        let mut indices = [0; 3];
        for (j, &index) in triangle.iter().enumerate() {
            let index = index as usize;
            let p = *positions.get(index).ok_or("Invalid feature vertex index")?;
            if p.iter().any(|v| !v.is_finite()) {
                return Err("Invalid feature coordinates".into());
            }
            if ids[index] == usize::MAX {
                let key = p.map(|v| (v / epsilon).round() as i64);
                ids[index] = *weld.entry(key).or_insert_with(|| {
                    let id = points.len();
                    points.push(p);
                    id
                });
            }
            indices[j] = ids[index];
        }
        let [a, b, c] = indices;
        let normal = match unit(cross(sub(points[b], points[a]), sub(points[c], points[a]))) {
            Some(n) => n,
            None => continue,
        };
        for (a, b) in [(a, b), (b, c), (c, a)] {
            let key = if a < b { (a, b) } else { (b, a) };
            edges
                .entry(key)
                .and_modify(|(n, crease, count)| {
                    *crease |= dot(*n, normal).abs() < 0.9;
                    *count += 1;
                })
                .or_insert((normal, false, 1));
        }
    }
    let mut edges: Vec<_> = edges
        .into_iter()
        .filter_map(|(edge, (_, crease, count))| (crease || count == 1).then_some(edge))
        .collect();
    edges.sort_unstable();
    Ok(edge_paths(&points, &edges, epsilon))
}

fn cylinders(features: &[Feature]) -> Vec<Feature> {
    let circles: Vec<_> = features
        .iter()
        .filter_map(|f| match f {
            Feature::Circle(c) if c.full() => Some(c),
            _ => None,
        })
        .collect();
    // Spatial grouping avoids quadratic work on assemblies with repeated holes.
    let mut groups = HashMap::<[i64; 7], Vec<&Circle>>::new();
    for c in circles {
        let mut n = c.normal();
        if n.iter().copied().find(|v| v.abs() > 1e-6).unwrap_or(1.) < 0. {
            n = mul(n, -1.);
        }
        let base = sub(c.center, mul(n, dot(c.center, n)));
        let scale = (c.radius * 0.002).max(1e-10);
        let key = [
            (n[0] * 1000.).round() as i64,
            (n[1] * 1000.).round() as i64,
            (n[2] * 1000.).round() as i64,
            (c.radius.ln() * 1000.).round() as i64,
            (base[0] / scale).round() as i64,
            (base[1] / scale).round() as i64,
            (base[2] / scale).round() as i64,
        ];
        groups.entry(key).or_default().push(c);
    }
    let mut out = Vec::new();
    for mut group in groups.into_values() {
        if group.len() < 2 {
            continue;
        }
        let normal = group[0].normal();
        group.sort_by(|a, b| dot(a.center, normal).total_cmp(&dot(b.center, normal)));
        for pair in group.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            let delta = sub(b.center, a.center);
            let height = dot(delta, a.normal());
            if height.abs() < a.radius * 1e-4
                || length(cross(delta, a.normal())) > a.radius * 0.001
                || (a.radius - b.radius).abs() > a.radius * 0.001
            {
                continue;
            }
            out.push(Feature::Cylinder {
                rim: a.clone(),
                height,
            });
        }
    }
    out
}

fn transform_feature(f: &Feature, m: [[f32; 4]; 3]) -> Option<Feature> {
    let vector = |p: P| [0, 1, 2].map(|r| (0..3).map(|i| f64::from(m[r][i]) * p[i]).sum::<f64>());
    let point = |p: P| add(vector(p), [0, 1, 2].map(|r| f64::from(m[r][3])));
    match f {
        Feature::Line { a, b } => Some(Feature::Line {
            a: point(*a),
            b: point(*b),
        }),
        Feature::Curve { points } => Some(Feature::Curve {
            points: points.iter().copied().map(point).collect(),
        }),
        Feature::Circle(c) | Feature::Cylinder { rim: c, .. } => {
            let a = vector(c.u);
            let b = vector(c.v);
            let scale = length(a);
            let other = length(b);
            if scale < 1e-20
                || (scale - other).abs() > scale * 1e-5
                || dot(a, b).abs() > scale * other * 1e-5
            {
                return None;
            }
            let mut c2 = c.clone();
            c2.center = point(c.center);
            c2.u = mul(a, 1. / scale);
            c2.v = mul(b, 1. / other);
            c2.radius *= scale;
            c2.deviation *= scale;
            if let Feature::Cylinder { height, .. } = f {
                let axis = vector(c.normal());
                if (length(axis) - scale).abs() > scale * 1e-5
                    || dot(axis, a).abs() > scale * scale * 1e-5
                    || dot(axis, b).abs() > scale * scale * 1e-5
                {
                    return None;
                }
                let h = dot(axis, c2.normal()) * height;
                Some(Feature::Cylinder { rim: c2, height: h })
            } else {
                Some(Feature::Circle(c2))
            }
        }
    }
}

fn screen(camera: &Camera, scene: &Scene, size: [u32; 2], p: P) -> Option<[f64; 2]> {
    camera
        .project(f32p(p), scene, size)
        .map(|p| p.map(f64::from))
}
fn clipped(camera: &Camera, p: P) -> bool {
    camera.clip[..3].iter().any(|v| *v != 0.)
        && (0..3)
            .map(|i| f64::from(camera.clip[i]) * p[i])
            .sum::<f64>()
            > f64::from(camera.clip[3])
}
fn segment_distance(at: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let d = [b[0] - a[0], b[1] - a[1]];
    let t = (((at[0] - a[0]) * d[0] + (at[1] - a[1]) * d[1])
        / (d[0] * d[0] + d[1] * d[1]).max(1e-20))
    .clamp(0., 1.);
    (at[0] - a[0] - t * d[0]).hypot(at[1] - a[1] - t * d[1])
}
impl Index {
    pub fn select(
        &self,
        camera: &Camera,
        scene: &Scene,
        size: [u32; 2],
        at: [u32; 2],
        hit: Option<[f32; 3]>,
        circular: bool,
    ) -> Option<Feature> {
        let hit = hit.map(f64p);
        if hit.is_some_and(|p| clipped(camera, p)) {
            return None;
        }
        let at = at.map(|v| v as f64 + 0.5);
        let pixels = 14.;
        let world_per_pixel = f64::from(scene.diameter)
            / f64::from(camera.zoom.max(1e-6))
            / f64::from(size[1].max(1));
        let mut best: Option<(f64, Feature)> = None;
        for f in &self.features {
            if circular && f.circle().is_none() {
                continue;
            }
            if !circular && matches!(f, Feature::Cylinder { .. }) {
                continue;
            }
            if let Feature::Cylinder { rim, height } = f {
                let p = match hit {
                    Some(p) => p,
                    None => continue,
                };
                let d = sub(p, rim.center);
                let along = dot(d, rim.normal());
                let radial = length(sub(d, mul(rim.normal(), along)));
                let tolerance =
                    (rim.radius * (1. - (rim.step * 0.5).cos()) * 1.2).max(rim.radius * 0.001);
                if along < height.min(0.)
                    || along > height.max(0.)
                    || (radial - rim.radius).abs() > tolerance
                {
                    continue;
                }
                let score = 8. + (radial - rim.radius).abs() / world_per_pixel.max(1e-20);
                if best.as_ref().is_none_or(|b| score < b.0) {
                    best = Some((score, f.clone()));
                }
                continue;
            }
            let mut closest = f64::INFINITY;
            for path in f.paths() {
                for w in path.windows(2) {
                    if clipped(camera, w[0]) || clipped(camera, w[1]) {
                        continue;
                    }
                    if let (Some(a), Some(b)) = (
                        screen(camera, scene, size, w[0]),
                        screen(camera, scene, size, w[1]),
                    ) {
                        closest = closest.min(segment_distance(at, a, b));
                    }
                }
            }
            if closest > pixels {
                continue;
            }
            // Occluded/back-side features must not win over the picked visible surface.
            if let Some(p) = hit {
                let d = match f {
                    Feature::Circle(c) => c.distance(p),
                    Feature::Line { a, b } => {
                        let ab = sub(*b, *a);
                        let t = (dot(sub(p, *a), ab) / dot(ab, ab).max(1e-20)).clamp(0., 1.);
                        length(sub(p, add(*a, mul(ab, t))))
                    }
                    Feature::Curve { points } => points
                        .windows(2)
                        .map(|w| {
                            let ab = sub(w[1], w[0]);
                            let t = (dot(sub(p, w[0]), ab) / dot(ab, ab).max(1e-20)).clamp(0., 1.);
                            length(sub(p, add(w[0], mul(ab, t))))
                        })
                        .fold(f64::INFINITY, f64::min),
                    _ => unreachable!(),
                };
                if d > world_per_pixel * pixels * 1.5 {
                    continue;
                }
            }
            if best.as_ref().is_none_or(|b| closest < b.0) {
                best = Some((closest, f.clone()));
            }
        }
        best.map(|b| b.1)
    }
    pub fn snap(
        &self,
        camera: &Camera,
        scene: &Scene,
        size: [u32; 2],
        at: [u32; 2],
        hit: Option<[f32; 3]>,
    ) -> Option<([f32; 3], &'static str)> {
        let at = at.map(|v| v as f64 + 0.5);
        let mut best: Option<(f64, [f32; 3], &'static str)> = None;
        let tolerance = f64::from(scene.diameter)
            / f64::from(camera.zoom.max(1e-6))
            / f64::from(size[1].max(1))
            * 20.;
        for f in &self.features {
            for (p, label) in f.anchors() {
                if clipped(camera, p) {
                    continue;
                }
                let q = match screen(camera, scene, size, p) {
                    Some(q) => q,
                    None => continue,
                };
                let d = (at[0] - q[0]).hypot(at[1] - q[1]);
                if d > 12. {
                    continue;
                }
                if let Some(h) = hit {
                    let delta = sub(p, f64p(h));
                    if length(delta) > tolerance && label != "Center" {
                        continue;
                    }
                    let eye_delta = glm::quat_rotate_vec3(
                        &camera.rotation,
                        &glm::vec3(delta[0] as f32, delta[1] as f32, delta[2] as f32),
                    );
                    if f64::from(eye_delta.z) < -tolerance {
                        continue;
                    }
                }
                // Break coincident projected-center ties in favor of the front feature.
                let rotated = glm::quat_rotate_vec3(
                    &camera.rotation,
                    &glm::vec3(p[0] as f32, p[1] as f32, p[2] as f32),
                );
                let score = d - f64::from(rotated.z) / f64::from(scene.diameter.max(1e-20)) * 0.01;
                if best.as_ref().is_none_or(|b| score < b.0) {
                    best = Some((score, f32p(p), label));
                }
            }
        }
        best.map(|(_, p, label)| (p, label))
    }
}

struct Source {
    positions: wgpu::Buffer,
    indices: Option<wgpu::Buffer>,
    obj: Option<wgpu::Buffer>,
    instances: Option<wgpu::Buffer>,
    lines: bool,
    points: bool,
}
pub struct Job {
    receive: mpsc::Receiver<std::result::Result<Index, String>>,
    cancel: Arc<AtomicBool>,
}
impl Drop for Job {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}
impl Job {
    pub fn start(
        scene: &Scene,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> std::result::Result<Self, String> {
        let sources: Vec<_> = scene
            .parts
            .iter()
            .map(|p| Source {
                positions: p.positions.clone(),
                indices: p.indices.clone(),
                obj: p.source_positions.clone(),
                instances: p.instances.as_ref().map(|i| i.transforms.clone()),
                lines: p.lines,
                points: p.points,
            })
            .collect();
        let curves = scene.curves.clone();
        let device = device.clone();
        let queue = queue.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let stop = cancel.clone();
        let (send, receive) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("VTX feature recognition".into())
            .spawn(move || {
                let _ = send.send(build(sources, curves, &device, &queue, &stop));
            })
            .map_err(|e| e.to_string())?;
        Ok(Self { receive, cancel })
    }
    pub fn poll(&self) -> Option<std::result::Result<Index, String>> {
        match self.receive.try_recv() {
            Ok(result) => Some(result),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(_) => Some(Err("Feature worker stopped".into())),
        }
    }
}
fn read(
    buffer: &wgpu::Buffer,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> std::result::Result<Vec<u8>, String> {
    if buffer.size() > 384 * 1024 * 1024 {
        return Err("This geometry buffer exceeds the feature-analysis memory limit.".into());
    }
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("VTX on-demand geometry readback"),
        size: buffer.size(),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_buffer_to_buffer(buffer, 0, &staging, 0, buffer.size());
    queue.submit([encoder.finish()]);
    let (tx, rx) = mpsc::sync_channel(1);
    staging.slice(..).map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    device
        .poll(wgpu::PollType::Wait)
        .map_err(|e| e.to_string())?;
    rx.recv()
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    let bytes = staging.slice(..).get_mapped_range().to_vec();
    staging.unmap();
    Ok(bytes)
}
fn floats(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
        .collect()
}
fn build(
    sources: Vec<Source>,
    curves: Vec<crate::dxf_drawing::Curve>,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    cancel: &AtomicBool,
) -> std::result::Result<Index, String> {
    use std::convert::TryInto;
    let started = std::time::Instant::now();
    let mut features = Vec::new();
    let mut curve_ranges: Vec<_> = curves.iter().map(|c| c.segments.clone()).collect();
    curve_ranges.sort_by_key(|r| r.start);
    for curve in curves {
        if curve.sweep.abs() < 1e-8 {
            continue;
        }
        let radius = length(curve.u);
        let other = length(curve.v);
        if radius <= 1e-20
            || (radius - other).abs() > radius * 1e-8
            || dot(curve.u, curve.v).abs() > radius * other * 1e-8
        {
            let count = (curve.sweep.abs() / TAU * 256.).ceil().max(8.) as usize;
            features.push(Feature::Curve {
                points: (0..=count)
                    .map(|i| {
                        let angle = curve.start + curve.sweep * i as f64 / count as f64;
                        add(
                            curve.center,
                            add(mul(curve.u, angle.cos()), mul(curve.v, angle.sin())),
                        )
                    })
                    .collect(),
            });
            continue;
        }
        features.push(Feature::Circle(Circle {
            center: curve.center,
            u: mul(curve.u, 1. / radius),
            v: mul(curve.v, 1. / other),
            radius,
            start: curve.start,
            sweep: curve.sweep,
            deviation: 0.,
            samples: 0,
            exact: true,
            step: 0.,
        }));
    }
    for source in sources {
        if cancel.load(Ordering::Relaxed) {
            return Err("Cancelled".into());
        }
        if source.points {
            continue;
        }
        let bytes = read(&source.positions, device, queue)?;
        let mut local = if source.lines {
            let mut points = Vec::new();
            let mut edges = Vec::new();
            let mut weld = HashMap::new();
            let mut range = 0;
            for (i, line) in bytes.chunks_exact(28).enumerate() {
                while range < curve_ranges.len() && curve_ranges[range].end <= i {
                    range += 1;
                }
                if range < curve_ranges.len() && curve_ranges[range].contains(&i) {
                    continue;
                }
                let values = floats(&line[..24]);
                let mut ids = [0; 2];
                for j in 0..2 {
                    let p: [f32; 3] = values[j * 3..j * 3 + 3].try_into().unwrap();
                    let key = p.map(f32::to_bits);
                    ids[j] = *weld.entry(key).or_insert_with(|| {
                        let id = points.len();
                        points.push(f64p(p));
                        id
                    });
                }
                edges.push((ids[0], ids[1]));
            }
            edge_paths(&points, &edges, 1e-6)
                .into_iter()
                .filter(|f| !matches!(f, Feature::Circle(_)))
                .collect()
        } else {
            let data = source
                .obj
                .as_ref()
                .map(|obj| read(obj, device, queue))
                .transpose()?;
            let values = floats(data.as_deref().unwrap_or(&bytes));
            let points: Vec<_> = values
                .chunks_exact(3)
                .map(|p| [p[0] as f64, p[1] as f64, p[2] as f64])
                .collect();
            let indices: Vec<u32> = if source.obj.is_some() {
                bytes
                    .chunks_exact(8)
                    .map(|b| u32::from_le_bytes(b[..4].try_into().unwrap()))
                    .collect()
            } else if let Some(index) = &source.indices {
                read(index, device, queue)?
                    .chunks_exact(4)
                    .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
                    .collect()
            } else {
                (0..points.len() as u32).collect()
            };
            let triangles: Vec<_> = indices
                .chunks_exact(3)
                .map(|t| [t[0], t[1], t[2]])
                .collect();
            analyze_mesh(&points, &triangles, cancel)?
        };
        local.extend(cylinders(&local));
        if let Some(instances) = &source.instances {
            let values = floats(&read(instances, device, queue)?);
            for m in values.chunks_exact(12) {
                let matrix =
                    [0, 1, 2].map(|r| [m[r * 4], m[r * 4 + 1], m[r * 4 + 2], m[r * 4 + 3]]);
                for f in &local {
                    if let Some(f) = transform_feature(f, matrix) {
                        features.push(f);
                    }
                }
            }
        } else {
            features.extend(local);
        }
    }
    Ok(Index {
        features,
        elapsed_ms: started.elapsed().as_secs_f64() * 1000.,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ring(radius: f64, z: f64) -> Vec<P> {
        (0..=64)
            .map(|i| {
                let a = i as f64 * TAU / 64.;
                [radius * a.cos(), radius * a.sin(), z]
            })
            .collect()
    }
    #[test]
    fn fitted_circles_reject_ellipses_lines_and_warped_loops() {
        let p = ring(12.7, 3.);
        let c = fit_circle(&p, 1e-6).unwrap();
        assert!((c.radius - 12.7).abs() < 1e-9);
        assert!(c.full());
        let ellipse: Vec<_> = p.iter().map(|p| [p[0] * 1.2, p[1], p[2]]).collect();
        assert!(fit_circle(&ellipse, 1e-6).is_none());
        let warped: Vec<_> = p
            .iter()
            .enumerate()
            .map(|(i, p)| [p[0], p[1], p[2] + (i as f64).sin()])
            .collect();
        assert!(fit_circle(&warped, 1e-6).is_none());
        assert!(fit_circle(&[[0.; 3]; 8], 1e-6).is_none());
        assert!(fit_circle(&p[..3], 1e-6).is_none());
    }
    #[test]
    fn triangulated_cylinder_has_two_rims_and_a_selectable_wall() {
        let mut points = ring(5., 0.);
        points.pop();
        let mut top = ring(5., 20.);
        top.pop();
        points.extend(top);
        points.extend([[0., 0., 0.], [0., 0., 20.]]);
        let mut triangles = Vec::new();
        for i in 0..64u32 {
            let j = (i + 1) % 64;
            triangles.extend([
                [i, j, i + 64],
                [j, j + 64, i + 64],
                [128, j, i],
                [129, i + 64, j + 64],
            ]);
        }
        let mut features = analyze_mesh(&points, &triangles, &AtomicBool::new(false)).unwrap();
        assert_eq!(
            features
                .iter()
                .filter(|f| matches!(f, Feature::Circle(_)))
                .count(),
            2
        );
        features.extend(cylinders(&features));
        assert_eq!(
            features
                .iter()
                .filter(|f| matches!(f, Feature::Cylinder { .. }))
                .count(),
            1
        );
    }
    #[test]
    fn transforms_preserve_real_circles_and_reject_nonuniform_ellipses() {
        let c = Feature::Circle(fit_circle(&ring(5., 0.), 1e-6).unwrap());
        let uniform = [[0., -2., 0., 10.], [2., 0., 0., 20.], [0., 0., 2., 30.]];
        let f = transform_feature(&c, uniform).unwrap();
        assert!((f.circle().unwrap().radius - 10.).abs() < 1e-8);
        let nonuniform = [[2., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.]];
        assert!(transform_feature(&c, nonuniform).is_none());
    }
    #[test]
    fn tangent_outline_splits_into_a_line_and_a_real_quarter_arc() {
        let mut path = vec![[5., -5., 0.], [5., -3., 0.], [5., 0., 0.]];
        path.extend((1..=32).map(|i| {
            let t = i as f64 * PI / 64.;
            [5. * t.cos(), 5. * t.sin(), 0.]
        }));
        let mut out = Vec::new();
        path_features(&path, 1e-6, &mut out);
        assert!(out.iter().any(|f| matches!(f, Feature::Line { .. })));
        assert!(out
            .iter()
            .any(|f| f.circle().is_some_and(|c| (c.radius - 5.).abs() < 1e-8)));
    }
    #[test]
    fn cancellation_stops_mesh_analysis() {
        assert!(analyze_mesh(
            &[[0., 0., 0.], [1., 0., 0.], [0., 1., 0.]],
            &[[0, 1, 2]],
            &AtomicBool::new(true)
        )
        .is_err());
    }
    #[test]
    fn a_helical_edge_is_one_curve_and_not_a_circle_or_many_selectable_facets() {
        let path: Vec<_> = (0..100)
            .map(|i| {
                let t = i as f64 * TAU / 100.;
                [5. * t.cos(), 5. * t.sin(), t * 0.08]
            })
            .collect();
        let mut out = Vec::new();
        path_features(&path, 1e-6, &mut out);
        assert_eq!(out.len(), 1);
        assert!(matches!(&out[0],Feature::Curve{points} if points.len()==100));
        assert_eq!(out[0].anchors().len(), 3);
        assert!(out[0].length() > 30.);
    }
    #[test]
    fn rejected_arc_at_the_end_of_a_path_is_bounded() {
        let mut path: Vec<_> = (0..100)
            .map(|i| {
                let a = i as f64 * PI / 200.;
                [5. * a.cos(), 5. * a.sin(), 0.]
            })
            .collect();
        path.last_mut().unwrap()[2] = 0.01;
        let mut features = Vec::new();
        path_features(&path, 1e-6, &mut features);
        assert!(!features.is_empty());
    }
}
