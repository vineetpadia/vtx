override CLIPPING: bool = false;
override PROJECTION: u32 = 0u;
struct Camera {
    framing: vec4<f32>,
    rotation: vec4<f32>,
    view: vec4<f32>,
    viewport: vec4<f32>,
    clip_plane: vec4<f32>,
}

struct Material {
    base: vec4<f32>,
    flags: vec4<f32>,
    uv_transform: vec4<f32>,
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
    @location(3) @interpolate(flat) line_length: f32,
}

fn linear(srgb: vec3<f32>) -> vec3<f32> {
    return select(srgb / 12.92, pow((srgb + 0.055) / 1.055, vec3(2.4)), srgb > vec3(0.04045));
}

fn project(world: vec3<f32>) -> VertexOut {
    var p = (world - camera.framing.xyz) * camera.framing.w;
    let r = camera.rotation;
    p += 2.0 * cross(r.xyz, cross(r.xyz, p) + r.w * p);
    let d = 4.0 - p.z;
    // WebGPU uses a 0..1 depth range after the perspective divide.
    let near = 0.05;
    let far = 100.0;
    var out: VertexOut;
    out.clip = vec4(p.x * 1.85 / camera.view.x, p.y * 1.85,
        far * (d - near) / (far - near) * 0.25, d * 0.25);
    if PROJECTION==1u || (PROJECTION==2u && camera.viewport.z>0.5) {out.clip=vec4(p.x*1.85/camera.view.x,p.y*1.85,d/far,1.0);}
    out.eye = p;
    return out;
}

fn base_material_color() -> vec4<f32> {
    if material.flags.x < 0.5 {
        return vec4(linear(material.base.rgb), material.base.a);
    }
    return material.base;
}

fn mesh_line_vertex(a:vec3<f32>, b:vec3<f32>, ink:vec4<f32>, i:u32) -> VertexOut {
    let first=project(a);let last=project(b);
    let size=max(camera.viewport.xy,vec2(1.0));
    let direction=(last.clip.xy/last.clip.w-first.clip.xy/first.clip.w)*size*0.5;
    let line_length=max(length(direction),0.0001);
    let tangent=direction/line_length;
    let normal=vec2(-tangent.y,tangent.x);
    let corner=array<vec2<f32>,6>(vec2(0.0,-1.0),vec2(1.0,-1.0),vec2(0.0,1.0),vec2(0.0,1.0),vec2(1.0,-1.0),vec2(1.0,1.0))[i];
    var out=first;
    out.clip=mix(first.clip,last.clip,corner.x);
    out.eye=mix(first.eye,last.eye,corner.x);
    let cap=corner.x*2.0-1.0;
    out.clip=vec4(out.clip.xy+(normal*corner.y+tangent*cap)*2.5/size*out.clip.w,out.clip.zw);
    var color=ink.rgb;
    if ink.a<0.5 {color=select(vec3(0.88,0.93,0.96),vec3(0.08,0.12,0.16),camera.view.z>0.5);}
    else if camera.view.z>0.5 && dot(color,vec3(0.299,0.587,0.114))>0.65 {color*=0.55;}
    out.color=vec4(linear(color),1.0);
    out.uv=vec2(corner.x*line_length+cap*1.25,corner.y*1.25);
    out.line_length=line_length;return out;
}
@fragment fn line_surface(input:VertexOut)->@location(0) vec4<f32>{
    clip_surface(input.eye);
    let cap=max(max(-input.uv.x,input.uv.x-input.line_length),0.0);
    let distance=length(vec2(cap,input.uv.y));
    let alpha=1.0-smoothstep(0.45,1.25,distance);
    return vec4(input.color.rgb,alpha);
}

fn mesh_vertex(
    position: vec3<f32>,
    transform_x: vec4<f32>,
    transform_y: vec4<f32>,
    transform_z: vec4<f32>,
    i: u32,
) -> VertexOut {
    let local = vec4(position, 1.0);
    let world = vec3(dot(transform_x, local), dot(transform_y, local), dot(transform_z, local));
    var out = project(world);
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

fn mesh_obj_vertex(corner: vec2<u32>) -> VertexOut {
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

fn shaded(input: VertexOut) -> vec4<f32> {
    clip_surface(input.eye);
    let n = normalize(cross(dpdx(input.eye), dpdy(input.eye)));
    let classic = 0.35 + 0.65 * abs(dot(n, normalize(vec3(0.5, 0.7, 1.0))));
    let facing = select(-n, n, n.z >= 0.0);
    let key = max(dot(facing, normalize(vec3(-0.45, 0.7, 1.0))), 0.0);
    let fill = max(dot(facing, normalize(vec3(0.8, -0.25, 0.45))), 0.0);
    let cad = 0.20 + 0.68 * key + 0.12 * fill;
    let light = select(classic, cad, camera.view.y > 0.5);
    var color = input.color;
    if material.flags.z > 0.5 {
        var sampled = textureSample(base_color, base_sampler, input.uv);
        if material.flags.z > 2.5 {
            // A decal replaces the surface color only inside its rectangle.
            // PNG alpha reveals the original material without a second mesh.
            let inside = all(input.uv >= vec2(0.0)) && all(input.uv <= vec2(1.0));
            color = vec4(mix(color.rgb, sampled.rgb, select(0.0, sampled.a, inside)), color.a);
        } else {
        if material.flags.z > 1.5 {
            sampled = vec4(linear(sampled.rgb), sampled.a);
        }
        color *= sampled;
        }
    }
    if color.a < material.flags.w { discard; }
    return vec4(color.rgb * select(light, 1.0, material.flags.x > 1.5), color.a);
}

@fragment fn surface(input: VertexOut) -> @location(0) vec4<f32> {
    return vec4(shaded(input).rgb, 1.0);
}

struct TransparentOut {
    @location(0) accumulation: vec4<f32>,
    @location(1) revealage: vec4<f32>,
}

@fragment fn transparent_surface(input: VertexOut) -> TransparentOut {
    let color = shaded(input);
    let alpha = clamp(color.a, 0.0, 1.0);
    // Weighted blended OIT: all layers contribute without triangle sorting.
    // Opaque depth still hides surfaces behind solid geometry.
    let weight = max(0.01, alpha * 8.0 + 0.01);
    var out: TransparentOut;
    out.accumulation = vec4(color.rgb * alpha, alpha) * weight;
    out.revealage = vec4(alpha);
    return out;
}

@fragment fn point(input: VertexOut) -> @location(0) vec4<f32> {
    clip_surface(input.eye);
    return vec4(input.color.rgb, 1.0);
}

@fragment fn geometry_normal(input: VertexOut) -> @location(0) vec4<f32> {
    if shaded(input).a < 0.001 { discard; }
    let n = normalize(cross(dpdx(input.eye), dpdy(input.eye)));
    return vec4(n, input.eye.z);
}

fn world_point(eye:vec3<f32>)->vec3<f32>{
    let p=eye/camera.framing.w;let q=vec4(-camera.rotation.xyz,camera.rotation.w);
    return p+2.0*cross(q.xyz,cross(q.xyz,p)+q.w*p)+camera.framing.xyz;
}
fn clip_surface(eye:vec3<f32>){
    if CLIPPING && dot(camera.clip_plane.xyz,camera.clip_plane.xyz)>0.5 && dot(camera.clip_plane.xyz,world_point(eye))>camera.clip_plane.w {discard;}
}
@vertex fn vertex(@location(0) p:vec3<f32>,@location(1) x:vec4<f32>,@location(2) y:vec4<f32>,@location(3) z:vec4<f32>,@builtin(vertex_index) i:u32)->VertexOut{return mesh_vertex(p,x,y,z,i);}
@vertex fn obj_vertex(@location(0) c:vec2<u32>)->VertexOut{return mesh_obj_vertex(c);}
@vertex fn line_vertex(@location(0) a:vec3<f32>,@location(4) b:vec3<f32>,@location(5) ink:vec4<f32>,@builtin(vertex_index) i:u32)->VertexOut{return mesh_line_vertex(a,b,ink,i);}
