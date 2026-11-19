#include "lighting.hlsli"
#include "atmosphere.hlsli"
#include "clouds.hlsli"
#include "gbuffer.hlsli"

Texture2D<float4> cloudLayer : register(t0);
float4 filteredClouds(float2 uv, float2 size) {
    float2 inset = 0.5 / ceil(size / 4);
    return cloudLayer.SampleLevel(cloudSampler, clamp(uv, inset, 1 - inset), 0);
}

Texture2D<float4> albedoMetal : register(t9);
Texture2D<float4> normalRoughMaterial : register(t10);
Texture2D<float> cameraDepth : register(t11);

float4 deferred_ps(float4 position : SV_Position) : SV_Target {
    float2 size = float2(terrainParams.w, terrainParams.y);
    float2 uv = position.xy / size;
    float3 ray = cameraRay(uv);
    int3 pixel = int3(position.xy, 0);
    float4 surface = normalRoughMaterial.Load(pixel);
    float rayLength = length(ray);
    float3 direction = ray / rayLength;
    float3 atmosphereEye = float3(camera.x, max(camera.y, 0), camera.z) * 0.001;
    if (surface.w == 0) {
        AtmosphereSample sky = sampleAtmosphere(atmosphereEye, direction, 1e8, 8);
        float4 cloud = filteredClouds(uv, size);
        // The procedural scene covers only a small part of the planet.
        if (sky.hitsGround) {
            float3 ground = sky.radiance + sky.transmittance * float3(0.16, 0.20, 0.09) * daylightFill();
            return float4(cloud.rgb + cloud.a * ground, 1);
        }

        // The Sun spans about 0.53 degrees. Filter its edge over roughly one pixel.
        const float sunRadius = 2.0 * sin(radians(0.265) * 0.5);
        float edge = 0.5 / (terrainParams.z * rayLength);
        float separation = length(direction - sun.xyz);
        float disk = 1.0 - smoothstep(sunRadius - edge, sunRadius + edge, separation);
        float3 sunlight = sky.transmittance * (4.0 * sun.w * disk);
        return float4(cloud.rgb + cloud.a * (sky.radiance + sunlight), 1);
    }

    float viewDepth = depthProjection.y / (cameraDepth.Load(pixel) + depthProjection.x);
    float3 view = ray * viewDepth;
    float3 world = camera.xyz + view;
    float4 albedo = albedoMetal.Load(pixel);
    bool matteGround = surface.w == MATERIAL_MATTE_GROUND;
    float3 color = shadeSurface(world, unpackNormal(surface.xy), albedo.rgb, albedo.a, surface.z, matteGround);
    float distance = rayLength * viewDepth;
    AtmosphereSample aerial = sampleAtmosphere(atmosphereEye, direction, distance * 0.001, 3);
    color = aerial.radiance + aerial.transmittance * color;
    float3 eye = camera.xyz * 0.001;
    if (max(radialOffsetSquared(eye), radialOffsetSquared(world * 0.001))
            >= CLOUD_BASE * (2 * EARTH_RADIUS + CLOUD_BASE)) {
        float2 span = cloudInterval(eye, direction, CLOUD_RANGE);
        float limit = distance * 0.001;
        if (min(span.y, limit) > span.x) {
            float4 cloud = limit >= span.y ? filteredClouds(uv, size)
                : marchClouds(eye, direction, float2(span.x, limit));
            color = cloud.rgb + cloud.a * color;
        }
    }
    return float4(color, 1);
}
