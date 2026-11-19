#include "lighting.hlsli"

struct TerrainNode {
    uint3 corners;
    float error;
};

StructuredBuffer<TerrainNode> terrainNodes : register(t3);
StructuredBuffer<float4> terrainSamples : register(t4);
StructuredBuffer<float4> resolvedVertices : register(t8);

uint2 gridPosition(uint packed) { return uint2(packed & 65535, packed >> 16); }

uint gridIndex(uint2 p) {
    return p.y * (terrainGrid.x + 1) + p.x;
}

float3 terrainPosition(uint2 p, float height) {
    float2 xz = (float2(p) - 0.5 * terrainGrid.x) * terrainParams.x;
    return float3(xz.x, height, xz.y);
}

uint bitmapWords() { return terrainGrid.w / 32; }

// The leaves of the reduction tree count packed 32-bit occupancy words.
// A copy of each word after the tree keeps rank queries stable during updates.
uint rankedSlot(StructuredBuffer<uint> tree, uint rank, bool allocated) {
    uint node = 1, capacity = terrainGrid.w;
    uint words = bitmapWords();
    while (node < words) {
        node *= 2;
        capacity /= 2;
        uint left = tree[node];
        if (!allocated) left = capacity - left;
        if (rank >= left) {
            rank -= left;
            node++;
        }
    }
    uint bits = tree[words + node];
    if (!allocated) bits = ~bits;
    uint slot = (node - words) * 32;
    [unroll] for (uint width = 16; width > 0; width >>= 1) {
        uint low = bits & ((1u << width) - 1);
        uint count = countbits(low);
        if (rank < count) {
            bits = low;
        } else {
            bits >>= width;
            slot += width;
            rank -= count;
        }
    }
    return slot;
}
