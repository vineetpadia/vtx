struct Camera {
    framing: vec4<f32>,
    rotation: vec4<f32>,
    view: vec4<f32>,
}
@group(0) @binding(0) var scene: texture_2d<f32>;
@group(0) @binding(1) var geometry: texture_2d<f32>;
@group(0) @binding(2) var<uniform> camera: Camera;
@group(0) @binding(3) var interior_geometry: texture_2d<f32>;

@vertex fn vertex(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let x = f32((index << 1u) & 2u);
    let y = f32(index & 2u);
    return vec4(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
}

fn linear(srgb: vec3<f32>) -> vec3<f32> {
    return select(srgb / 12.92, pow((srgb + 0.055) / 1.055, vec3(2.4)), srgb > vec3(0.04045));
}

fn normal_at(at: vec2<i32>, interior: bool) -> vec4<f32> {
    let limit = vec2<i32>(textureDimensions(geometry)) - vec2(1);
    if interior { return textureLoad(interior_geometry, clamp(at, vec2(0), limit), 0); }
    return textureLoad(geometry, clamp(at, vec2(0), limit), 0);
}

fn valid(geometry: vec4<f32>) -> bool {
    return dot(geometry.xyz, geometry.xyz) > 0.5;
}

fn edge_at(at: vec2<i32>, interior: bool) -> f32 {
    let center = normal_at(at, interior);
    let left = normal_at(at + vec2(-1, 0), interior);
    let right = normal_at(at + vec2(1, 0), interior);
    let up = normal_at(at + vec2(0, -1), interior);
    let down = normal_at(at + vec2(0, 1), interior);
    if valid(center) {
        var edge = 0.0;
        let neighbors = array<vec4<f32>, 4>(left, right, up, down);
        for (var i = 0; i < 4; i++) {
            let neighbor = neighbors[i];
            if !valid(neighbor) {
                edge = 1.0;
            } else {
                // Only substantial changes in surface direction become edges.
                // The triangulation of a planar face stays invisible.
                let crease = 1.0 - abs(dot(center.xyz, neighbor.xyz));
                edge = max(edge, smoothstep(0.12, 0.24, crease));
            }
        }
        if valid(left) && valid(right) && valid(up) && valid(down) {
            // A depth curvature test catches occlusion boundaries while leaving
            // the constant depth slope of a planar face alone.
            let curvature = max(abs(left.a + right.a - 2.0 * center.a), abs(up.a + down.a - 2.0 * center.a));
            edge = max(edge, smoothstep(0.006, 0.018, curvature));
        }
        return edge;
    }
    return 0.0;
}

@fragment fn composite(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let at = vec2<i32>(position.xy);
    let background = select(vec3(0.003, 0.006, 0.012), vec3(1.0), camera.view.z > 0.5);
    let surface = textureLoad(scene, at, 0);
    var color = surface.rgb + background * (1.0 - surface.a);
    if camera.view.w > 0.5 {
        let front = normal_at(at, false);
        let interior = normal_at(at, true);
        let beneath_cover = valid(front) && valid(interior) && abs(front.a-interior.a)>0.001;
        let edge = max(edge_at(at, false), edge_at(at, true)*select(1.0, 0.65, beneath_cover));
        let ink = select(linear(vec3(0.24, 0.31, 0.39)), linear(vec3(0.10, 0.14, 0.19)), camera.view.z > 0.5);
        color = mix(color, min(color, ink), edge * 0.82);
    }
    return vec4(color, 1.0);
}

fn color_at(at: vec2<i32>) -> vec3<f32> {
    let limit = vec2<i32>(textureDimensions(scene)) - vec2(1);
    return textureLoad(scene, clamp(at, vec2(0), limit), 0).rgb;
}

fn filtered(at: vec2<f32>) -> vec3<f32> {
    let base = vec2<i32>(floor(at));
    let f = fract(at);
    return mix(mix(color_at(base), color_at(base + vec2(1,0)), f.x),
        mix(color_at(base + vec2(0,1)), color_at(base + vec2(1,1)), f.x), f.y);
}

fn luma(color: vec3<f32>) -> f32 { return dot(color, vec3(0.299, 0.587, 0.114)); }

@fragment fn antialias(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let at = vec2<i32>(position.xy);
    let center = color_at(at);
    let nw = luma(color_at(at + vec2(-1,-1)));
    let ne = luma(color_at(at + vec2(1,-1)));
    let sw = luma(color_at(at + vec2(-1,1)));
    let se = luma(color_at(at + vec2(1,1)));
    let lo = min(luma(center), min(min(nw,ne),min(sw,se)));
    let hi = max(luma(center), max(max(nw,ne),max(sw,se)));
    if hi - lo < max(0.02, hi * 0.10) { return vec4(center, 1.0); }
    var direction = vec2(-((nw+ne)-(sw+se)), (nw+sw)-(ne+se));
    let reduce = max((nw+ne+sw+se)*0.03125, 0.0078125);
    direction = clamp(direction/(min(abs(direction.x),abs(direction.y))+reduce), vec2(-4.0), vec2(4.0));
    let point = vec2<f32>(at);
    let a = (filtered(point-direction/6.0)+filtered(point+direction/6.0))*0.5;
    let b = a*0.5 + (filtered(point-direction*0.5)+filtered(point+direction*0.5))*0.25;
    return vec4(select(b, a, luma(b)<lo || luma(b)>hi), 1.0);
}
