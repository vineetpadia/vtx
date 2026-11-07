struct Camera {
    framing: vec4<f32>,
    rotation: vec4<f32>,
    view: vec4<f32>,
    section: vec4<f32>,
}

struct Material {
    base: vec4<f32>,
    flags: vec4<f32>,
    uv_transform: vec4<f32>,
    metadata: vec4<f32>,
}

@group(0) @binding(0) var<uniform> camera: Camera;
@group(1) @binding(0) var<uniform> material: Material;
@group(1) @binding(1) var<storage, read> colors: array<u32>;
@group(1) @binding(2) var<storage, read> texcoords: array<vec2<f32>>;
@group(1) @binding(3) var base_color: texture_2d<f32>;
@group(1) @binding(4) var base_sampler: sampler;

struct VertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) eye: vec3<f32>,
    @location(1) color: vec4<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) world: vec3<f32>,
    @location(4) @interpolate(flat) instance: u32,
}

fn linear(srgb: vec3<f32>) -> vec3<f32> {
    return select(srgb / 12.92, pow((srgb + 0.055) / 1.055, vec3(2.4)), srgb > vec3(0.04045));
}

fn project(world: vec3<f32>) -> VertexOut {
    var p = (world - camera.framing.xyz) * camera.framing.w;
    let r = camera.rotation;
    p = vec3(r.x * p.x + r.y * p.z, p.y, -r.y * p.x + r.x * p.z);
    p = vec3(p.x, r.z * p.y - r.w * p.z, r.w * p.y + r.z * p.z);
    let d = 4.0 - p.z;
    // WebGPU uses a 0..1 depth range after the perspective divide.
    let near = 0.05;
    let far = 100.0;
    var out: VertexOut;
    out.clip = vec4(p.x * 1.85 / camera.view.x, p.y * 1.85,
        far * (d - near) / (far - near) * 0.25, d * 0.25);
    if camera.view.y > 0.5 {
        out.clip = vec4(p.x * 1.85 / camera.view.x, p.y * 1.85, 0.5 - p.z * 0.1 * camera.view.z, 1.0);
    }
    out.eye = p;
    out.world = world;
    out.instance = 0u;
    return out;
}

fn base_material_color() -> vec4<f32> {
    if material.flags.x < 0.5 {
        return vec4(linear(material.base.rgb), material.base.a);
    }
    return material.base;
}

@vertex fn vertex(
    @location(0) position: vec3<f32>,
    @location(1) transform_x: vec4<f32>,
    @location(2) transform_y: vec4<f32>,
    @location(3) transform_z: vec4<f32>,
    @builtin(vertex_index) i: u32,
    @builtin(instance_index) instance: u32,
) -> VertexOut {
    let local = vec4(position, 1.0);
    let world = vec3(dot(transform_x, local), dot(transform_y, local), dot(transform_z, local));
    var out = project(world);
    out.instance = instance;
    out.color = base_material_color();
    if material.flags.y > 0.5 {
        let rgba = unpack4x8unorm(colors[i]);
        out.color = vec4(linear(rgba.rgb), rgba.a);
    }
    out.uv = vec2(0.0);
    if material.flags.z > 0.5 {
        out.uv = texcoords[i];
    }
    return out;
}

@vertex fn obj_vertex(@location(0) corner: vec2<u32>) -> VertexOut {
    // The OBJ pipeline binds packed XYZ positions in the ordinary vertex color slot.
    let i = corner.x * 3u;
    let world = bitcast<vec3<f32>>(vec3(colors[i], colors[i + 1u], colors[i + 2u]));
    var out = project(world);
    out.color = base_material_color();
    if material.flags.z > 0.5 {
        let uv = texcoords[corner.y] * material.uv_transform.xy + material.uv_transform.zw;
        out.uv = vec2(uv.x, 1.0 - uv.y);
    }
    return out;
}

fn clip_section(world: vec3<f32>) {
    if camera.view.w > 0.5 && dot(camera.section.xyz, world) + camera.section.w > 0.0 { discard; }
}

@fragment fn surface(input: VertexOut) -> @location(0) vec4<f32> {
    clip_section(input.world);
    let n = normalize(cross(dpdx(input.eye), dpdy(input.eye)));
    let light = 0.35 + 0.65 * abs(dot(n, normalize(vec3(0.5, 0.7, 1.0))));
    var color = input.color;
    if material.flags.z > 0.5 {
        var sampled = textureSample(base_color, base_sampler, input.uv);
        if material.flags.z > 1.5 {
            sampled = vec4(linear(sampled.rgb), sampled.a);
        }
        color *= sampled;
    }
    if color.a < material.flags.w { discard; }
    if camera.view.w > 0.5 {
        let distance = dot(camera.section.xyz, input.world) + camera.section.w;
        if abs(distance) < max(fwidth(distance), 1e-7) * 1.5 { return vec4(1.0, 0.55, 0.1, 1.0); }
    }
    return vec4(color.rgb * light, 1.0);
}

@fragment fn point(input: VertexOut) -> @location(0) vec4<f32> {
    clip_section(input.world);
    return vec4(input.color.rgb, 1.0);
}

struct QueryOut {
    @location(0) position_part: vec4<f32>,
    @location(1) normal_instance: vec4<f32>,
}
@fragment fn query_surface(input: VertexOut) -> QueryOut {
    clip_section(input.world);
    // Match visible material alpha tests in the image pass.
    var alpha = input.color.a;
    if material.flags.z > 0.5 { alpha *= textureSample(base_color, base_sampler, input.uv).a; }
    if alpha < material.flags.w {discard;}
    var out: QueryOut;
    out.position_part = vec4(input.world, material.metadata.x);
    out.normal_instance = vec4(-normalize(cross(dpdx(input.world), dpdy(input.world))), f32(input.instance) + 1.0);
    return out;
}
@fragment fn query_point(input: VertexOut) -> QueryOut {
    clip_section(input.world);
    var out: QueryOut;
    out.position_part = vec4(input.world, material.metadata.x);
    out.normal_instance = vec4(0.0, 0.0, 0.0, f32(input.instance) + 1.0);
    return out;
}
