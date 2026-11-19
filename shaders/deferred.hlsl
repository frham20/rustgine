#include "lighting.hlsli"
#include "atmosphere.hlsli"
#include "gbuffer.hlsli"

Texture2D<float4> albedoMetal : register(t9);
Texture2D<float4> normalRoughMaterial : register(t10);
Texture2D<float> cameraDepth : register(t11);

float4 deferred_ps(float4 position : SV_Position) : SV_Target {
    float2 size = float2(terrainParams.w, terrainParams.y);
    float2 screen = (2 * position.xy / size - 1) * float2(1, -1);
    float3 right = viewProjection[0].xyz;
    float3 up = viewProjection[1].xyz;
    float3 ray = viewProjection[3].xyz + screen.x * right / dot(right, right)
                                      + screen.y * up / dot(up, up);
    int3 pixel = int3(position.xy, 0);
    float4 surface = normalRoughMaterial.Load(pixel);
    float rayLength = length(ray);
    float3 direction = ray / rayLength;
    float3 atmosphereEye = float3(camera.x, max(camera.y, 0), camera.z) * 0.001;
    if (surface.w == 0) {
        AtmosphereSample sky = sampleAtmosphere(atmosphereEye, direction, 1e8, 8);
        // The procedural scene covers only a small part of the planet.
        if (sky.hitsGround) {
            return float4(sky.radiance + sky.transmittance * float3(0.16, 0.20, 0.09) * daylightFill(), 1);
        }

        // The Sun spans about 0.53 degrees. Filter its edge over roughly one pixel.
        const float sunRadius = 2.0 * sin(radians(0.265) * 0.5);
        float edge = 0.5 / (terrainParams.z * rayLength);
        float separation = length(direction - sun.xyz);
        float disk = 1.0 - smoothstep(sunRadius - edge, sunRadius + edge, separation);
        float3 sunlight = sky.transmittance * (4.0 * sun.w * disk);
        return float4(sky.radiance + sunlight, 1);
    }

    float viewDepth = depthProjection.y / (cameraDepth.Load(pixel) + depthProjection.x);
    float3 view = ray * viewDepth;
    float3 world = camera.xyz + view;
    float4 albedo = albedoMetal.Load(pixel);
    bool matteGround = surface.w == MATERIAL_MATTE_GROUND;
    float3 color = shadeSurface(world, unpackNormal(surface.xy), albedo.rgb, albedo.a, surface.z, matteGround);
    float distance = rayLength * viewDepth;
    AtmosphereSample aerial = sampleAtmosphere(atmosphereEye, direction, distance * 0.001, 3);
    return float4(aerial.radiance + aerial.transmittance * color, 1);
}
