// Distances inside this model are in kilometers, with sea level at world y = 0.
static const float EARTH_RADIUS = 6360.0;
static const float ATMOSPHERE_HEIGHT = 60.0;
static const float RAYLEIGH_HEIGHT = 8.0;
static const float3 RAYLEIGH = float3(0.005802, 0.013558, 0.033100);

struct AtmosphereSample {
    float3 radiance;
    float3 transmittance;
    bool hitsGround;
};

// Squared radius minus the squared Earth radius, without subtracting two large squares.
float radialOffsetSquared(float3 position) {
    return dot(position, position) + 2.0 * EARTH_RADIUS * position.y;
}

float3 sunTransmittance(float radius, float sunProjection, float density) {
    // Approximate the solar column by the secant law, bounded by the curved tangent column.
    float zenithCos = saturate(sunProjection / radius);
    float cosSquared = zenithCos * zenithCos;
    float tangentCorrection = (2.0 * RAYLEIGH_HEIGHT) / (3.14159265 * radius);
    float column = density * RAYLEIGH_HEIGHT * rsqrt(lerp(tangentCorrection, 1.0, cosSquared));
    return exp(-RAYLEIGH * column);
}

float2 earthShadowInterval(float2 ray, float radialOffset, float projection,
                           float sunProjection, float viewSunCos, float3 direction) {
    float2 empty = float2(ray.x, ray.x);
    // Only the side of the planet facing away from the Sun can cast a shadow.
    float startSun = sunProjection + ray.x * viewSunCos;
    float endSun = sunProjection + ray.y * viewSunCos;
    if (min(startSun, endSun) >= 0) return empty;
    if (startSun > 0) ray.x = -sunProjection / viewSunCos;
    if (endSun > 0) ray.y = -sunProjection / viewSunCos;

    // Intersect the ray with Earth's cylindrical shadow for parallel sunlight.
    float3 crossSun = cross(direction, sun.xyz);
    float a = dot(crossSun, crossSun);
    float b = projection - sunProjection * viewSunCos;
    float c = radialOffset - sunProjection * sunProjection;
    if (a == 0) return c < 0 ? ray : empty;
    float discriminant = b * b - a * c;
    if (discriminant <= 0) return empty;

    // Use the stable quadratic form so the near root doesn't lose precision.
    float q = -b - (b >= 0 ? 1 : -1) * sqrt(discriminant);
    float2 roots = float2(q / a, c / q);
    return clamp(float2(min(roots.x, roots.y), max(roots.x, roots.y)), ray.x, ray.y);
}

AtmosphereSample sampleAtmosphere(float3 eye, float3 direction, float limit, uint steps) {
    AtmosphereSample result;
    result.radiance = 0;
    result.transmittance = 1;
    result.hitsGround = false;

    float radialOffset = radialOffsetSquared(eye);
    float projection = dot(eye, direction) + EARTH_RADIUS * direction.y;
    float groundDiscriminant = projection * projection - radialOffset;
    float topDiscriminant = groundDiscriminant
        + ATMOSPHERE_HEIGHT * (2.0 * EARTH_RADIUS + ATMOSPHERE_HEIGHT);
    if (topDiscriminant < 0) return result;

    float topRoot = sqrt(topDiscriminant);
    float start = max(0, -projection - topRoot);
    float end = min(limit, -projection + topRoot);
    if (projection < 0 && groundDiscriminant >= 0) {
        float ground = radialOffset / (-projection + sqrt(groundDiscriminant));
        result.hitsGround = ground <= end;
        end = min(end, ground);
    }
    if (end <= start) return result;

    float mu = dot(direction, sun.xyz);
    float sunProjection = dot(eye, sun.xyz) + EARTH_RADIUS * sun.y;
    float2 shadow = earthShadowInterval(float2(start, end), radialOffset, projection,
                                        sunProjection, mu, direction);
    float previous = start;
    [unroll] for (uint i = 1; i <= steps; ++i) {
        // Smaller intervals near the viewer resolve the denser air in long sky rays.
        float fraction = float(i) / float(steps);
        float next = lerp(start, end, fraction * fraction);
        float3 position = eye + (0.5 * (previous + next)) * direction;
        float offset = radialOffsetSquared(position);
        float radius = sqrt(EARTH_RADIUS * EARTH_RADIUS + offset);
        float height = max(0, offset / (radius + EARTH_RADIUS));
        float density = exp(-height / RAYLEIGH_HEIGHT);
        float3 extinction = RAYLEIGH * density;
        float3 transmittance = exp(-extinction * (next - previous));
        float2 clippedShadow = clamp(shadow, previous, next);
        // A segment can be partly lit. Integrate each lit end with its own view attenuation.
        if (clippedShadow.y - clippedShadow.x < next - previous) {
            float3 lit = 1.0 - transmittance;
            if (clippedShadow.y > clippedShadow.x) {
                lit = (1.0 - exp(-extinction * (clippedShadow.x - previous)))
                    + exp(-extinction * (clippedShadow.y - previous))
                    * (1.0 - exp(-extinction * (next - clippedShadow.y)));
            }
            float sunAtSample = sunProjection + (0.5 * (previous + next)) * mu;
            float3 sunLight = sunTransmittance(radius, sunAtSample, density);
            result.radiance += result.transmittance * lit * sunLight;
        }
        result.transmittance *= transmittance;
        previous = next;
    }
    float phase = (3.0 / (16.0 * 3.14159265)) * (1.0 + mu * mu);
    result.radiance *= sun.w * phase;
    return result;
}
