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

float meadowVariation(float2 xz) {
    return mottling(xz * 0.051 + float2(14, 91));
}

float3 meadowCanopyColor(float variation) {
    return meadowBladeColor(lerp(0.25, 0.75, variation)) * 0.68;
}

TerrainMaterials terrainMaterials(float3 world, float3 normal) {
    TerrainMaterials material;
    float2 xz = world.xz;
    material.slope = length(normal.xz);
    float upland = smoothstep(88, 183, world.y);
    // Long-term exposure biases the material blend independently of the current hour.
    float aspect = dot(normal.xz, normalize(float2(-0.67, 0.50)));
    float groundCover = mottling(xz * 0.026 + float2(17, 83));
    float drainage = mottling(xz * 0.012);
    material.moisture = saturate(0.72 * (1.0 - upland)
        + 0.24 * (1.0 - saturate(2.5 * material.slope)) + 0.42 * (drainage - 0.5) - 0.20 * aspect);

    // Soil and bedrock come through on exposed slopes. Sheltered lowlands collect organic matter.
    material.rock = smoothstep(0.24, 0.39, material.slope + 0.16 * (groundCover - 0.5) + 0.07 * upland);
    material.mineral = (1.0 - material.rock) * smoothstep(0.40, 0.69,
        1.17 * material.slope + 0.38 * groundCover + 0.16 * upland - 0.18 * material.moisture);
    material.organic = (1.0 - material.rock - material.mineral) * smoothstep(0.62, 0.93, material.moisture)
                     * smoothstep(0.24, 0.82, groundCover);
    material.meadow = 1.0 - material.rock - material.mineral - material.organic;
    return material;
}
