float terrainHash(float2 lattice) {
    float3 p = frac(float3(lattice.xyx) * float3(0.1031, 0.1030, 0.0973));
    p += dot(p, p.yxz + 33.33);
    return frac((p.x + p.y) * p.z);
}

float mottling(float2 p) {
    const float2 axis = float2(1, 0);
    float2 lattice = floor(p);
    float2 f = frac(p);
    f = f * f * (3 - 2 * f);
    return lerp(lerp(terrainHash(lattice), terrainHash(lattice + axis), f.x),
                lerp(terrainHash(lattice + axis.yx), terrainHash(lattice + 1), f.x), f.y);
}

// Footprint in procedural lattice cells per pixel. Unresolved octaves converge
// to their spatial mean, including on surfaces viewed at a grazing angle.
float noiseWeight(float cells) { return 1 - smoothstep(0.30, 1.25, cells); }
float filteredMottling(float2 p, float cells) { return lerp(0.5, mottling(p), noiseWeight(cells)); }

Texture2DArray<float4> landforms : register(t16);
SamplerState landformSampler : register(s3);

float3 alpineLandforms(float2 p, float2 dx, float2 dy) {
    float blend = smoothstep(2250, 2500, max(abs(p.x), abs(p.y)));
    float3 inner = 0, outer = 0;
    if (blend < 1) inner = landforms.SampleGrad(landformSampler, float3(p / 5000 + 0.5, 0), dx / 5000, dy / 5000).rgb;
    if (blend > 0) outer = landforms.SampleGrad(landformSampler, float3(p / 32000 + 0.5, 1), dx / 32000, dy / 32000).rgb;
    return lerp(inner, outer, blend);
}

static const float3 CANOPY_COLOR = float3(0.033,0.073,0.040);
float canopyBlend(float crownPixels) { return 1 - smoothstep(0.8, 4.0, crownPixels); }

// Weights of loose talus and lingering snow. Use the unperturbed ground normal.
float2 alpineDeposits(float3 world, float3 n, float2 terrain, float footprint) {
    if (world.y < lakeLevel + 220) return 0;
    float shelter = terrain.x, source = terrain.y;
    float slope = length(n.xz);
    float altitude = world.y - lakeLevel;
    float patch = 0.65 * filteredMottling(world.xz * 0.011, footprint * 0.011)
                + 0.35 * filteredMottling(world.xz * 0.048 + 13, footprint * 0.048);
    float scree = smoothstep(220, 510, altitude) * source * smoothstep(0.13, 0.38, slope)
                * (1 - smoothstep(0.62, 0.84, slope)) * smoothstep(0.08, 0.70, shelter + patch * 0.37);
    float lee = saturate(0.45 + dot(n.xz, float2(0.20, -0.98)) * 0.85);
    float cold = smoothstep(430, 715, altitude);
    float retention = 0.59 * shelter + 0.22 * lee + 0.29 * patch + 0.15 * cold;
    float snow = cold * (1 - smoothstep(0.57, 0.86, slope)) * smoothstep(0.63, 0.77, retention);
    return float2(scree, snow);
}

struct TerrainMaterials {
    float slope;
    float moisture;
    float meadow;
    float organic;
    float mineral;
    float rock;
};

float meadowCoverage(TerrainMaterials material) {
    float mixedGround = smoothstep(0.22, 0.82, material.meadow);
    float exposedRock = smoothstep(0.40, 0.68, material.rock);
    return mixedGround * (1.0 - exposedRock);
}

float3 meadowBladeColor(float variation) {
    return lerp(float3(0.024, 0.068, 0.016), float3(0.103, 0.175, 0.038), variation);
}

float meadowVariation(float2 xz, float footprint) {
    return filteredMottling(xz * 0.051 + float2(14, 91), footprint * 0.051);
}

float3 meadowCanopyColor(float variation) {
    return meadowBladeColor(lerp(0.25, 0.75, variation)) * 0.68;
}

float3 tintMeadow(float3 base, float3 scale) {
    [branch] if (all(scale == 1.0)) return base;

    const float3 luma = float3(0.2126, 0.7152, 0.0722);
    float luminance = dot(base, luma);
    float3 tinted = base * scale;
    float tintedLuminance = dot(tinted, luma);
    if (tintedLuminance <= 0.0) return luminance.xxx;

    tinted *= luminance / tintedLuminance;
    float peak = max(tinted.r, max(tinted.g, tinted.b));
    // Reduce saturation at constant luminance if a channel would clip in the G-buffer.
    if (peak > 1.0) {
        tinted = lerp(luminance.xxx, tinted, (1.0 - luminance) / (peak - luminance));
    }
    return tinted;
}

TerrainMaterials terrainMaterials(float3 world, float3 normal, float footprint) {
    TerrainMaterials material;
    float2 xz = world.xz;
    material.slope = length(normal.xz);
    float upland = smoothstep(88, 183, world.y);
    if (lakeLevel > 0) {
        upland = smoothstep(lakeLevel + 280, lakeLevel + 650, world.y);
        // Meadow and conifer duff persist on moderate alpine banks.
        material.slope *= lerp(0.55, 0.92, upland);
    }
    // Long-term exposure biases the material blend independently of the current hour.
    float aspect = dot(normal.xz, normalize(float2(-0.67, 0.50)));
    float groundCover = filteredMottling(xz * 0.026 + float2(17, 83), footprint * 0.026);
    float drainage = filteredMottling(xz * 0.012, footprint * 0.012);
    material.moisture = saturate(0.72 * (1.0 - upland)
        + 0.24 * (1.0 - saturate(2.5 * material.slope)) + 0.42 * (drainage - 0.5) - 0.20 * aspect);

    // Soil and bedrock come through on exposed slopes. Sheltered lowlands collect organic matter.
    material.rock = smoothstep(0.24, 0.39, material.slope + 0.16 * (groundCover - 0.5) + 0.07 * upland);
    material.mineral = (1.0 - material.rock) * smoothstep(0.40, 0.69,
        1.17 * material.slope + 0.38 * groundCover + 0.16 * upland - 0.18 * material.moisture);
    material.organic = (1.0 - material.rock - material.mineral) * smoothstep(0.62, 0.93, material.moisture)
                     * smoothstep(0.24, 0.82, groundCover);
    material.meadow = 1.0 - material.rock - material.mineral - material.organic;
    if (lakeLevel > 0) {
        float bank = smoothstep(lakeLevel + 0.3, lakeLevel + 4.5, world.y);
        material.mineral += material.meadow * (1 - bank);
        material.meadow *= bank;
    }
    return material;
}
