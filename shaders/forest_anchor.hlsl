#include "terrain_surface.hlsli"

RWStructuredBuffer<float4> instances : register(u0);
cbuffer Forest : register(b1) { uint instanceCount; }

[numthreads(64, 1, 1)]
void anchor_trees(uint id : SV_DispatchThreadID) {
    if (id >= instanceCount) return;
    instances[id].y = groundSurface(instances[id].xz).x - 0.35;
}
