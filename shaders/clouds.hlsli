// Distances are in km. Density and extinction follow Bittner's shell raymarch.
Texture3D<float4> cloudNoise : register(t12);
SamplerState cloudVolumeSampler : register(s1);
cbuffer CloudShadowPass : register(b1) { uint cloudShadowSize; }
static const float CLOUD_BASE = 1.5;
static const float CLOUD_TOP = 3.3;
static const float CLOUD_RANGE = 64.0;

float2 cloudRenderSize(float2 viewportSize) {
    return ceil(viewportSize / cloudRenderDivisor);
}

float3 cameraRay(float2 uv) {
    float2 screen = (2 * uv - 1) * float2(1, -1);
    float3 right = viewProjection[0].xyz, up = viewProjection[1].xyz;
    return viewProjection[3].xyz + screen.x * right / dot(right, right)
                                 + screen.y * up / dot(up, up);
}

float2 cloudInterval(float3 eye, float3 dir, float limit) {
    float radial = radialOffsetSquared(eye);
    float projection = dot(eye, dir) + EARTH_RADIUS * dir.y;
    float bottom = CLOUD_BASE * (2 * EARTH_RADIUS + CLOUD_BASE) - radial;
    float top = CLOUD_TOP * (2 * EARTH_RADIUS + CLOUD_TOP) - radial;
    float det = projection * projection + top;
    if (det <= 0) return 0;
    float root = sqrt(det);
    float2 span = float2(max(0, -projection - root), min(limit, -projection + root));
    det = projection * projection + bottom;
    if (det > 0) {
        // Select the nearest portion of the shell ahead of the viewer.
        root = sqrt(det);
        if (bottom > 0) span.x = max(span.x, -projection + root);
        else if (-projection - root >= 0) span.y = min(span.y, -projection - root);
    }
    // Below the deck, a downward ray may hit the planet before the far shell.
    if (projection < 0 && projection * projection >= radial) {
        span.y = min(span.y, radial / max(0.001, -projection + sqrt(max(0, projection * projection - radial))));
    }
    return span;
}

// March samples cover a long segment along the ray and a much narrower
// cross-section at the pixel. Keep those axes separate for anisotropic sampling.
void cloudGradients(float3 dir, float stepKm, float beamKm, out float3 dx, out float3 dy) {
    float3 tangent = normalize(cross(dir, abs(dir.y) < 0.9 ? float3(0,1,0) : float3(1,0,0)));
    dx = stepKm > beamKm ? dir * (stepKm / 12) : cross(dir,tangent) * (beamKm / 12);
    dy = tangent * (beamKm / 12);
}

float cloudDensity(float3 p, bool detailed, float3 dx, float3 dy) {
    float altitude = radialOffsetSquared(p) / (2 * EARTH_RADIUS);
    float h = saturate((altitude - CLOUD_BASE) / (CLOUD_TOP - CLOUD_BASE));
    float profile = smoothstep(0, 0.13, h) * (1 - smoothstep(0.45, 1, h));
    float3 uv = p / 12.0;
    uv.x -= cloudWind;
    float4 shape = cloudNoise.SampleGrad(cloudVolumeSampler, uv, dx, dy);
    float weather = cloudNoise.SampleGrad(cloudVolumeSampler, float3(uv.xz * 0.5, 0.37),
                                         float3(dx.xz * 0.5, 0), float3(dy.xz * 0.5, 0)).b;
    float field = shape.r + 0.48 * (weather - 0.5) - 0.39 - 0.42 * (1 - profile);
    if (detailed && field > 0) {
        float4 erosion = cloudNoise.SampleGrad(cloudVolumeSampler, uv * 3, dx * 3, dy * 3);
        field -= 0.16 * (1 - erosion.g) + 0.05 * (1 - shape.a);
    }
    return saturate(field * 4.0) * profile;
}

// Premultiplied in-scattered radiance and remaining transmittance.
float4 marchClouds(float3 eye, float3 dir, float2 span, float pixelAngle) {
    float4 cloud = float4(0, 0, 0, 1);
    if (span.y <= span.x) return cloud;
    float stepKm = max(2.6, span.y - span.x) / cloudViewSteps;
    float cosine = dot(dir, sun.xyz);
    const float g = 0.60;
    float phase = (1 - g*g) / (12.56637 * pow(1 + g*g - 2*g*cosine, 1.5));
    float daylight = smoothstep(-0.025, 0.09, sun.y);
    float3 sunlight = sunTransmittance(EARTH_RADIUS + CLOUD_BASE,
        EARTH_RADIUS * sun.y, exp(-CLOUD_BASE / RAYLEIGH_HEIGHT)) * sun.w * daylight;
    float3 ambient = float3(0.09, 0.12, 0.17) * daylightFill();
    // Redistribute samples over the same 1.8 km of cloud toward the sun.
    float lightGrowth = exp2(4.0 / max(1u, cloudLightSteps));
    float firstLightStep = 0.12 * (lightGrowth - 1);
    float t = span.x;
    [loop] for (uint i = 0; i < cloudViewSteps && t < span.y && cloud.a > 0.012; ++i) {
        float ds = min(stepKm, span.y - t);
        float3 p = eye + dir * (t + 0.5 * ds);
        float beam = (t + 0.5 * ds) * pixelAngle;
        float3 dx, dy;
        cloudGradients(dir, ds, beam, dx, dy);
        float density = cloudDensity(p, true, dx, dy);
        if (density > 0.001) {
            float opticalDepth = 0;
            if (daylight > 0) {
                float lightStep = firstLightStep;
                float offset = 0;
                [loop] for (uint j = 0; j < cloudLightSteps; ++j) {
                    cloudGradients(sun.xyz, lightStep, beam, dx, dy);
                    opticalDepth += lightStep * cloudDensity(p + sun.xyz * (offset + 0.5 * lightStep), false, dx, dy);
                    offset += lightStep;
                    lightStep *= lightGrowth;
                }
            }
            float attenuation = exp(-3.6 * opticalDepth);
            float3 light = ambient + sunlight * (0.085 + phase) * (0.15 + 0.85 * attenuation);
            float opacity = 1 - exp(-3.6 * density * ds);
            // Distant clouds dissolve into the existing atmospheric horizon.
            opacity *= 1 - smoothstep(24.0, CLOUD_RANGE, t);
            cloud.rgb += cloud.a * opacity * light;
            cloud.a *= 1 - opacity;
        }
        t += ds;
    }
    return cloud;
}

float4 clouds_ps(float4 position : SV_Position) : SV_Target {
    float2 size = cloudRenderSize(float2(terrainParams.w, terrainParams.y));
    float3 dir = normalize(cameraRay(position.xy / size));
    float3 eye = camera.xyz * 0.001;
    float pixelAngle = terrainParams.y / (size.y * terrainParams.z);
    return marchClouds(eye, dir, cloudInterval(eye, dir, CLOUD_RANGE), pixelAngle);
}

float cloud_shadow_ps(float4 position : SV_Position) : SV_Target {
    float2 xz = cloudShadowOrigin() + (position.xy - 0.5 * cloudShadowSize) * CLOUD_SHADOW_TEXEL;
    float3 eye = float3(xz.x, lakeLevel, xz.y) * 0.001;
    float2 span = cloudInterval(eye, sun.xyz, CLOUD_RANGE);
    if (span.y <= span.x) return 1;
    float stepKm = (span.y - span.x) / cloudShadowSteps;
    float opticalDepth = 0;
    float3 dx, dy;
    cloudGradients(sun.xyz, stepKm, CLOUD_SHADOW_TEXEL * 0.001, dx, dy);
    [loop] for (uint i = 0; i < cloudShadowSteps; ++i) {
        opticalDepth += cloudDensity(eye + sun.xyz * (span.x + (i + 0.5) * stepKm), false, dx, dy) * stepKm;
    }
    // Broad, low-frequency extinction from the same moving density field as the sky.
    return 0.20 + 0.80 * exp(-3.6 * opticalDepth);
}
