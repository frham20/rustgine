#include "terrain_common.hlsli"
#include "grass_common.hlsli"

cbuffer GrassGenerate : register(b1) {
    float4 lodEnds;
    float4 lodSpacings;
    float4 transitionWidths;
    uint4 gridSides;
    uint4 groupEnds;
    float drawDistance;
    float density;
    float heightScale;
    uint meshPath;
    uint fullDensity;
    uint groupCount;
    uint patchCapacity;
};

struct Candidate {
    Patch patch; // bladeCount is the total before dividing this cell into batches.
    uint fullOffset;
    uint tailOffset;
};

RWStructuredBuffer<Candidate> candidates : register(u0);
RWStructuredBuffer<uint> groupOffsets : register(u1);
RWStructuredBuffer<Patch> visiblePatches : register(u2);
RWByteAddressBuffer drawData : register(u3);

static const uint GROUP_WIDTH = 32;
static const uint SCAN_WIDTH = 128;
// 32 base blades at 500% density and the 2x coarse LOD multiplier.
static const uint MAX_FULL_BATCHES = 10;
static const uint ARGS_OFFSET = 8;
static const uint INDEXED_STRIDE = 24; // One root constant and the five indexed draw arguments.
static const uint TOTALS_OFFSET = ARGS_OFFSET + MAX_BLADES * INDEXED_STRIDE;
static const uint BUCKETS_OFFSET = TOTALS_OFFSET + MAX_BLADES * 4;
static const uint REJECTED = 0xffffffff;

float transitionStart(uint level) {
    float previous = level == 0 ? 0.0 : lodEnds[level - 1];
    return max(max(lodEnds[level] - transitionWidths[level], previous), lodEnds[level] * 0.5);
}

float approximateHeight(float2 xz) {
    float2 p = clamp(xz / terrainParams.x + 0.5 * terrainGrid.x, 0.0, float(terrainGrid.x));
    uint2 cell = min(uint2(p), terrainGrid.x - 1);
    float2 t = p - cell;
    uint i = gridIndex(cell), stride = terrainGrid.x + 1;
    float lo = lerp(terrainSamples[i].x, terrainSamples[i + 1].x, t.x);
    float hi = lerp(terrainSamples[i + stride].x, terrainSamples[i + stride + 1].x, t.x);
    return lerp(lo, hi, t.y);
}

Patch classifyCell(uint group, uint lane) {
    Patch patch = (Patch)0;
    uint level = uint(group >= groupEnds.x) + uint(group >= groupEnds.y) + uint(group >= groupEnds.z);
    uint firstGroup = level == 0 ? 0 : groupEnds[level - 1];
    uint cellIndex = (group - firstGroup) * GROUP_WIDTH + lane;
    uint side = gridSides[level];
    if (cellIndex >= side * side) return patch;

    float spacing = lodSpacings[level];
    float2 eyeCell = camera.xz / spacing;
    // Match the original grid's rounding away from zero at half cells.
    int2 origin = int2(sign(eyeCell) * floor(abs(eyeCell) + 0.5));
    int2 cell = origin + int2(cellIndex % side, cellIndex / side) - int(side / 2);
    uint seed = asuint(cell.x) * 0x8da6b343u ^ asuint(cell.y) * 0xd8163841u ^ level * 0x9e3779b9u;
    uint noise = hash(seed ^ 0x25b1ce3du);
    float2 jitter = (float2(noise & 0xffff, noise >> 16) + 0.5) / 65536.0 - 0.5;
    float2 xz = (float2(cell) + jitter) * spacing;
    float radius = max(0.47, 0.4 * spacing);
    float2 edge = abs(xz) + radius;
    float limit = min(drawDistance, lodEnds[level]);
    float2 delta = xz - camera.xz;
    if (any(edge > terrainParams.x * terrainGrid.x * 0.5) || dot(delta, delta) >= limit * limit) return patch;

    float3 center = float3(xz.x, approximateHeight(xz), xz.y);
    float reach = radius + 1.05 * max(heightScale, 1.0);
    [unroll] for (uint plane = 0; plane < 6; ++plane) {
        if (dot(terrainFrustum[plane], float4(center, 1)) < -reach) return patch;
    }
    float distanceToPatch = distance(center, camera.xyz);
    float incoming = level == 0 ? 1.0 : smoothstep(transitionStart(level - 1), lodEnds[level - 1], distanceToPatch);
    float outgoing = level == 3 ? 1.0 : 1.0 - smoothstep(transitionStart(level), lodEnds[level], distanceToPatch);
    float coverage = incoming * outgoing;
    if (coverage == 0.0) return patch;

    float heightNoise = float(hash(seed ^ 0x7301a2e7u) >> 16) / 65535.0;
    float height = (0.30 + 0.12 * heightNoise) * heightScale;
    float fade = smoothstep(0.16, 2.0, height * terrainParams.z / max(distanceToPatch, 1.0));
    if (level == 3 || drawDistance - 3.0 < lodEnds[level]) {
        fade *= 1.0 - smoothstep(drawDistance - 3.0, drawDistance, distanceToPatch);
    }
    if (fade < 0.005) return patch;

    float baseBlades = fullDensity != 0 ? 32.0 : 32.0
        - 26.0 * smoothstep(4.0, 19.0, distanceToPatch)
        - 4.0 * smoothstep(20.0, 48.0, distanceToPatch);
    float blades = baseBlades * (level == 0 || fullDensity != 0 ? 1.0 : 2.0) * density;
    if (coverage < 1.0) {
        uint blendSeed = seed ^ 0xa511e9b3u;
        float offset = float(((blendSeed ^ (blendSeed >> 16)) * 0x7feb352du) >> 8) / 16777216.0;
        blades = floor(floor(blades) * coverage + offset) + (offset < coverage ? frac(blades) : 0.0);
    }
    patch.center = xz;
    patch.radius = radius;
    patch.height = height;
    patch.seed = seed;
    patch.bladeCount = blades;
    patch.fade = fade;
    patch.widthScale = min(32.0 / baseBlades, 2.0);
    return patch;
}

groupshared uint groupBatches[GROUP_WIDTH];
groupshared uint groupBlades[GROUP_WIDTH];
groupshared uint tailMasks[MAX_BLADES];
groupshared uint fullMasks[MAX_FULL_BATCHES];

[numthreads(GROUP_WIDTH, 1, 1)]
void grass_classify(uint group : SV_GroupID, uint lane : SV_GroupIndex) {
    Patch patch = classifyCell(group, lane);
    uint full = uint(patch.bladeCount) / MAX_BLADES;
    uint tail = uint(ceil(patch.bladeCount - full * MAX_BLADES));
    uint batches = full + uint(tail != 0);
    Candidate candidate;
    candidate.patch = patch;
    candidate.fullOffset = REJECTED;
    candidate.tailOffset = 0;
    if (meshPath != 0) {
        groupBatches[lane] = batches;
        groupBlades[lane] = full * MAX_BLADES + tail;
        GroupMemoryBarrierWithGroupSync();
        if (lane == 0) {
            uint total = 0, blades = 0;
            for (uint i = 0; i < GROUP_WIDTH; ++i) {
                total += groupBatches[i];
                blades += groupBlades[i];
                groupBatches[i] = total;
            }
            groupOffsets[group] = total;
            groupOffsets[groupCount + group] = blades;
        }
        GroupMemoryBarrierWithGroupSync();
        if (batches != 0) candidate.fullOffset = groupBatches[lane] - batches;
    } else {
        tailMasks[lane] = 0;
        if (lane < MAX_FULL_BATCHES) fullMasks[lane] = 0;
        GroupMemoryBarrierWithGroupSync();
        uint bit = 1u << lane;
        if (tail != 0) InterlockedOr(tailMasks[tail - 1], bit);
        for (uint i = 0; i < full; ++i) InterlockedOr(fullMasks[i], bit);
        GroupMemoryBarrierWithGroupSync();

        uint total = countbits(tailMasks[lane]);
        if (lane == MAX_BLADES - 1) {
            for (uint i = 0; i < MAX_FULL_BATCHES; ++i) total += countbits(fullMasks[i]);
        }
        groupOffsets[lane * groupCount + group] = total;
        if (batches != 0) {
            uint before = bit - 1;
            candidate.fullOffset = countbits(tailMasks[MAX_BLADES - 1] & before);
            if (full != 0 || tail == MAX_BLADES) {
                for (uint i = 0; i < MAX_FULL_BATCHES; ++i) {
                    candidate.fullOffset += countbits(fullMasks[i] & before);
                }
            }
            if (tail != 0) {
                candidate.tailOffset = tail == MAX_BLADES
                    ? candidate.fullOffset + full : countbits(tailMasks[tail - 1] & before);
            }
        }
    }
    uint index = group * GROUP_WIDTH + lane;
    if (batches != 0) {
        candidates[index] = candidate;
    } else {
        candidates[index].fullOffset = REJECTED;
    }
}

groupshared uint scanSums[SCAN_WIDTH];

// Each workgroup scans one bucket (or one mesh counter) in source order.
[numthreads(SCAN_WIDTH, 1, 1)]
void grass_scan(uint channel : SV_GroupID, uint lane : SV_GroupIndex) {
    uint stepSize = (groupCount + SCAN_WIDTH - 1) / SCAN_WIDTH;
    uint start = lane * stepSize, end = min(start + stepSize, groupCount);
    uint base = channel * groupCount;
    uint sum = 0;
    for (uint i = start; i < end; ++i) sum += groupOffsets[base + i];
    scanSums[lane] = sum;
    GroupMemoryBarrierWithGroupSync();
    if (lane == 0) {
        uint prefix = 0;
        for (uint i = 0; i < SCAN_WIDTH; ++i) {
            uint count = scanSums[i];
            scanSums[i] = prefix;
            prefix += count;
        }
        drawData.Store(TOTALS_OFFSET + 4 * channel, prefix);
    }
    GroupMemoryBarrierWithGroupSync();
    sum = scanSums[lane];
    for (uint outputIndex = start; outputIndex < end; ++outputIndex) {
        uint count = groupOffsets[base + outputIndex];
        groupOffsets[base + outputIndex] = sum;
        sum += count;
    }
}

[numthreads(1, 1, 1)]
void grass_commands() {
    if (meshPath != 0) {
        uint count = min(drawData.Load(TOTALS_OFFSET), patchCapacity);
        drawData.Store2(0, uint2(count, drawData.Load(TOTALS_OFFSET + 4)));
        drawData.Store3(ARGS_OFFSET, uint3(min(count, MESH_GROUPS_X),
            (count + MESH_GROUPS_X - 1) / MESH_GROUPS_X, 1));
        return;
    }
    uint base = 0, blades = 0;
    for (uint bucket = 0; bucket < MAX_BLADES; ++bucket) {
        uint count = min(drawData.Load(TOTALS_OFFSET + 4 * bucket), patchCapacity - base);
        uint offset = ARGS_OFFSET + INDEXED_STRIDE * bucket;
        drawData.Store(BUCKETS_OFFSET + 4 * bucket, base);
        drawData.Store4(offset, uint4(base, (bucket + 1) * TRIANGLES_PER_BLADE * 3, count, 0));
        drawData.Store2(offset + 16, uint2(0, 0));
        base += count;
        blades += count * (bucket + 1);
    }
    drawData.Store2(0, uint2(base, blades));
}

void emitBatch(Patch patch, uint offset, uint batch, float blades) {
    if (offset >= patchCapacity) return;
    patch.bladeCount = blades;
    patch.seed += batch * 0x9e3779b9u;
    visiblePatches[offset] = patch;
}

[numthreads(GROUP_WIDTH, 1, 1)]
void grass_scatter(uint group : SV_GroupID, uint lane : SV_GroupIndex) {
    uint index = group * GROUP_WIDTH + lane;
    uint local = candidates[index].fullOffset;
    if (local == REJECTED) return;
    Candidate candidate = candidates[index];
    uint full = uint(candidate.patch.bladeCount) / MAX_BLADES;
    float tail = candidate.patch.bladeCount - full * MAX_BLADES;
    uint fullBase = groupOffsets[group] + local;
    uint tailIndex = fullBase + full;
    if (meshPath == 0) {
        uint fullBucket = MAX_BLADES - 1;
        if (full != 0) {
            fullBase = drawData.Load(BUCKETS_OFFSET + 4 * fullBucket)
                + groupOffsets[fullBucket * groupCount + group] + local;
        }
        if (tail > 0.0) {
            uint bucket = uint(ceil(tail)) - 1;
            tailIndex = drawData.Load(BUCKETS_OFFSET + 4 * bucket)
                + groupOffsets[bucket * groupCount + group] + candidate.tailOffset;
        }
    }
    for (uint i = 0; i < full; ++i) emitBatch(candidate.patch, fullBase + i, i, MAX_BLADES);
    if (tail > 0.0) emitBatch(candidate.patch, tailIndex, full, tail);
}
