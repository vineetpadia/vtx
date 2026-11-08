struct Screen {size:vec4<f32>}
@group(0) @binding(0) var<uniform> screen:Screen;
@group(1) @binding(0) var image:texture_2d<f32>;
@group(1) @binding(1) var image_sampler:sampler;
struct Out {@builtin(position) position:vec4<f32>,@location(0) uv:vec2<f32>,@location(1) color:vec4<f32>}
@vertex fn vertex(@location(0) p:vec2<f32>,@location(1) uv:vec2<f32>,@location(2) color:vec4<f32>)->Out{
 var o:Out;o.position=vec4(2.0*p.x/screen.size.x-1.0,1.0-2.0*p.y/screen.size.y,0.0,1.0);o.uv=uv;o.color=color;return o;
}
@fragment fn fragment(o:Out)->@location(0) vec4<f32>{
 let c=o.color*textureSample(image,image_sampler,o.uv);
 let rgb=select(c.rgb/12.92,pow((c.rgb+vec3(0.055))/1.055,vec3(2.4)),c.rgb>vec3(0.04045));
 return vec4(select(c.rgb,rgb,screen.size.z>0.5),c.a);
}
