//! Read-only DXF drawing display. dxf-rs handles ASCII/binary decoding;
//! this module converts supported entities into bounded colored line segments.
use dxf::{
    entities::{Entity, EntityType, Spline},
    Block, Color, Drawing, Point, Vector,
};
use std::{
    collections::{BTreeMap, HashMap},
    f64::consts::TAU,
    path::Path,
};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
type P = [f64; 3];
type Matrix = [[f64; 4]; 3];
const IDENTITY: Matrix = [[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.]];
const LIMIT: usize = 2_000_000;
pub struct Segment {
    pub a: [f32; 3],
    pub b: [f32; 3],
    pub color: [u8; 4],
}
pub struct Scene {
    pub segments: Vec<Segment>,
    pub curves: Vec<Curve>,
    pub entities: usize,
    pub diagnostics: Vec<String>,
    pub issues: usize,
}
#[derive(Clone)]
pub struct Curve {
    pub segments: std::ops::Range<usize>,
    pub center: P,
    pub u: P,
    pub v: P,
    pub start: f64,
    pub sweep: f64,
}
fn point(p: &Point) -> P {
    [p.x, p.y, p.z]
}
fn vector(p: &Vector) -> P {
    [p.x, p.y, p.z]
}
fn cross(a: P, b: P) -> P {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn unit(a: P) -> Option<P> {
    let length = a.iter().map(|v| v * v).sum::<f64>().sqrt();
    (length.is_finite() && length > 1e-20).then(|| a.map(|x| x / length))
}
fn transform(m: Matrix, p: P) -> P {
    [0, 1, 2].map(|a| m[a][3] + (0..3).map(|i| m[a][i] * p[i]).sum::<f64>())
}
fn multiply(a: Matrix, b: Matrix) -> Matrix {
    [0, 1, 2].map(|r| {
        [0, 1, 2, 3].map(|c| {
            (0..3).map(|k| a[r][k] * b[k][c]).sum::<f64>() + if c == 3 { a[r][3] } else { 0. }
        })
    })
}
// Autodesk's arbitrary axis algorithm maps OCS coordinates into WCS.
fn ocs(normal: P) -> Result<Matrix> {
    let n = unit(normal).ok_or("DXF has an invalid extrusion normal")?;
    let up = if n[0].abs() < 1. / 64. && n[1].abs() < 1. / 64. {
        [0., 1., 0.]
    } else {
        [0., 0., 1.]
    };
    let x = unit(cross(up, n)).ok_or("Invalid DXF OCS axes")?;
    let y = cross(n, x);
    Ok([0, 1, 2].map(|r| [x[r], y[r], n[r], 0.]))
}
fn indexed(index: u8) -> [u8; 4] {
    let rgb = match index {
        1 => [255, 0, 0],
        2 => [255, 255, 0],
        3 => [0, 255, 0],
        4 => [0, 255, 255],
        5 => [0, 0, 255],
        6 => [255, 0, 255],
        7 => return [0; 4],
        8 => [128, 128, 128],
        9 => [192, 192, 192],
        250..=255 => {
            let v = [51, 80, 105, 130, 190, 255][(index - 250) as usize];
            [v, v, v]
        }
        _ => {
            let h = ((index.saturating_sub(10)) / 10) as f64 / 24. * 6.;
            let v = [255., 255., 165., 165., 127., 127., 76., 76., 38., 38.]
                [(index.saturating_sub(10) % 10) as usize]
                / 255.;
            let saturation = if index % 2 == 0 { 1. } else { 0.5 };
            let c = v * saturation;
            let x = c * (1. - (h % 2. - 1.).abs());
            let m = v - c;
            let p = match h as usize {
                0 => [c, x, 0.],
                1 => [x, c, 0.],
                2 => [0., c, x],
                3 => [0., x, c],
                4 => [x, 0., c],
                _ => [c, 0., x],
            };
            p.map(|n| ((n + m) * 255.) as u8)
        }
    };
    [rgb[0], rgb[1], rgb[2], 255]
}
fn color(c: &Color, true_color: i32, layer: [u8; 4], parent: [u8; 4]) -> [u8; 4] {
    if true_color != 0 {
        return [
            ((true_color >> 16) & 255) as u8,
            ((true_color >> 8) & 255) as u8,
            (true_color & 255) as u8,
            255,
        ];
    }
    if c.is_by_block() {
        parent
    } else {
        c.index().map(indexed).unwrap_or(layer)
    }
}
struct Layer {
    visible: bool,
    color: [u8; 4],
}
struct Builder<'a> {
    blocks: HashMap<&'a str, &'a Block>,
    layers: HashMap<&'a str, Layer>,
    lines: Vec<(P, P, [u8; 4])>,
    curves: Vec<Curve>,
    unsupported: BTreeMap<&'static str, usize>,
    entities: usize,
    visited: usize,
}
impl Builder<'_> {
    fn skip(&mut self, name: &'static str) {
        *self.unsupported.entry(name).or_default() += 1;
    }
    fn line(&mut self, a: P, b: P, m: Matrix, color: [u8; 4]) -> Result<()> {
        if self.lines.len() >= LIMIT {
            return Err("DXF exceeds the two-million-segment display limit".into());
        }
        let a = transform(m, a);
        let b = transform(m, b);
        if a.iter()
            .chain(b.iter())
            .any(|v| !v.is_finite() || v.abs() > 1e30)
        {
            return Err("DXF contains invalid coordinates".into());
        }
        if a != b {
            self.lines.push((a, b, color))
        }
        Ok(())
    }
    fn curve(
        &mut self,
        center: P,
        u: P,
        v: P,
        start: f64,
        sweep: f64,
        m: Matrix,
        color: [u8; 4],
    ) -> Result<()> {
        if !start.is_finite()
            || !sweep.is_finite()
            || sweep.abs() > TAU + 1e-7
            || u.iter().chain(v.iter()).any(|x| !x.is_finite())
        {
            return Err("Invalid DXF curve".into());
        }
        let count = (sweep.abs() / TAU * 256.).ceil().max(1.) as usize;
        let origin = transform(m, center);
        let u_end = transform(m, [0, 1, 2].map(|i| center[i] + u[i]));
        let v_end = transform(m, [0, 1, 2].map(|i| center[i] + v[i]));
        let first_segment = self.lines.len();
        let at =
            |angle: f64| [0, 1, 2].map(|a| center[a] + u[a] * angle.cos() + v[a] * angle.sin());
        let mut previous = at(start);
        for i in 1..=count {
            let next = at(start + sweep * i as f64 / count as f64);
            self.line(previous, next, m, color)?;
            previous = next
        }
        self.curves.push(Curve {
            segments: first_segment..self.lines.len(),
            center: origin,
            u: [0, 1, 2].map(|i| u_end[i] - origin[i]),
            v: [0, 1, 2].map(|i| v_end[i] - origin[i]),
            start,
            sweep,
        });
        Ok(())
    }
    fn polyline(
        &mut self,
        vertices: &[(P, f64)],
        closed: bool,
        m: Matrix,
        color: [u8; 4],
    ) -> Result<()> {
        if vertices.len() > LIMIT {
            return Err("DXF polyline exceeds the display limit".into());
        }
        for i in 0..vertices.len().saturating_sub(if closed { 0 } else { 1 }) {
            let (a, bulge) = vertices[i];
            let b = vertices[(i + 1) % vertices.len()].0;
            if !bulge.is_finite() {
                return Err("Invalid DXF polyline bulge".into());
            }
            if bulge.abs() < 1e-12 {
                self.line(a, b, m, color)?;
                continue;
            }
            let dx = b[0] - a[0];
            let dy = b[1] - a[1];
            let chord = dx.hypot(dy);
            if chord < 1e-15 {
                continue;
            }
            let offset = (1. - bulge * bulge) / (4. * bulge);
            let center = [
                (a[0] + b[0]) * 0.5 - dy * offset,
                (a[1] + b[1]) * 0.5 + dx * offset,
                a[2],
            ];
            let radius = chord * (1. + bulge * bulge) / (4. * bulge.abs());
            self.curve(
                center,
                [radius, 0., 0.],
                [0., radius, 0.],
                (a[1] - center[1]).atan2(a[0] - center[0]),
                4. * bulge.atan(),
                m,
                color,
            )?;
        }
        Ok(())
    }
    fn entity(
        &mut self,
        e: &Entity,
        m: Matrix,
        parent_layer: &str,
        parent_color: [u8; 4],
        stack: &mut Vec<String>,
    ) -> Result<()> {
        self.visited += 1;
        if self.visited > LIMIT {
            return Err("DXF exceeds the entity/instance display limit".into());
        }
        if !e.common.is_visible {
            return Ok(());
        }
        let layer = if e.common.layer == "0" {
            parent_layer
        } else {
            &e.common.layer
        };
        if self.layers.get(layer).is_some_and(|l| !l.visible) {
            return Ok(());
        }
        let layer_color = self.layers.get(layer).map_or([0; 4], |l| l.color);
        let color = color(
            &e.common.color,
            e.common.color_24_bit,
            layer_color,
            parent_color,
        );
        self.entities += 1;
        match &e.specific {
            EntityType::Line(p) => self.line(point(&p.p1), point(&p.p2), m, color)?,
            EntityType::Circle(p) => {
                let m = multiply(m, ocs(vector(&p.normal))?);
                self.curve(
                    point(&p.center),
                    [p.radius, 0., 0.],
                    [0., p.radius, 0.],
                    0.,
                    TAU,
                    m,
                    color,
                )?;
            }
            EntityType::Arc(p) => {
                let m = multiply(m, ocs(vector(&p.normal))?);
                let delta = p.end_angle - p.start_angle;
                let sweep = if delta.abs() >= 360. - 1e-8 {
                    360.
                } else {
                    delta.rem_euclid(360.)
                };
                self.curve(
                    point(&p.center),
                    [p.radius, 0., 0.],
                    [0., p.radius, 0.],
                    p.start_angle.to_radians(),
                    sweep.to_radians(),
                    m,
                    color,
                )?;
            }
            EntityType::LwPolyline(p) => {
                let m = multiply(m, ocs(vector(&p.extrusion_direction))?);
                let v: Vec<_> = p
                    .vertices
                    .iter()
                    .map(|v| ([v.x, v.y, e.common.elevation], v.bulge))
                    .collect();
                self.polyline(&v, p.is_closed(), m, color)?;
            }
            EntityType::Polyline(p) => {
                if p.is_polyface_mesh() || p.is_3d_polygon_mesh() {
                    self.skip("polygon/polyface mesh");
                    return Ok(());
                }
                let m = if p.is_3d_polyline() {
                    m
                } else {
                    multiply(m, ocs(vector(&p.normal))?)
                };
                let v: Vec<_> = p
                    .vertices()
                    .map(|v| {
                        let mut at = point(&v.location);
                        if !p.is_3d_polyline() {
                            at[2] = p.location.z
                        }
                        (at, v.bulge)
                    })
                    .collect();
                self.polyline(&v, p.is_closed(), m, color)?;
            }
            EntityType::Ellipse(p) => {
                let u = vector(&p.major_axis);
                let n = unit(vector(&p.normal)).ok_or("Invalid DXF ellipse normal")?;
                let v = cross(n, u).map(|x| x * p.minor_axis_ratio);
                let delta = p.end_parameter - p.start_parameter;
                let sweep = if delta.abs() >= TAU - 1e-8 {
                    TAU
                } else {
                    delta.rem_euclid(TAU)
                };
                self.curve(point(&p.center), u, v, p.start_parameter, sweep, m, color)?;
            }
            EntityType::Spline(p) => self.spline(p, m, color)?,
            EntityType::Insert(p) => {
                if stack.len() >= 32 || stack.contains(&p.name) {
                    self.skip("recursive block reference");
                    return Ok(());
                }
                let Some(block) = self.blocks.get(p.name.as_str()).copied() else {
                    self.skip("missing/external block");
                    return Ok(());
                };
                if !block.xref_path_name.is_empty() {
                    self.skip("external reference");
                    return Ok(());
                }
                let (s, c) = p.rotation.to_radians().sin_cos();
                let scales = [p.x_scale_factor, p.y_scale_factor, p.z_scale_factor];
                let base = point(&block.base_point);
                let rotation = [[c, -s, 0., 0.], [s, c, 0., 0.], [0., 0., 1., 0.]];
                let scaled = [0, 1, 2].map(|r| {
                    [0, 1, 2, 3].map(|k| {
                        if k == r {
                            scales[r]
                        } else if k == 3 {
                            -base[r] * scales[r]
                        } else {
                            0.
                        }
                    })
                });
                let outer = multiply(m, ocs(vector(&p.extrusion_direction))?);
                let rows = p.row_count.max(1) as usize;
                let columns = p.column_count.max(1) as usize;
                if rows.saturating_mul(columns) > LIMIT {
                    return Err("DXF block array exceeds the display limit".into());
                }
                stack.push(p.name.clone());
                for row in 0..rows {
                    for column in 0..columns {
                        let mut offset = IDENTITY;
                        offset[0][3] = column as f64 * p.column_spacing;
                        offset[1][3] = row as f64 * p.row_spacing;
                        let mut placement = multiply(rotation, multiply(offset, scaled));
                        let location = point(&p.location);
                        for a in 0..3 {
                            placement[a][3] += location[a]
                        }
                        let transform = multiply(outer, placement);
                        for child in &block.entities {
                            self.entity(child, transform, layer, color, stack)?
                        }
                    }
                }
                stack.pop();
            }
            EntityType::Face3D(p) => self.polyline(
                &[
                    (point(&p.first_corner), 0.),
                    (point(&p.second_corner), 0.),
                    (point(&p.third_corner), 0.),
                    (point(&p.fourth_corner), 0.),
                ],
                true,
                m,
                color,
            )?,
            EntityType::Solid(p) => self.polyline(
                &[
                    (point(&p.first_corner), 0.),
                    (point(&p.second_corner), 0.),
                    (point(&p.fourth_corner), 0.),
                    (point(&p.third_corner), 0.),
                ],
                true,
                m,
                color,
            )?,
            EntityType::ModelPoint(p) => {
                let a = point(&p.location);
                self.line(a, [a[0] + 0.01, a[1], a[2]], m, color)?;
            }
            EntityType::Text(_)
            | EntityType::MText(_)
            | EntityType::Attribute(_)
            | EntityType::AttributeDefinition(_) => self.skip("text/attributes"),
            _ => self.skip("other entity types (including hatches/dimensions)"),
        }
        Ok(())
    }
    fn spline(&mut self, p: &Spline, m: Matrix, color: [u8; 4]) -> Result<()> {
        let degree = p.degree_of_curve as usize;
        let n = p.control_points.len();
        let knots = &p.knot_values;
        if !(1..=16).contains(&degree)
            || n <= degree
            || n > LIMIT
            || knots.len() != n + degree + 1
            || (!p.weight_values.is_empty() && p.weight_values.len() != n)
            || knots.windows(2).any(|w| !w[0].is_finite() || w[0] > w[1])
            || !knots.last().is_some_and(|v| v.is_finite())
        {
            self.skip("unsupported spline definition");
            return Ok(());
        }
        let control: Vec<_> = p
            .control_points
            .iter()
            .enumerate()
            .map(|(i, cp)| {
                let w = p_weight(&p.weight_values, i);
                [cp.x * w, cp.y * w, cp.z * w, w]
            })
            .collect();
        if control.iter().flatten().any(|v| !v.is_finite()) || control.iter().any(|p| p[3] <= 0.) {
            return Err("Invalid DXF spline weights/control points".into());
        }
        let mut previous = None;
        for span in degree..n {
            let lo = knots[span];
            let hi = knots[span + 1];
            if hi <= lo {
                continue;
            }
            for step in 0..=24 {
                let t = lo + (hi - lo) * step as f64 / 24.;
                let mut work = control[span - degree..=span].to_vec();
                for r in 1..=degree {
                    for j in (r..=degree).rev() {
                        let i = span - degree + j;
                        let denom = knots[i + degree - r + 1] - knots[i];
                        let alpha = if denom > 0. {
                            (t - knots[i]) / denom
                        } else {
                            0.
                        };
                        work[j] = [0, 1, 2, 3]
                            .map(|a| (1. - alpha) * work[j - 1][a] + alpha * work[j][a]);
                    }
                }
                let value = work[degree];
                let at = [
                    value[0] / value[3],
                    value[1] / value[3],
                    value[2] / value[3],
                ];
                if let Some(old) = previous {
                    self.line(old, at, m, color)?
                }
                previous = Some(at);
            }
        }
        Ok(())
    }
}
fn p_weight(weights: &[f64], i: usize) -> f64 {
    if weights.is_empty() {
        1.
    } else {
        weights[i]
    }
}
pub fn load(path: &Path) -> Result<Scene> {
    if std::fs::metadata(path)?.len() > 128 * 1024 * 1024 {
        return Err("DXF exceeds the 128 MiB input limit".into());
    }
    let drawing = Drawing::load_file(path)?;
    let mut builder = Builder {
        blocks: drawing.blocks().map(|b| (b.name.as_str(), b)).collect(),
        layers: drawing
            .layers()
            .map(|l| {
                (
                    l.name.as_str(),
                    Layer {
                        visible: l.is_layer_on,
                        color: color(&l.color, 0, [0; 4], [0; 4]),
                    },
                )
            })
            .collect(),
        lines: Vec::new(),
        curves: Vec::new(),
        unsupported: BTreeMap::new(),
        entities: 0,
        visited: 0,
    };
    let use_paper = !drawing
        .entities()
        .any(|e| !e.common.is_in_paper_space && e.common.is_visible);
    for e in drawing
        .entities()
        .filter(|e| e.common.is_in_paper_space == use_paper)
    {
        builder.entity(e, IDENTITY, "0", [0; 4], &mut Vec::new())?
    }
    if builder.lines.is_empty() {
        return Err(format!(
            "DXF contains no supported visible drawing geometry. Skipped: {:?}",
            builder.unsupported
        )
        .into());
    }
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for (a, b, _) in &builder.lines {
        for p in [a, b] {
            for axis in 0..3 {
                lo[axis] = lo[axis].min(p[axis]);
                hi[axis] = hi[axis].max(p[axis]);
            }
        }
    }
    let spans = [0, 1, 2].map(|i| hi[i] - lo[i]);
    let axes = if spans[0] < spans[2] * 1e-8 {
        [1, 2]
    } else if spans[1] < spans[2] * 1e-8 {
        [0, 2]
    } else {
        [0, 1]
    };
    let center = [0, 1, 2].map(|i| lo[i] * 0.5 + hi[i] * 0.5);
    let at = |p: P| {
        [
            (p[axes[0]] - center[axes[0]]) as f32,
            (p[axes[1]] - center[axes[1]]) as f32,
            0.,
        ]
    };
    let mut curves = builder.curves;
    let mut boundaries: Vec<_> = curves
        .iter()
        .enumerate()
        .flat_map(|(i, c)| [(c.segments.start, i, true), (c.segments.end, i, false)])
        .collect();
    boundaries.sort_unstable();
    let mut boundary = 0;
    let mut segments = Vec::with_capacity(builder.lines.len());
    let line_count = builder.lines.len();
    for (i, (a, b, color)) in builder.lines.into_iter().enumerate() {
        while boundary < boundaries.len() && boundaries[boundary].0 == i {
            let (_, curve, start) = boundaries[boundary];
            if start {
                curves[curve].segments.start = segments.len();
            } else {
                curves[curve].segments.end = segments.len();
            }
            boundary += 1;
        }
        let a = at(a);
        let b = at(b);
        if a != b {
            segments.push(Segment { a, b, color });
        }
    }
    while boundary < boundaries.len() && boundaries[boundary].0 == line_count {
        let (_, curve, start) = boundaries[boundary];
        if start {
            curves[curve].segments.start = segments.len();
        } else {
            curves[curve].segments.end = segments.len();
        }
        boundary += 1;
    }
    if segments.is_empty() {
        return Err("DXF has no lines visible in the drawing plane".into());
    }
    let mut diagnostics=vec![format!("DXF drawing: {} visible entities, {} curve/line segments. Drag to pan, wheel to zoom, R to fit.",builder.entities,segments.len())];
    let issues = builder.unsupported.values().sum();
    for (name, count) in builder.unsupported {
        diagnostics.push(format!("Skipped {count} {name}."));
    }
    Ok(Scene {
        segments,
        curves: curves
            .into_iter()
            .map(|c| Curve {
                segments: c.segments,
                center: [
                    (c.center[axes[0]] - center[axes[0]]),
                    (c.center[axes[1]] - center[axes[1]]),
                    0.,
                ],
                u: [c.u[axes[0]], c.u[axes[1]], 0.],
                v: [c.v[axes[0]], c.v[axes[1]], 0.],
                start: c.start,
                sweep: c.sweep,
            })
            .collect(),
        entities: builder.entities,
        diagnostics,
        issues,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn builder<'a>() -> Builder<'a> {
        Builder {
            blocks: HashMap::new(),
            layers: HashMap::new(),
            lines: Vec::new(),
            curves: Vec::new(),
            unsupported: BTreeMap::new(),
            entities: 0,
            visited: 0,
        }
    }
    fn near(a: P, b: P) {
        assert!(
            (0..3).all(|i| (a[i] - b[i]).abs() < 1e-10),
            "{:?} != {:?}",
            a,
            b
        );
    }
    #[test]
    fn bulges_keep_the_signed_arc_instead_of_closing_with_a_chord() {
        for sign in [1., -1.] {
            let mut b = builder();
            b.polyline(
                &[([-1., 0., 0.], sign), ([1., 0., 0.], 0.)],
                false,
                IDENTITY,
                [0; 4],
            )
            .unwrap();
            assert_eq!(b.lines.len(), 128);
            near(b.lines[0].0, [-1., 0., 0.]);
            near(b.lines.last().unwrap().1, [1., 0., 0.]);
            near(b.lines[63].1, [0., -sign, 0.]);
        }
        near(
            transform(ocs([0., 0., -1.]).unwrap(), [1., 2., 3.]),
            [-1., 2., -3.],
        );
    }
    #[test]
    fn blocks_subtract_base_then_scale_rotate_and_place_arrays() {
        let mut line = Entity::new(EntityType::Line(dxf::entities::Line::new(
            Point::new(10., 20., 0.),
            Point::new(11., 20., 0.),
        )));
        line.common.color = Color::by_block();
        let block = Block {
            name: "test".into(),
            base_point: Point::new(10., 20., 0.),
            entities: vec![line],
            ..Default::default()
        };
        let mut b = builder();
        b.blocks.insert("test", &block);
        let mut insert = Entity::new(EntityType::Insert(dxf::entities::Insert {
            name: "test".into(),
            location: Point::new(100., 200., 0.),
            x_scale_factor: 2.,
            y_scale_factor: 3.,
            rotation: 90.,
            row_count: 2,
            row_spacing: 10.,
            ..Default::default()
        }));
        insert.common.color = Color::from_index(3);
        b.entity(&insert, IDENTITY, "0", [0; 4], &mut Vec::new())
            .unwrap();
        assert_eq!(b.lines.len(), 2);
        near(b.lines[0].0, [100., 200., 0.]);
        near(b.lines[0].1, [100., 202., 0.]);
        near(b.lines[1].0, [90., 200., 0.]);
        assert_eq!(b.lines[0].2, [0, 255, 0, 255]);
        insert.common.is_visible = false;
        b.entity(&insert, IDENTITY, "0", [0; 4], &mut Vec::new())
            .unwrap();
        assert_eq!(b.lines.len(), 2);
    }
    #[test]
    fn rational_spline_preserves_a_quarter_circle() {
        let p = Spline {
            degree_of_curve: 2,
            control_points: vec![
                Point::new(1., 0., 0.),
                Point::new(1., 1., 0.),
                Point::new(0., 1., 0.),
            ],
            knot_values: vec![0., 0., 0., 1., 1., 1.],
            weight_values: vec![1., 0.5f64.sqrt(), 1.],
            ..Default::default()
        };
        let mut b = builder();
        b.spline(&p, IDENTITY, [0; 4]).unwrap();
        assert_eq!(b.lines.len(), 24);
        near(b.lines[0].0, [1., 0., 0.]);
        near(b.lines.last().unwrap().1, [0., 1., 0.]);
        for (a, c, _) in b.lines {
            for p in [a, c] {
                assert!((p[0] * p[0] + p[1] * p[1] - 1.).abs() < 1e-12);
            }
        }
    }
    #[test]
    fn hidden_layers_cycles_and_invalid_coordinates_cannot_create_spurious_lines() {
        let line = Entity::new(EntityType::Line(dxf::entities::Line::new(
            Point::origin(),
            Point::new(1., 0., 0.),
        )));
        let block = Block {
            name: "cycle".into(),
            entities: vec![Entity::new(EntityType::Insert(dxf::entities::Insert {
                name: "cycle".into(),
                ..Default::default()
            }))],
            ..Default::default()
        };
        let mut b = builder();
        b.blocks.insert("cycle", &block);
        b.entity(&block.entities[0], IDENTITY, "0", [0; 4], &mut Vec::new())
            .unwrap();
        assert!(b.lines.is_empty());
        assert_eq!(b.unsupported["recursive block reference"], 1);
        b.layers.insert(
            "0",
            Layer {
                visible: false,
                color: [0; 4],
            },
        );
        b.entity(&line, IDENTITY, "0", [0; 4], &mut Vec::new())
            .unwrap();
        assert!(b.lines.is_empty());
        assert!(b
            .line([f64::NAN, 0., 0.], [1., 0., 0.], IDENTITY, [0; 4])
            .is_err());
        assert!(ocs([0.; 3]).is_err());
    }
}
