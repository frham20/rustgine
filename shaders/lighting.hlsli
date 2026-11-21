cbuffer Frame : register(b0) {
    column_major float4x4 viewProjection;
    column_major float4x4 lightProjection[4];
    float4 camera;
    float4 sun;       // Surface-to-light direction and incident irradiance.
    float4 grassParams; // Wind time in seconds, followed by the linear RGB grass color scale.
    float4 outputParams; // Exposure multiplier, HDR flag, paper white and peak in nits.
    float4 terrainParams; // Grid spacing, viewport height, pixel scale and viewport width.
    uint4 terrainGrid; // Grid cells per side, root count, nodes per root and pool capacity.
    uint4 fpsHud; // Glyph pixel scale (zero hides the HUD), then eight packed glyph indices.
    float2 depthProjection; // Perspective matrix Z coefficients for reconstructing view depth.
    float cloudWind; // Wrapped horizontal displacement in cloud texture coordinates.
    float lakeLevel; // Lake elevation in metres.
    float4 terrainFrustum[6]; // World-space inward planes for terrain LOD selection.
    float4 shadowRanges; // Outer radius of each alpine cascade, in metres.
    uint cloudRenderDivisor;
    uint cloudViewSteps;
    uint cloudLightSteps;
    uint cloudShadowSteps;
    uint sunShaftRadialSamples;
    uint sunShaftEdgeRefinement;
};
cbuffer ShadowPass : register(b3) { uint shadowSlice; }
Texture2DArray<float> shadowDepth : register(t1);
SamplerComparisonState shadowSampler : register(s0);
Texture2D<float> cloudShadowMap : register(t15);
SamplerState cloudSampler : register(s2);

float sampleShadow(float3 world, uint slice, float bias) {
    float4 light = mul(lightProjection[slice], float4(world, 1));
    float3 p = light.xyz / light.w;
    float2 uv = p.xy * float2(0.5, -0.5) + 0.5;
    if (any(uv < 0) || any(uv > 1) || p.z <= 0 || p.z >= 1) return 1;
    float sum = 0;
    [unroll] for (int y = -1; y <= 1; ++y) {
        [unroll] for (int x = -1; x <= 1; ++x) {
            sum += shadowDepth.SampleCmpLevelZero(shadowSampler, float3(uv, slice), p.z - bias, int2(x, y));
        }
    }
    return sum / 9;
}

float visibility(float3 world, float3 n) {
    float range = distance(world, camera.xyz);
    if (range >= shadowRanges.w) return 1;
    uint slice = (uint)(range > shadowRanges.x) + (uint)(range > shadowRanges.y) + (uint)(range > shadowRanges.z);
    uint width, height, layers;
    shadowDepth.GetDimensions(width, height, layers);
    float texel = 2.04 * shadowRanges[slice] / width;
    // A world-space receiver offset follows the footprint of each cascade.
    float shadow = sampleShadow(world + n * (0.65 * texel), slice, 0.000015);
    float blend = smoothstep(0.86 * shadowRanges[slice], shadowRanges[slice], range);
    if (blend > 0) {
        float next = 1;
        if (slice < 3) {
            float nextTexel = 2.04 * shadowRanges[slice + 1] / width;
            next = sampleShadow(world + n * (0.65 * nextTexel), slice + 1, 0.000015);
        }
        shadow = lerp(shadow, next, blend);
    }
    return shadow;
}

static const float CLOUD_SHADOW_TEXEL = 96; // metres on the reference lake plane
float2 cloudShadowOrigin() {
    return floor(camera.xz / CLOUD_SHADOW_TEXEL) * CLOUD_SHADOW_TEXEL;
}

float cloudVisibility(float3 world) {
    if (cloudShadowSteps == 0 || sun.y <= 0.025) return 1;
    uint w, h;
    cloudShadowMap.GetDimensions(w, h);
    float2 reference = world.xz - (world.y - lakeLevel) * sun.xz / max(sun.y, 0.025);
    float2 uv = 0.5 + (reference - cloudShadowOrigin()) / (CLOUD_SHADOW_TEXEL * float2(w,h));
    float edge = max(abs(2 * uv.x - 1), abs(2 * uv.y - 1));
    if (edge >= 1) return 1;
    float transmission = cloudShadowMap.SampleLevel(cloudSampler, uv, 0);
    // Above the deck only the cloud column above the receiver can block sunlight.
    float below = 1 - smoothstep(1500, 3300, world.y);
    float weight = below * (1 - smoothstep(0.91, 1, edge)) * smoothstep(0.025, 0.12, sun.y);
    return lerp(1, transmission, weight);
}

float3 fresnel(float cosTheta, float3 f0) {
    return f0 + (1 - f0) * pow(1 - saturate(cosTheta), 5);
}

float daylightFill() {
    return lerp(0.025, 1.0, smoothstep(-0.18, 0.15, sun.y));
}

float3 shadeSurface(float3 world, float3 n, float3 base, float metal, float rough, bool matteGround) {
    const float pi = 3.14159265;
    float3 v = normalize(camera.xyz - world);
    float3 l = sun.xyz;
    float3 h = normalize(v + l);
    float nl = saturate(dot(n, l)), nv = max(saturate(dot(n, v)), 0.001);
    float nh = saturate(dot(n, h)), vh = saturate(dot(v, h));
    float a = rough * rough, a2 = a * a;
    float d = nh * nh * (a2 - 1) + 1;
    float distribution = a2 / (pi * d * d);
    float gv = nl * sqrt(nv * nv * (1 - a2) + a2);
    float gl = nv * sqrt(nl * nl * (1 - a2) + a2);
    float smith = 0.5 / max(gv + gl, 0.00001);
    float3 f0 = lerp(matteGround ? 0.025 : 0.04, base, metal);
    float3 f = fresnel(vh, f0);
    float3 diffuse = (1 - f) * (1 - metal) * base / pi;
    float3 specular = distribution * smith * f * (matteGround ? 0.45 : 1.0);
    float irradiance = sun.w * (matteGround ? 0.65 : 1.0) * smoothstep(0.0, 0.11, l.y);
    float3 color = 0;
    if (irradiance > 0.0 && nl > 0.0) {
        color = (diffuse + specular) * nl * irradiance
              * float3(1.0, 0.926, 0.82) * visibility(world, n) * cloudVisibility(world);
    }

    float hemi = 0.5 + 0.5 * n.y;
    float fill = daylightFill();
    if (matteGround) {
        // Diffuse outdoor fill avoids white grazing reflections across rough soil and foliage.
        return color + fill * base * lerp(float3(0.10, 0.09, 0.07), float3(0.26, 0.30, 0.34), hemi);
    }

    // Low-frequency studio environment. The broad reflection keeps unlit metal readable.
    color += fill * (1 - metal) * base * lerp(float3(0.12, 0.15, 0.16), float3(0.48, 0.58, 0.66), hemi);
    float3 reflected = reflect(-v, n);
    float sky = smoothstep(-0.22 - rough, 0.35 + rough, reflected.y);
    float strip = smoothstep(0.84 - rough * 0.28, 0.99, dot(reflected, normalize(float3(1, 0.8, -0.3))));
    color += fill * fresnel(nv, f0) * (lerp(float3(0.10, 0.15, 0.17), float3(0.75, 0.91, 1.12), sky)
                           + strip * float3(0.8, 0.94, 1.0)) * (1 - rough * 0.55);
    return color;
}


