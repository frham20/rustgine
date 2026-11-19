#include "terrain_common.hlsli"

// The tree is bound as a read-only snapshot during evaluate/apply and as a
// UAV during reduction. The mutable bitmap is independent of that snapshot.
StructuredBuffer<uint> previousTree : register(t6);
RWStructuredBuffer<uint> pool : register(u0);
RWStructuredBuffer<uint> leafSlots : register(u1); // Slot plus one, zero if absent.
RWStructuredBuffer<uint4> commands : register(u2);
RWStructuredBuffer<uint> occupancy : register(u3); // Bitmap followed by per-group split counts / offsets.
RWStructuredBuffer<uint> sumTree : register(u4);
RWByteAddressBuffer arguments : register(u5);

static const uint KEEP = 0, SPLIT = 1, MERGE = 2;
static const uint UPDATE_THREADS = 128;
static const uint REDUCE_THREADS = 64;
static const float SPLIT_PIXELS = 6.0, MERGE_PIXELS = 3.9;

uint stableCountIndex() { return bitmapWords() + terrainGrid.w / UPDATE_THREADS; }

[numthreads(UPDATE_THREADS, 1, 1)]
void terrain_init(uint id : SV_DispatchThreadID) {
    uint roots = terrainGrid.y, stride = terrainGrid.z;
    if (id < roots * stride) {
        uint root = id / stride;
        leafSlots[id] = id % stride == 1 ? root + 1 : 0;
    }
    if (id < roots) {
        pool[id] = id * stride + 1;
        pool[terrainGrid.w + id] = id;
    }
    if (id < bitmapWords()) occupancy[id] = id < roots / 32 ? 0xffffffff : 0;
    if (id == 0) occupancy[stableCountIndex()] = 0;
}

float projectedError(TerrainNode node) {
    uint2 pa = gridPosition(node.corners.x), pb = gridPosition(node.corners.y), pc = gridPosition(node.corners.z);
    float3 a = terrainPosition(pa, terrainSamples[gridIndex(pa)].x);
    float3 b = terrainPosition(pb, terrainSamples[gridIndex(pb)].x);
    float3 c = terrainPosition(pc, terrainSamples[gridIndex(pc)].x);
    float3 view = camera.xyz - (a + b + c) / 3;
    float distance = length(view);
    float3 normal = normalize(cross(b - a, c - a));
    float incidence = max(0.42, abs(dot(normal, view)) / max(distance, 1.0));
    return terrainParams.z * node.error / (max(distance, 15.0) * incidence);
}

uint3 classify(uint slot) {
    uint nodeId = pool[slot];
    uint index = nodeId % terrainGrid.z;
    uint action = KEEP, sibling = 0;
    float error = projectedError(terrainNodes[nodeId]);
    if (index < terrainGrid.z / 2 && error > SPLIT_PIXELS) {
        action = SPLIT;
    } else if (index > 1 && error < MERGE_PIXELS) {
        sibling = leafSlots[nodeId ^ 1];
        uint parent = nodeId - index + index / 2;
        if (sibling != 0 && projectedError(terrainNodes[parent]) < MERGE_PIXELS) action = MERGE;
    }
    return uint3(nodeId, action, sibling);
}

groupshared uint splitMasks[UPDATE_THREADS / 32];

[numthreads(UPDATE_THREADS, 1, 1)]
void terrain_evaluate(uint rank : SV_DispatchThreadID, uint lane : SV_GroupIndex, uint3 group : SV_GroupID) {
    if (lane < UPDATE_THREADS / 32) splitMasks[lane] = 0;
    GroupMemoryBarrierWithGroupSync();
    uint slot = 0;
    uint3 command = 0;
    if (rank < previousTree[1]) {
        slot = pool[terrainGrid.w + rank];
        command = classify(slot);
        if (command.y == SPLIT) InterlockedOr(splitMasks[lane / 32], 1u << (lane % 32));
    }
    GroupMemoryBarrierWithGroupSync();
    uint prefix = 0;
    for (uint word = 0; word < lane / 32; ++word) prefix += countbits(splitMasks[word]);
    prefix += countbits(splitMasks[lane / 32] & ((1u << (lane % 32)) - 1));
    if (rank < previousTree[1]) commands[slot] = uint4(command, prefix);
    if (lane == UPDATE_THREADS - 1) occupancy[bitmapWords() + group.x] = prefix + uint(command.y == SPLIT);
}

groupshared uint blockTotals[UPDATE_THREADS];

// Assign free-slot ranks in draw order. Atomic allocation order otherwise
// changes depth ties along shared edges between otherwise identical captures.
[numthreads(UPDATE_THREADS, 1, 1)]
void terrain_split_offsets(uint lane : SV_GroupIndex) {
    uint groups = (previousTree[1] + UPDATE_THREADS - 1) / UPDATE_THREADS;
    uint perLane = (groups + UPDATE_THREADS - 1) / UPDATE_THREADS;
    uint first = lane * perLane, end = min(first + perLane, groups);
    uint base = bitmapWords();
    uint sum = 0;
    for (uint i = first; i < end; ++i) sum += occupancy[base + i];
    blockTotals[lane] = sum;
    GroupMemoryBarrierWithGroupSync();
    uint offset = 0;
    for (uint j = 0; j < lane; ++j) offset += blockTotals[j];
    for (uint groupIndex = first; groupIndex < end; ++groupIndex) {
        uint count = occupancy[base + groupIndex];
        occupancy[base + groupIndex] = offset;
        offset += count;
    }
    if (lane == UPDATE_THREADS - 1) {
        uint active = previousTree[1];
        // With no allocatable splits, an unchanged count proves no merge was applied either.
        occupancy[stableCountIndex()] = offset == 0 || active == terrainGrid.w ? active : 0;
    }
}

[numthreads(UPDATE_THREADS, 1, 1)]
void terrain_apply(uint rank : SV_DispatchThreadID) {
    uint active = previousTree[1];
    if (rank >= active) return;
    uint slot = pool[terrainGrid.w + rank];
    uint4 command = commands[slot];
    uint node = command.x, action = command.y;
    uint index = node % terrainGrid.z;
    if (action == SPLIT) {
        uint allocation = occupancy[bitmapWords() + rank / UPDATE_THREADS] + command.w;
        // Old free slots are disjoint from all dispatched blocks, including
        // blocks retired by merges in this dispatch.
        if (allocation >= terrainGrid.w - active) return;
        uint freeSlot = rankedSlot(previousTree, allocation, false);
        uint child = node + index;
        pool[slot] = child;
        pool[freeSlot] = child + 1;
        leafSlots[node] = 0;
        leafSlots[child] = slot + 1;
        leafSlots[child + 1] = freeSlot + 1;
        InterlockedOr(occupancy[freeSlot / 32], 1u << (freeSlot % 32));
    } else if (action == MERGE) {
        if (commands[command.z - 1].y != MERGE) return;
        leafSlots[node] = 0;
        if (index % 2 == 0) {
            uint parent = node - index + index / 2;
            pool[slot] = parent;
            leafSlots[parent] = slot + 1;
        } else {
            InterlockedAnd(occupancy[slot / 32], ~(1u << (slot % 32)));
        }
    }
}

groupshared uint counts[REDUCE_THREADS];

void reduceSubtree(uint base, uint lane) {
    GroupMemoryBarrierWithGroupSync();
    [unroll] for (uint step = 1; step < REDUCE_THREADS; step *= 2) {
        base /= 2;
        if (lane % (step * 2) == 0) {
            counts[lane] += counts[lane + step];
            sumTree[base + lane / (step * 2)] = counts[lane];
        }
        GroupMemoryBarrierWithGroupSync();
    }
}

[numthreads(REDUCE_THREADS, 1, 1)]
void terrain_reduce_blocks(uint id : SV_DispatchThreadID, uint lane : SV_GroupIndex) {
    uint words = bitmapWords();
    uint bits = occupancy[id];
    uint count = countbits(bits);
    sumTree[words * 2 + id] = bits;
    sumTree[words + id] = count;
    counts[lane] = count;
    reduceSubtree(words + id - lane, lane);
}

[numthreads(REDUCE_THREADS, 1, 1)]
void terrain_reduce_root(uint lane : SV_GroupIndex) {
    counts[lane] = sumTree[REDUCE_THREADS + lane];
    reduceSubtree(REDUCE_THREADS, lane);
    if (lane == 0) {
        uint active = counts[0];
        arguments.Store3(0, uint3((active + UPDATE_THREADS - 1) / UPDATE_THREADS, 1, 1));
        arguments.Store4(12, uint4(active * 3, 1, 0, 0));
        arguments.Store(28, 0); // StartInstanceLocation of the indexed draw.
        arguments.Store(32, uint(active == occupancy[stableCountIndex()]));
    }
}
