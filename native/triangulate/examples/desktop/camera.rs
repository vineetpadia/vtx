use super::model::Scene;
use nalgebra_glm as glm;

#[derive(Clone)]
pub struct Camera {
    pub rotation: glm::Quat,
    pub zoom: f32,
    pub edges: bool,
    pub light_background: bool,
    pub cad_lighting: bool,
    pub pan: [f32; 3],
    pub clip: [f32; 4],
    pub orthographic: bool,
}
fn initial_rotation() -> glm::Quat {
    glm::quat_angle_axis(0.4, &glm::vec3(1., 0., 0.))
        * glm::quat_angle_axis(-0.6, &glm::vec3(0., 1., 0.))
}
impl Default for Camera {
    fn default() -> Self {
        let classic = std::env::var("VTX_VIEW_STYLE").is_ok_and(|s| s == "classic");
        Self {
            rotation: initial_rotation(),
            zoom: 1.,
            edges: !classic,
            light_background: !classic,
            cad_lighting: !classic,
            pan: [0.; 3],
            clip: [0.; 4],
            orthographic: false,
        }
    }
}
impl Camera {
    pub fn project(&self, point: [f32; 3], scene: &Scene, size: [u32; 2]) -> Option<[f32; 2]> {
        let aspect = size[0] as f32 / size[1].max(1) as f32;
        let scale = 0.9 * aspect.min(1.) * self.zoom / scene.diameter;
        let p = glm::vec3(
            point[0] - scene.center[0] - self.pan[0],
            point[1] - scene.center[1] - self.pan[1],
            point[2] - scene.center[2] - self.pan[2],
        ) * scale;
        let p = glm::quat_rotate_vec3(&self.rotation, &p);
        let w = if self.orthographic {
            1.
        } else {
            (4. - p.z) * 0.25
        };
        if w <= 0.01 {
            return None;
        }
        Some([
            (p.x * 1.85 / aspect / w + 1.) * size[0] as f32 * 0.5,
            (1. - p.y * 1.85 / w) * size[1] as f32 * 0.5,
        ])
    }
    fn to_world(&self, v: glm::Vec3) -> glm::Vec3 {
        glm::quat_rotate_vec3(&glm::quat_conjugate(&self.rotation), &v)
    }
    fn per_pixel(&self, diameter: f32, size: [u32; 2]) -> f32 {
        let aspect = size[0].max(1) as f32 / size[1].max(1) as f32;
        diameter / (0.9 * aspect.min(1.) * self.zoom) * 2. / (1.85 * size[1].max(1) as f32)
    }
    pub fn pan_by(&mut self, delta: [f32; 2], diameter: f32, size: [u32; 2]) {
        let unit = self.per_pixel(diameter, size);
        let offset = self.to_world(glm::vec3(-delta[0] * unit, delta[1] * unit, 0.));
        for i in 0..3 {
            self.pan[i] += offset[i];
        }
    }
    pub fn orbit(&mut self, delta: [f32; 2], speed: f32) {
        let axis = glm::vec3(delta[1], delta[0], 0.);
        let distance = axis.norm();
        if distance > 0. {
            self.rotate(glm::quat_angle_axis(
                distance * 0.008 * speed,
                &(axis / distance),
            ));
        }
    }
    pub fn roll(&mut self, delta: f32, speed: f32) {
        self.rotate(glm::quat_angle_axis(
            -delta * 0.008 * speed,
            &glm::vec3(0., 0., 1.),
        ));
    }
    fn rotate(&mut self, delta: glm::Quat) {
        // Keep the model's projected center stationary after panning. Rotation
        // is about the model, rather than making it orbit the screen center.
        let screen_pan = glm::quat_rotate_vec3(
            &self.rotation,
            &glm::vec3(self.pan[0], self.pan[1], self.pan[2]),
        );
        self.rotation = glm::quat_normalize(&(delta * self.rotation));
        self.pan = self.to_world(screen_pan).into();
    }
    pub fn zoom_at(
        &mut self,
        exponent: f32,
        anchor: Option<[f32; 2]>,
        diameter: f32,
        size: [u32; 2],
    ) {
        let before = self.per_pixel(diameter, size);
        self.zoom = (self.zoom * exponent.clamp(-10., 10.).exp()).clamp(0.05, 30.);
        if let Some([x, y]) = anchor {
            let change = before - self.per_pixel(diameter, size);
            let offset = self.to_world(glm::vec3(
                (x - size[0] as f32 * 0.5) * change,
                (size[1] as f32 * 0.5 - y) * change,
                0.,
            ));
            for i in 0..3 {
                self.pan[i] += offset[i];
            }
        }
    }
    pub fn fit(&mut self, scene: &Scene, aspect: f32) {
        self.rotation = if scene.plan_view {
            glm::quat_identity()
        } else {
            initial_rotation()
        };
        self.frame(scene, aspect);
    }
    pub fn frame(&mut self, scene: &Scene, aspect: f32) {
        self.pan = [0.; 3];
        self.zoom = 1.;
        if !self.cad_lighting && !scene.plan_view {
            return;
        }
        let mut low = 0.;
        let mut high = 8.;
        for _ in 0..24 {
            let zoom = (low + high) * 0.5;
            let scale = 0.9 * aspect.min(1.) * zoom / scene.diameter;
            let mut fits = true;
            for corner in 0..8 {
                let p: [f32; 3] = [0, 1, 2]
                    .map(|a| (scene.bounds[(corner >> a) & 1][a] - scene.center[a]) * scale);
                let p = glm::quat_rotate_vec3(&self.rotation, &glm::vec3(p[0], p[1], p[2]));
                let w = if self.orthographic {
                    1.
                } else {
                    (4. - p.z) * 0.25
                };
                fits &= w > 0.05
                    && (p.x * 1.85 / aspect / w).abs() < 0.84
                    && (p.y * 1.85 / w).abs() < 0.84;
            }
            if fits {
                low = zoom;
            } else {
                high = zoom;
            }
        }
        self.zoom = low.max(0.05);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pan_follows_screen_axes_after_free_rotation() {
        let mut c = Camera::default();
        c.orbit([270., 500.], 1.);
        c.roll(70., 1.);
        let r = c.rotation;
        c.pan_by([100., 40.], 10., [640, 480]);
        let p = glm::quat_rotate_vec3(&c.rotation, &glm::vec3(c.pan[0], c.pan[1], c.pan[2]));
        assert!(p.x < 0. && p.y > 0.);
        assert!((p.y / p.x + 0.4).abs() < 0.00001);
        assert!(p.z.abs() < 0.00001);
        assert_eq!(c.rotation, r);
    }
    #[test]
    fn free_orbit_passes_both_poles_and_stays_normalized() {
        let mut c = Camera {
            rotation: glm::quat_identity(),
            ..Camera::default()
        };
        c.orbit([0., std::f32::consts::PI / 0.008], 1.);
        let up = glm::quat_rotate_vec3(&c.rotation, &glm::vec3(0., 1., 0.));
        assert!(up.y < -0.999);
        for _ in 0..10000 {
            c.orbit([1., 2.], 1.);
        }
        assert!((c.rotation.norm() - 1.).abs() < 0.00001);
    }
    #[test]
    fn rotating_a_panned_model_keeps_its_screen_center() {
        let mut c = Camera::default();
        c.pan_by([100., 60.], 10., [640, 480]);
        let before = glm::quat_rotate_vec3(&c.rotation, &glm::vec3(c.pan[0], c.pan[1], c.pan[2]));
        c.orbit([200., 300.], 1.);
        let after = glm::quat_rotate_vec3(&c.rotation, &glm::vec3(c.pan[0], c.pan[1], c.pan[2]));
        assert!((before - after).norm() < 0.00001);
    }
    #[test]
    fn zoom_anchor_stays_on_the_same_view_plane_point() {
        let size = [640, 480];
        let anchor = [120., 330.];
        let mut c = Camera::default();
        c.orbit([270., 500.], 1.);
        let anchor_world = |c: &Camera| {
            glm::vec3(c.pan[0], c.pan[1], c.pan[2])
                + c.to_world(glm::vec3(
                    (anchor[0] - 320.) * c.per_pixel(10., size),
                    (240. - anchor[1]) * c.per_pixel(10., size),
                    0.,
                ))
        };
        let before = anchor_world(&c);
        c.zoom_at(0.7, Some(anchor), 10., size);
        assert!((before - anchor_world(&c)).norm() < 0.00001);
        c.zoom_at(-0.7, Some(anchor), 10., size);
        assert!((c.zoom - 1.).abs() < 0.00001);
        assert!(glm::vec3(c.pan[0], c.pan[1], c.pan[2]).norm() < 0.00001);
    }
    #[test]
    fn zoomed_pan_moves_less_world_distance() {
        let mut a = Camera::default();
        let mut b = Camera {
            zoom: 2.,
            ..Camera::default()
        };
        a.pan_by([30., 50.], 10., [300, 500]);
        b.pan_by([30., 50.], 10., [300, 500]);
        for i in 0..3 {
            assert!((a.pan[i] - 2. * b.pan[i]).abs() < 0.00001);
        }
    }
}
