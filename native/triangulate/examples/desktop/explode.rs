use std::sync::{Mutex, OnceLock};

use crate::three_mf::Instance;

pub const IDENTITY: [(Instance, bool); 1] = [(
    [[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.]],
    false,
)];

pub struct PlacementBatch {
    pub buffer: wgpu::Buffer,
    pub base: Vec<Instance>,
    pub components: Vec<usize>,
}

pub struct Explosion {
    pub count: usize,
    batches: Vec<PlacementBatch>,
    offsets: OnceLock<Vec<[f32; 3]>>,
    legacy_offsets: OnceLock<Vec<[f32; 3]>>,
    components: Vec<Option<[[f32; 3]; 2]>>,
    center: [f32; 3],
    diameter: f32,
    applied: Mutex<(f32, bool)>,
}

impl Explosion {
    pub fn new(
        components: Vec<Option<[[f32; 3]; 2]>>,
        batches: Vec<PlacementBatch>,
        center: [f32; 3],
        diameter: f32,
    ) -> Option<Self> {
        let count = components.iter().flatten().count();
        if count < 2 {
            return None;
        }
        Some(Self {
            count,
            batches,
            offsets: OnceLock::new(),
            legacy_offsets: OnceLock::new(),
            components,
            center,
            diameter,
            applied: Mutex::new((0., false)),
        })
    }

    pub fn bounds(&self, mut bounds: [[f32; 3]; 2], amount: f32, legacy: bool) -> [[f32; 3]; 2] {
        if amount == 0. && bounds[0][0].is_finite() {
            return bounds;
        }
        if legacy {
            let mut extent = [0.0f32; 3];
            for offset in self.offsets(true) {
                for axis in 0..3 {
                    extent[axis] = extent[axis].max(offset[axis].abs());
                }
            }
            for axis in 0..3 {
                bounds[0][axis] -= extent[axis] * amount;
                bounds[1][axis] += extent[axis] * amount;
            }
            return bounds;
        }
        for (part, offset) in self.components.iter().zip(self.offsets(false)) {
            if let Some([low, high]) = part {
                for axis in 0..3 {
                    bounds[0][axis] = bounds[0][axis].min(low[axis] + offset[axis] * amount);
                    bounds[1][axis] = bounds[1][axis].max(high[axis] + offset[axis] * amount);
                }
            }
        }
        bounds
    }

    pub fn framing_scale(&self, diameter: f32, amount: f32) -> f32 {
        if amount == 0. {
            return 1.;
        }
        let bounds = self.bounds([[f32::INFINITY; 3], [f32::NEG_INFINITY; 3]], amount, false);
        let span = std::array::from_fn::<_, 3, _>(|axis| {
            (f64::from(bounds[1][axis]) - f64::from(bounds[0][axis])) / f64::from(diameter)
        });
        span.iter()
            .map(|value| value * value)
            .sum::<f64>()
            .sqrt()
            .max(1.) as f32
    }

    fn offsets(&self, legacy: bool) -> &[[f32; 3]] {
        if legacy {
            return self.legacy_offsets.get_or_init(|| {
                let mut coincident = std::collections::HashMap::<[i64; 3], usize>::new();
                self.components
                    .iter()
                    .map(|bounds| {
                        let Some([low, high]) = bounds else {
                            return [0.; 3];
                        };
                        let relative =
                            AXES.map(|axis| low[axis] * 0.5 + high[axis] * 0.5 - self.center[axis]);
                        let key = relative.map(|v| {
                            (f64::from(v) / f64::from(self.diameter) * 100_000.).round() as i64
                        });
                        let rank = coincident.entry(key).or_default();
                        let mut offset = relative.map(|v| v * 1.15);
                        if *rank > 0 || key == [0; 3] {
                            let angle = (*rank as f32 + 1.) * 2.399_963;
                            let step =
                                self.diameter * (0.12 + 0.04 * (*rank as f32).sqrt()).min(0.5);
                            let direction = [angle.cos(), angle.sin(), (angle * 0.71).sin() * 0.6];
                            for axis in AXES {
                                offset[axis] += direction[axis] * step;
                            }
                        }
                        *rank += 1;
                        offset
                    })
                    .collect()
            });
        }
        self.offsets
            .get_or_init(|| plan(&self.components, self.center, self.diameter))
    }

    pub fn displacement(&self, component: usize, axis: usize, amount: f32, legacy: bool) -> f32 {
        if amount == 0. {
            0.
        } else {
            self.offsets(legacy)[component][axis] * amount
        }
    }

    pub fn guides(&self, amount: f32) -> Vec<([f32; 3], [f32; 3])> {
        self.components
            .iter()
            .zip(self.offsets(false))
            .filter_map(|(bounds, &offset)| {
                let [lo, hi] = bounds.as_ref()?;
                if offset == [0.; 3] {
                    return None;
                }
                let start = AXES.map(|i| lo[i] * 0.5 + hi[i] * 0.5);
                let end = AXES.map(|i| start[i] + offset[i] * amount);
                Some((start, end))
            })
            .collect()
    }

    pub fn apply(&self, queue: &wgpu::Queue, amount: f32, legacy: bool) {
        let mut applied = self.applied.lock().unwrap();
        let state = (amount, legacy && amount != 0.);
        if *applied == state {
            return;
        }
        let offsets = self.offsets(legacy);
        for batch in &self.batches {
            let size = wgpu::BufferSize::new(batch.buffer.size()).unwrap();
            let fill = |matrices: &mut [Instance]| {
                for ((matrix, base), &component) in
                    matrices.iter_mut().zip(&batch.base).zip(&batch.components)
                {
                    *matrix = *base;
                    for (row, offset) in matrix.iter_mut().zip(offsets[component]) {
                        row[3] += offset * amount;
                    }
                }
            };
            if let Some(mut upload) = queue.write_buffer_with(&batch.buffer, 0, size) {
                fill(bytemuck::cast_slice_mut(&mut upload));
            } else {
                let mut matrices = batch.base.clone();
                fill(&mut matrices);
                queue.write_buffer(&batch.buffer, 0, bytemuck::cast_slice(&matrices));
            }
        }
        *applied = state;
    }
}

type Bounds = [[f64; 3]; 2];
type Motion = [f64; 3];
const AXES: [usize; 3] = [0, 1, 2];
const GAP: f64 = 0.085;
const EPSILON: f64 = 1e-6;

#[derive(Clone, Copy)]
struct Component {
    bounds: Bounds,
    size: Motion,
    center: Motion,
    motion: Motion,
    id: usize,
}

impl Component {
    fn area(self) -> f64 {
        let [x, y, z] = self.size;
        (x * y).max(y * z).max(z * x)
    }

    fn overlaps_across(self, other: Self, axis: usize, fraction: f64) -> bool {
        AXES.iter().copied().all(|i| {
            i == axis
                || self.bounds[1][i].min(other.bounds[1][i])
                    - self.bounds[0][i].max(other.bounds[0][i])
                    > EPSILON.max(fraction * self.size[i].min(other.size[i]))
        })
    }

    fn distance_past(self, other: Self, axis: usize, sign: f64) -> f64 {
        if sign > 0. {
            other.bounds[1][axis] - self.bounds[0][axis]
        } else {
            self.bounds[1][axis] - other.bounds[0][axis]
        }
    }

    fn near(self, axis: usize, sign: f64) -> f64 {
        if sign > 0. {
            self.bounds[0][axis]
        } else {
            -self.bounds[1][axis]
        }
    }
}

fn crosses_during_move(part: Component, other: Component, motion: Motion) -> bool {
    let mut enter: f64 = 0.;
    let mut exit: f64 = 1.;
    let mut initially_overlapping = true;
    for axis in AXES {
        let speed = motion[axis] - other.motion[axis];
        let low = other.bounds[0][axis] - part.bounds[1][axis] + EPSILON;
        let high = other.bounds[1][axis] - part.bounds[0][axis] - EPSILON;
        initially_overlapping &= low < 0. && high > 0.;
        if speed.abs() < 1e-8 {
            if low >= 0. || high <= 0. {
                return false;
            }
        } else {
            let a = low / speed;
            let b = high / speed;
            enter = enter.max(a.min(b));
            exit = exit.min(a.max(b));
            if exit <= enter {
                return false;
            }
        }
    }
    !initially_overlapping
}

fn plan(components: &[Option<[[f32; 3]; 2]>], center: [f32; 3], diameter: f32) -> Vec<[f32; 3]> {
    let scale = 2. / f64::from(diameter);
    let mut parts: Vec<_> = components
        .iter()
        .enumerate()
        .filter_map(|(id, bounds)| {
            let bounds = bounds.as_ref()?.map(|point| {
                AXES.map(|axis| (f64::from(point[axis]) - f64::from(center[axis])) * scale)
            });
            Some(Component {
                bounds,
                size: AXES.map(|axis| bounds[1][axis] - bounds[0][axis]),
                center: AXES.map(|axis| (bounds[0][axis] + bounds[1][axis]) * 0.5),
                motion: [0.; 3],
                id,
            })
        })
        .collect();
    let mut offsets = vec![[0.; 3]; components.len()];
    if parts.len() < 2 {
        return offsets;
    }
    let mut anchor_index = 0;
    for index in 1..parts.len() {
        if parts[index].area() > parts[anchor_index].area() {
            anchor_index = index;
        }
    }
    let anchor = parts.remove(anchor_index);
    let mut axes_by_size = AXES;
    axes_by_size.sort_by(|&a, &b| anchor.size[a].total_cmp(&anchor.size[b]));
    let [shortest, middle, longest] = axes_by_size;
    let thin = (anchor.size[shortest] < anchor.size[middle] * 0.46).then_some(shortest);
    let long = (anchor.size[longest] > anchor.size[middle] * 4.).then_some(longest);
    let mut lanes: [Vec<Component>; 6] = std::array::from_fn(|_| Vec::new());
    for part in parts {
        let relative = AXES.map(|axis| part.center[axis] - anchor.center[axis]);
        let (axis, sign) =
            if let Some(axis) = thin.filter(|&axis| part.overlaps_across(anchor, axis, 0.16)) {
                (axis, if relative[axis] < -EPSILON { -1. } else { 1. })
            } else {
                let mut best = (f64::INFINITY, 0, 1.);
                for axis in AXES {
                    for sign in [1., -1.] {
                        let score = part.distance_past(anchor, axis, sign).max(0.)
                            - relative[axis] * sign * 0.25;
                        if score < best.0 {
                            best = (score, axis, sign);
                        }
                    }
                }
                (best.1, best.2)
            };
        lanes[axis * 2 + usize::from(sign < 0.)].push(part);
    }
    let mut ordered = Vec::new();
    for (lane_index, lane) in lanes.iter_mut().enumerate() {
        let axis = lane_index / 2;
        let sign = if lane_index % 2 == 0 { 1. } else { -1. };
        lane.sort_by(|a, b| {
            a.near(axis, sign)
                .total_cmp(&b.near(axis, sign))
                .then(a.id.cmp(&b.id))
        });
        let mut furthest = f64::NEG_INFINITY;
        for index in 0..lane.len() {
            let part = lane[index];
            let mut distance = part.distance_past(anchor, axis, sign).max(0.) + GAP;
            let first = index.saturating_sub(128);
            if let Some(old) = first.checked_sub(1).map(|i| lane[i]) {
                furthest =
                    furthest.max(old.near(axis, sign) + old.size[axis] + sign * old.motion[axis]);
                distance = distance.max(furthest + GAP - part.near(axis, sign));
            }
            for &other in &lane[first..index] {
                if part.overlaps_across(other, axis, 0.) {
                    distance = distance.max(
                        part.distance_past(other, axis, sign) + sign * other.motion[axis] + GAP,
                    );
                }
            }
            lane[index].motion[axis] = distance * sign;
            ordered.push(lane[index]);
        }
    }
    if ordered.len() < 160 {
        ordered.sort_by(|a, b| {
            let distance =
                |part: &Component| part.motion.iter().map(|value| value * value).sum::<f64>();
            distance(a).total_cmp(&distance(b)).then(a.id.cmp(&b.id))
        });
        let mut placed = vec![anchor];
        let mut intervals = Vec::<[f64; 2]>::with_capacity(ordered.len());
        for part in &mut ordered {
            let relative = AXES.map(|axis| part.center[axis] - anchor.center[axis]);
            let original_axis = AXES
                .iter()
                .copied()
                .find(|&axis| part.motion[axis] != 0.)
                .unwrap();
            let rail = long.filter(|&axis| {
                part.overlaps_across(anchor, axis, 0.4)
                    && relative[axis].abs() < anchor.size[axis] * 0.55
                    && relative[axis].abs() + part.size[axis] * 0.25 > EPSILON
            });
            let preferred = rail.unwrap_or(original_axis);
            let direction = rail.map_or(part.motion[original_axis].signum(), |axis| {
                if relative[axis] < 0. {
                    -1.
                } else {
                    1.
                }
            });
            let mut best = (f64::INFINITY, part.motion);
            for axis in AXES {
                for sign in [1., -1.] {
                    let mut distance = GAP;
                    if part.overlaps_across(anchor, axis, 0.) {
                        distance += part.distance_past(anchor, axis, sign).max(0.);
                    }
                    intervals.clear();
                    for &other in &placed {
                        if !AXES.iter().copied().all(|i| {
                            i == axis
                                || (part.bounds[1][i]
                                    > other.bounds[0][i] + other.motion[i] + EPSILON
                                    && part.bounds[0][i]
                                        < other.bounds[1][i] + other.motion[i] - EPSILON)
                        }) {
                            continue;
                        }
                        let low = sign * other.motion[axis]
                            - other.distance_past(*part, axis, sign)
                            - GAP;
                        let high =
                            sign * other.motion[axis] + part.distance_past(other, axis, sign) + GAP;
                        if high > distance && high > low {
                            intervals.push([low, high]);
                        }
                    }
                    intervals.sort_by(|a, b| a[0].total_cmp(&b[0]));
                    for &[low, high] in &intervals {
                        if low >= distance {
                            break;
                        }
                        distance = distance.max(high);
                    }
                    let mut motion = [0.; 3];
                    motion[axis] = sign * distance;
                    let crossings = placed
                        .iter()
                        .filter(|&&other| crosses_during_move(*part, other, motion))
                        .count();
                    let axis_cost = if axis == preferred {
                        0.
                    } else if rail.is_some() {
                        1.1
                    } else if thin == Some(preferred) {
                        0.38
                    } else {
                        0.09
                    };
                    let side_cost = if axis == preferred && sign != direction {
                        if thin == Some(preferred) {
                            0.55
                        } else {
                            0.14
                        }
                    } else {
                        0.
                    };
                    let score = distance + axis_cost + side_cost + crossings as f64 * 3.;
                    if score < best.0 {
                        best = (score, motion);
                    }
                }
            }
            part.motion = best.1;
            placed.push(*part);
        }
    }
    for part in ordered {
        offsets[part.id] = part.motion.map(|distance| (distance / scale) as f32);
    }
    offsets
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rail_carriage_moves_along_the_rail() {
        let rail = Some([[-0.03, -1., -0.02], [0.03, 1., 0.02]]);
        let carriage = Some([[-0.07, -0.13, -0.02], [0.07, 0.09, 0.04]]);
        let moves = plan(&[rail, carriage], [0.; 3], 2.);
        assert_eq!(moves[0], [0.; 3]);
        assert!(moves[1][1] < -1.09);
        assert_eq!([moves[1][0], moves[1][2]], [0.; 2]);
    }

    #[test]
    fn stack_clears_a_fixed_base_and_preserves_its_order() {
        let base = Some([[-0.5, -0.5, -0.1], [0.5, 0.5, 0.1]]);
        let inner = Some([[-0.125, -0.125, 0.03], [0.125, 0.125, 0.13]]);
        let outer = Some([[-0.125, -0.125, 0.10], [0.125, 0.125, 0.20]]);
        let moves = plan(&[base, inner, outer], [0.; 3], 1.45);
        assert_eq!(moves[0], [0.; 3]);
        assert!(0.03 + moves[1][2] > 0.10);
        assert!(0.10 + moves[2][2] > 0.13 + moves[1][2]);
    }
}
