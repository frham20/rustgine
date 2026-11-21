#include "lighting.hlsli"
#include "lake_reflection.hlsli"
#include "clouds.hlsli"
#include "gbuffer.hlsli"

Texture2D<float4> cloudLayer : register(t0);
Texture2D<float4> lakeReflection : register(t14);
float4 filteredClouds(float2 uv, float2 size) {
    float2 inset = 0.5 / cloudRenderSize(size);
    return cloudLayer.SampleLevel(cloudSampler, clamp(uv, inset, 1 - inset), 0);
}

Texture2D<float4> albedoMetal : register(t9);
Texture2D<float4> normalRoughMaterial : register(t10);
Texture2D<float> cameraDepth : register(t11);

static const float SUN_SHAFT_STRENGTH = 0.070;

float shaftOcclusion(float2 uv, float2 inset) {
    return saturate(normalRoughMaterial.SampleLevel(cloudSampler, clamp(uv, inset, 1 - inset), 0).w);
}

float3 sunShafts(float2 uv, float2 size, float3 direction, float3 eye, float airPath) {
    // Keep the GPU work bounded to the ranges in SunShaftQuality.
    uint segments = min(sunShaftRadialSamples, 48u);
    if (segments == 0) return 0;
    uint refinement = min(sunShaftEdgeRefinement, 8u);
    float daylight = smoothstep(0.015, 0.14, sun.y);
    float forward = smoothstep(0.76, 0.995, dot(direction, sun.xyz));
    if (daylight * forward * airPath <= 0) return 0;

    // Project an infinite direction using the same perspective as the solar disk.
    float4 clip = mul(viewProjection, float4(sun.xyz, 0));
    if (clip.w <= 0.05) return 0;
    float2 ndc = clip.xy / clip.w;
    float screenFade = 1 - smoothstep(0.80, 1.0, max(abs(ndc.x), abs(ndc.y)));
    if (screenFade <= 0) return 0;
    float2 sunUv = ndc * float2(0.5, -0.5) + 0.5;

    const float falloff = 0.65;
    float segmentWidth = 1.0 / segments;
    float2 radialSpan = 0.88 * (sunUv - uv);
    float2 inset = 0.5 / size;
    float visible = 0;
    float start = 0;
    float startMask = shaftOcclusion(uv, inset);
    [loop] for (uint i = 1; i <= segments; ++i) {
        float end = i * segmentWidth;
        float endMask = shaftOcclusion(uv + end * radialSpan, inset);
        bool startOpen = startMask < 0.5;
        bool endOpen = endMask < 0.5;
        [branch] if (startOpen || endOpen) {
            float2 open = float2(start, end);
            [branch] if (startOpen != endOpen) {
                // Resolve the skyline within the interval so crossing a tap cannot
                // add or remove an entire interval of light at once.
                float lo = start, hi = end;
                float loMask = startMask, hiMask = endMask;
                [loop] for (uint j = 0; j < refinement; ++j) {
                    float mid = (lo + hi) * 0.5;
                    float mask = shaftOcclusion(uv + mid * radialSpan, inset);
                    if ((mask < 0.5) == startOpen) {
                        lo = mid;
                        loMask = mask;
                    } else {
                        hi = mid;
                        hiMask = mask;
                    }
                }
                float crossing = lerp(lo, hi, (0.5 - loMask) / (hiMask - loMask));
                if (startOpen) open.y = crossing;
                else open.x = crossing;
            }
            float center = (open.x + open.y) * 0.5;
            float weight = (open.y - open.x) * (1 - falloff * center);
            visible += weight * filteredClouds(uv + center * radialSpan, size).a;
        }
        start = end;
        startMask = endMask;
    }
    float transmission = saturate(visible / (1 - 0.5 * falloff));
    float radial = radialOffsetSquared(eye);
    float radius = sqrt(EARTH_RADIUS * EARTH_RADIUS + radial);
    float height = max(0, radial / (radius + EARTH_RADIUS));
    float3 tint = sunTransmittance(radius, dot(eye, sun.xyz) + EARTH_RADIUS * sun.y,
                                  exp(-height / RAYLEIGH_HEIGHT));
    return tint * (SUN_SHAFT_STRENGTH * sun.w * daylight * forward * screenFade * airPath
                   * transmission * transmission);
}

float4 sky_reflection_ps(float4 position : SV_Position) : SV_Target {
    float2 size = ceil(float2(terrainParams.w, terrainParams.y) / 4);
    float3 dir = normalize(cameraRay(position.xy / size));
    dir.y = -dir.y;
    float3 eye = mirrorPoint(camera.xyz);
    // Start at the lake so a mirrored camera below sea level does not intersect Earth's interior.
    if (dir.y > 0.0001) eye += dir * clamp((lakeLevel - eye.y) / dir.y, 0, 20000);
    eye.y = max(eye.y, lakeLevel);
    eye *= 0.001;
    AtmosphereSample sky = sampleAtmosphere(eye, dir, 1e8, 8);
    float4 cloud = marchClouds(eye, dir, cloudInterval(eye, dir, CLOUD_RANGE), 4 / terrainParams.z);
    float sunDisk = smoothstep(0.99996, 0.999994, dot(dir, sun.xyz)) * smoothstep(0, 0.05, sun.y);
    return float4(cloud.rgb + cloud.a * (sky.radiance + sky.transmittance * (4 * sun.w * sunDisk)), 1);
}

float4 deferred_ps(float4 position : SV_Position) : SV_Target {
    float2 size = float2(terrainParams.w, terrainParams.y);
    float2 uv = position.xy / size;
    float3 ray = cameraRay(uv);
    int3 pixel = int3(position.xy, 0);
    float4 surface = normalRoughMaterial.Load(pixel);
    float rayLength = length(ray);
    float3 direction = ray / rayLength;
    float3 atmosphereEye = float3(camera.x, max(camera.y, 0), camera.z) * 0.001;
    float viewDepth = 0;
    if (surface.w != 0) viewDepth = depthProjection.y / (cameraDepth.Load(pixel) + depthProjection.x);
    float distance = rayLength * viewDepth;
    // Only air in front of a surface can scatter light into its pixel.
    float airPath = surface.w == 0 ? 1 : smoothstep(120.0, 4200.0, distance);
    float3 shafts = sunShafts(uv, size, direction, atmosphereEye, airPath);
    if (surface.w == 0) {
        AtmosphereSample sky = sampleAtmosphere(atmosphereEye, direction, 1e8, 8);
        float4 cloud = filteredClouds(uv, size);
        // The procedural scene covers only a small part of the planet.
        if (sky.hitsGround) {
            float3 ground = sky.radiance + sky.transmittance * float3(0.16, 0.20, 0.09) * daylightFill();
            return float4(cloud.rgb + cloud.a * ground + shafts, 1);
        }

        // The Sun spans about 0.53 degrees. Filter its edge over roughly one pixel.
        const float sunRadius = 2.0 * sin(radians(0.265) * 0.5);
        float edge = 0.5 / (terrainParams.z * rayLength);
        float separation = length(direction - sun.xyz);
        float disk = 1.0 - smoothstep(sunRadius - edge, sunRadius + edge, separation);
        float3 sunlight = sky.transmittance * (4.0 * sun.w * disk);
        return float4(cloud.rgb + cloud.a * (sky.radiance + sunlight) + shafts, 1);
    }

    float3 view = ray * viewDepth;
    float3 world = camera.xyz + view;
    float4 albedo = albedoMetal.Load(pixel);
    bool matteGround = surface.w == MATERIAL_MATTE_GROUND;
    float3 n = unpackNormal(surface.xy);
    float3 color;
    if (surface.w == MATERIAL_LAKE) {
        float2 inset = 0.5 / ceil(size / 4);
        // Project the tilted reflection and its flat counterpart into camera space.
        float3 bend = reflect(direction, n) - reflect(direction, float3(0,1,0));
        bend.y = -bend.y;
        float2 distortion = float2(dot(viewProjection[0].xyz, bend), -dot(viewProjection[1].xyz, bend));
        float3 reflected = lakeReflection.SampleLevel(cloudSampler, clamp(uv + 0.062 * distortion, inset, 1-inset), 0).rgb;
        float f = fresnel(abs(dot(-direction, n)), 0.0204.xxx).x;
        float sunlit = sun.y > 0.025 ? visibility(world, n) * cloudVisibility(world) : 0;
        color = lerp(albedo.rgb * daylightFill() * (0.48 + 0.52 * sunlit), reflected, f);
    } else {
        color = shadeSurface(world, n, albedo.rgb, albedo.a, surface.z, matteGround);
    }
    color = aerialColor(camera.xyz, world, color);
    float3 eye = camera.xyz * 0.001;
    if (max(radialOffsetSquared(eye), radialOffsetSquared(world * 0.001))
            >= CLOUD_BASE * (2 * EARTH_RADIUS + CLOUD_BASE)) {
        float2 span = cloudInterval(eye, direction, CLOUD_RANGE);
        float limit = distance * 0.001;
        if (min(span.y, limit) > span.x) {
            float4 cloud = limit >= span.y ? filteredClouds(uv, size)
                : marchClouds(eye, direction, float2(span.x, limit), 1 / terrainParams.z);
            color = cloud.rgb + cloud.a * color;
        }
    }
    return float4(color + shafts, 1);
}
