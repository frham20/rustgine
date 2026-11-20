#include "lighting.hlsli"
#include "gbuffer.hlsli"
#include "terrain_materials.hlsli"
#include "lake_reflection.hlsli"
#include "alpine_geology.hlsli"

struct Vertex { float3 p : POSITION; float3 n : NORMAL; };
struct Surface {
    float4 position : SV_Position;
    float3 world : WORLD;
    float3 normal : NORMAL;
    float3 base : COLOR;
    float clipHeight : SV_ClipDistance0;
};

cbuffer AlpineDraw : register(b1) { uint drawFlags; } // Low bits: camera, shadow, mirror. Bit 2: billboard.
StructuredBuffer<float4> trees : register(t2);

Surface projectSurface(float3 world, float3 normal, float3 base) {
    Surface o;
    o.world = world;
    o.normal = normal;
    o.base = base;
    uint pass = drawFlags & 3;
    o.clipHeight = pass == 2 ? world.y - lakeLevel : 1;
    if (pass == 2) world = mirrorPoint(world);
    o.position = pass == 1 ? mul(lightProjection[shadowSlice], float4(world, 1)) : mul(viewProjection, float4(world, 1));
    return o;
}

Surface land_vs(Vertex v) {
    return projectSurface(v.p, v.n, 0);
}

AlpineSurface mountainSurface(Surface input) {
    float3 n = normalize(input.normal);
    float alpine = smoothstep(480, 1000, input.world.y) + 0.7 * (1 - n.y);
    float2 grainUV = input.world.xz * 0.0045;
    float grain = filteredMottling(grainUV, max(length(ddx(grainUV)),length(ddy(grainUV))));
    float3 base = lerp(float3(0.043, 0.083, 0.043), float3(0.24, 0.23, 0.195),
                       smoothstep(0.25, 0.85, alpine + 0.18 * grain));
    return alpineSurface(input.world, n, base * (0.8 + 0.4 * grain), 0.92, smoothstep(0.48, 0.90, alpine),
                         (drawFlags & 3) == 2 ? 0.25 : 1);
}

GBuffer land_ps(Surface input) {
    AlpineSurface s = mountainSurface(input);
    return materialSample(s.color, s.normal, 0, s.roughness, MATERIAL_MATTE_GROUND);
}

Surface pine_vs(Vertex v, uint instance : SV_InstanceID) {
    float4 tree = trees[instance];
    float seed = frac(tree.w * 13.198);
    bool bark = dot(v.n, v.n) > 2;
    bool card = (drawFlags & 4) != 0;
    float2 forward;
    if (card) {
        float2 facing = (drawFlags & 3) == 1 ? sun.xz : camera.xz - tree.xz;
        forward = normalize(facing + float2(0.00001, 0));
    } else {
        float angle = seed * 6.283185;
        forward = float2(sin(angle), cos(angle));
    }
    float2 right = float2(forward.y, -forward.x);
    float width = 0.86 + 0.36 * frac(seed * 73.17);
    if (!card && !bark) width *= 1 + 0.13 * sin(26 * v.p.y + 31 * seed);
    float3 world = tree.xyz;
    world.xz += tree.w * width * (right * v.p.x + forward * v.p.z);
    world.y += tree.w * v.p.y;
    float2 radial = right * v.n.x + forward * v.n.z;
    float3 n = normalize(float3(radial.x, v.n.y, radial.y));
    float3 color = lerp(float3(0.019, 0.050, 0.031), float3(0.064, 0.126, 0.058), seed);
    color *= lerp(0.58, 1.17, saturate(v.p.y));
    if (!card) {
        float tips = saturate(length(v.p.xz) / max(0.018, 0.20 * (1 - v.p.y)));
        color *= lerp(0.69, 1.25, tips);
    }
    if (bark) color = float3(0.079, 0.053, 0.034);
    if (card) {
        bool mirror = (drawFlags & 3) == 2;
        float3 eye = mirror ? mirrorPoint(camera.xyz) : camera.xyz;
        float crownPixels = 0.37 * width * tree.w * terrainParams.z * (mirror ? 0.25 : 1) / max(1, distance(eye,tree.xyz));
        float cover = canopyBlend(crownPixels);
        color = lerp(color, CANOPY_COLOR, cover);
        n = normalize(lerp(n, float3(0,1,0), 0.85 * cover));
    }
    return projectSurface(world, n, color);
}

GBuffer pine_ps(Surface input) {
    return materialSample(input.base, normalize(input.normal), 0, 0.94, MATERIAL_MATTE_GROUND);
}

float4 land_reflection_ps(Surface input) : SV_Target {
    AlpineSurface s = mountainSurface(input);
    return reflectedLand(input.world, s.normal, s.color, s.roughness);
}
float4 pine_reflection_ps(Surface input) : SV_Target { return reflectedLand(input.world, input.normal, input.base, 0.94); }

Surface lake_vs(Vertex v) { return projectSurface(v.p, float3(0,1,0), v.n.x); }

GBuffer lake_ps(Surface input) {
    float2 p = input.world.xz;
    float time = grassParams.x * 0.65;
    float dist = distance(input.world, camera.xyz);
    float detail = 1 - smoothstep(90, 760, dist);
    float4 phase = float4(dot(p,float2(0.064,0.093)), dot(p,float2(-0.055,0.081)),
                          dot(p,float2(0.83,0.22)), dot(p,float2(0.49,-0.63)));
    // Attenuate each wave as its projected period approaches two pixels.
    float4 resolved = 1 - smoothstep(0.8, 3.14, fwidth(phase));
    float2 ripple = resolved.xy * float2(0.014 * sin(phase.x + time), 0.011 * cos(phase.y - time));
    ripple += detail * resolved.zw * float2(0.019 * cos(phase.z + time*2), 0.012 * sin(phase.w - time*2));
    float3 n = normalize(float3(ripple.x, 1, ripple.y));
    float depth = max(input.base.x, 0);
    float3 bed = float3(0.105, 0.37, 0.294);
    float3 deep = float3(0.009, 0.154, 0.205);
    float3 body = lerp(deep, bed, exp(-depth * float3(0.15, 0.072, 0.053)));
    return materialSample(body, n, 0, 0.07, MATERIAL_LAKE);
}
