// Distances are in km. Density and extinction follow Bittner's shell raymarch.
Texture3D<float4> cloudNoise : register(t12);
SamplerState cloudSampler : register(s2);
static const float CLOUD_BASE = 1.5;
static const float CLOUD_TOP = 3.3;
static const float CLOUD_RANGE = 64.0;

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

float cloudDensity(float3 p, bool detailed) {
    float altitude = radialOffsetSquared(p) / (2 * EARTH_RADIUS);
    float h = saturate((altitude - CLOUD_BASE) / (CLOUD_TOP - CLOUD_BASE));
    float profile = smoothstep(0, 0.13, h) * (1 - smoothstep(0.45, 1, h));
    float3 uv = p / 12.0;
    uv.x -= cloudWind;
    float4 shape = cloudNoise.SampleLevel(cloudSampler, uv, 0);
    float weather = cloudNoise.SampleLevel(cloudSampler, float3(uv.xz * 0.5, 0.37), 0).b;
    float field = shape.r + 0.48 * (weather - 0.5) - 0.39 - 0.42 * (1 - profile);
    if (detailed && field > 0) {
        float4 erosion = cloudNoise.SampleLevel(cloudSampler, uv * 3, 0);
        field -= 0.16 * (1 - erosion.g) + 0.05 * (1 - shape.a);
    }
    return saturate(field * 4.0) * profile;
}

// Premultiplied in-scattered radiance and remaining transmittance.
float4 marchClouds(float3 eye, float3 dir, float2 span) {
    float4 cloud = float4(0, 0, 0, 1);
    if (span.y <= span.x) return cloud;
    float stepKm = max(0.065, (span.y - span.x) / 40);
    float cosine = dot(dir, sun.xyz);
    const float g = 0.60;
    float phase = (1 - g*g) / (12.56637 * pow(1 + g*g - 2*g*cosine, 1.5));
    float daylight = smoothstep(-0.025, 0.09, sun.y);
    float3 sunlight = sunTransmittance(EARTH_RADIUS + CLOUD_BASE,
        EARTH_RADIUS * sun.y, exp(-CLOUD_BASE / RAYLEIGH_HEIGHT)) * sun.w * daylight;
    float3 ambient = float3(0.09, 0.12, 0.17) * daylightFill();
    float t = span.x;
    [loop] for (int i = 0; i < 40 && t < span.y && cloud.a > 0.012; ++i) {
        float ds = min(stepKm, span.y - t);
        float3 p = eye + dir * (t + 0.5 * ds);
        float density = cloudDensity(p, true);
        if (density > 0.001) {
            float opticalDepth = 0;
            if (daylight > 0) {
                float lightStep = 0.12;
                float offset = 0;
                [unroll] for (int j = 0; j < 4; ++j) {
                    opticalDepth += lightStep * cloudDensity(p + sun.xyz * (offset + 0.5 * lightStep), false);
                    offset += lightStep;
                    lightStep *= 2;
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
    float2 size = ceil(float2(terrainParams.w, terrainParams.y) / 4);
    float3 dir = normalize(cameraRay(position.xy / size));
    float3 eye = camera.xyz * 0.001;
    return marchClouds(eye, dir, cloudInterval(eye, dir, CLOUD_RANGE));
}
