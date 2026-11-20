#include "terrain_common.hlsli"

// Zero for internal nodes, slot + 1 for the triangles in this frame's draws.
StructuredBuffer<uint> activeLeaves : register(t7);

float cross2(float2 a, float2 b) { return a.x * b.y - a.y * b.x; }

float4 groundSurface(float2 worldXZ) {
    float2 p = worldXZ / terrainParams.x + 0.5 * terrainGrid.x;
    uint tiles = uint(sqrt(float(terrainGrid.y / 2)));
    uint span = terrainGrid.x / tiles;
    uint2 tile = min(uint2(p / span), tiles - 1);
    float2 nw = float2(tile * span), se = nw + span;
    float2 local = p - nw;
    bool upper = local.x > local.y;
    uint root = (2 * (tile.y * tiles + tile.x) + uint(upper)) * terrainGrid.z;
    float2 a = upper ? se : nw;
    float2 b = upper ? float2(se.x, nw.y) : float2(nw.x, se.y);
    float2 c = upper ? nw : se;
    uint node = 1;
    // Match the active triangle while the terrain splits or merges.
    while (node < terrainGrid.z / 2 && activeLeaves[root + node] == 0) {
        float2 midpoint = (a + c) * 0.5;
        bool right = cross2(midpoint - b, p - b) > 0;
        float2 nextA = right ? c : b;
        c = right ? b : a;
        b = midpoint;
        a = nextA;
        node = 2 * node + uint(right);
    }
    float area = cross2(b - a, c - a);
    float u = cross2(p - a, c - a) / area;
    float v = cross2(b - a, p - a) / area;
    float4 av = resolvedVertices[gridIndex(uint2(a))];
    float4 bv = resolvedVertices[gridIndex(uint2(b))];
    float4 cv = resolvedVertices[gridIndex(uint2(c))];
    return av + u * (bv - av) + v * (cv - av);
}
