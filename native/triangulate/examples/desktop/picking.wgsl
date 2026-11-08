struct Pick {settings:vec4<f32>}
@group(2) @binding(0) var<uniform> pick:Pick;
fn remap(input:VertexOut)->VertexOut {var o=input;o.clip=vec4((o.clip.xy-pick.settings.xy*o.clip.w)*pick.settings.zw,o.clip.zw);return o;}
@vertex fn pick_vertex(@location(0) p:vec3<f32>,@location(1) x:vec4<f32>,@location(2) y:vec4<f32>,@location(3) z:vec4<f32>,@builtin(vertex_index) i:u32)->VertexOut{return remap(mesh_vertex(p,x,y,z,i));}
@vertex fn pick_obj(@location(0) c:vec2<u32>)->VertexOut{return remap(mesh_obj_vertex(c));}
@vertex fn pick_line(@location(0) a:vec3<f32>,@location(4) b:vec3<f32>,@location(5) ink:vec4<f32>,@builtin(vertex_index) i:u32)->VertexOut{return remap(mesh_line_vertex(a,b,ink,i));}
@fragment fn pick_surface(input:VertexOut)->@location(0) vec4<f32>{if shaded(input).a<0.001 {discard;}return vec4(world_point(input.eye),1.0);}
@fragment fn pick_point(input:VertexOut)->@location(0) vec4<f32>{clip_surface(input.eye);return vec4(world_point(input.eye),1.0);}
@fragment fn pick_stroke(input:VertexOut)->@location(0) vec4<f32>{clip_surface(input.eye);let cap=max(max(-input.uv.x,input.uv.x-input.line_length),0.0);if length(vec2(cap,input.uv.y))>1.0 {discard;}return vec4(world_point(input.eye),1.0);}
