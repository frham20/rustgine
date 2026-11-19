cbuffer Frame : register(b0) {
    column_major float4x4 viewProjection;
    column_major float4x4 model;
    column_major float4x4 lightProjection;
    float4 camera;
    float4 sun;       // Surface-to-light direction and incident irradiance.
    float4 grassParams; // Wind time in seconds, followed by the linear RGB grass color scale.
    float4 outputParams; // Exposure multiplier, HDR flag, paper white and peak in nits.
    float4 terrainParams; // Grid spacing, viewport height, pixel scale and viewport width.
    uint4 terrainGrid; // Grid cells per side, root count, nodes per root and pool capacity.
    uint4 fpsHud; // Glyph pixel scale (zero hides the HUD), then eight packed glyph indices.
    float2 depthProjection; // Perspective matrix Z coefficients for reconstructing view depth.
    float4 terrainFrustum[6]; // World-space inward planes for terrain LOD selection.
};
Texture2D<float> shadowDepth : register(t1);
SamplerComparisonState shadowSampler : register(s0);

float visibility(float3 world) {
    float4 light = mul(lightProjection, float4(world, 1));
    float3 p = light.xyz / light.w;
    float2 uv = p.xy * float2(0.5, -0.5) + 0.5;
    if (any(uv < 0) || any(uv > 1) || p.z <= 0 || p.z >= 1) return 1;
    float sum = 0;
    [unroll] for (int y = -1; y <= 1; ++y) {
        [unroll] for (int x = -1; x <= 1; ++x) {
            sum += shadowDepth.SampleCmpLevelZero(shadowSampler, uv, p.z - 0.00035, int2(x, y));
        }
    }
    return sum / 9;
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
              * float3(1.0, 0.926, 0.82) * visibility(world);
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


