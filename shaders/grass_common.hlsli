struct Patch {
    float2 center;
    float radius;
    float height;
    uint seed;
    float bladeCount;
    float fade;
    float widthScale;
};

static const uint MAX_BLADES = 32;
static const uint TRIANGLES_PER_BLADE = 6;
static const uint MESH_GROUPS_X = 1024;

uint hash(uint value) {
    value ^= value >> 16;
    value *= 0x7feb352d;
    value ^= value >> 15;
    value *= 0x846ca68b;
    return value ^ (value >> 16);
}
