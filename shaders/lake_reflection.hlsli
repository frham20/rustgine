#include "atmosphere.hlsli"

// The flat lake reflects geometry and directions about the same world-space plane.
float3 mirrorPoint(float3 p) { return float3(p.x, 2 * lakeLevel - p.y, p.z); }

float3 aerialColor(float3 eye, float3 world, float3 color) {
    float3 delta = world - eye;
    float meters = length(delta);
    float3 dir = delta / max(meters, 0.001);
    // Integrate an exponential layer of valley aerosol along the segment.
    // The horizontal limit avoids cancellation when both endpoints are level.
    float2 h = max(0, float2(eye.y, world.y) - lakeLevel) / 950;
    float dh = abs(h.y - h.x);
    float density = exp(-min(h.x, h.y)) * (dh < 0.001 ? 1 - dh * 0.5 : (1 - exp(-dh)) / dh);
    float haze = 1 - exp(-meters * (0.000024 + 0.000125 * density));
    float forward = pow(saturate(dot(dir, sun.xyz)), 4);
    float3 horizon = lerp(float3(0.19,0.36,0.54), float3(0.47,0.52,0.57), 0.60 * forward);
    color = lerp(color, horizon * daylightFill(), haze);
    AtmosphereSample air = sampleAtmosphere(float3(eye.x, max(0,eye.y), eye.z) * 0.001, dir, meters * 0.001, 3);
    return air.radiance + air.transmittance * color;
}

float4 reflectedLand(float3 world, float3 normal, float3 base, float roughness) {
    float3 color = shadeSurface(world, normalize(normal), base, 0, roughness, true);
    float3 eye = mirrorPoint(camera.xyz);
    float t = saturate((lakeLevel - eye.y) / max(world.y - eye.y, 0.001));
    // The main water pass supplies the remaining leg from the lake to the camera.
    return float4(aerialColor(lerp(eye, world, t), world, color), 1);
}
