Texture2DArray<float4> terrainTiles : register(t12);
Texture2DArray<float2> rockSlopes : register(t17);
SamplerState terrainSampler : register(s1);

float3 triplanarRock(uint layer, float repeat, float3 p, float3 dx, float3 dy, float3 weights) {
    p /= repeat;
    dx /= repeat;
    dy /= repeat;
    return weights.x * terrainTiles.SampleGrad(terrainSampler, float3(p.zy, layer), dx.zy, dy.zy).rgb
         + weights.y * terrainTiles.SampleGrad(terrainSampler, float3(p.xz, layer), dx.xz, dy.xz).rgb
         + weights.z * terrainTiles.SampleGrad(terrainSampler, float3(p.xy, layer), dx.xy, dy.xy).rgb;
}

float3 triplanarSlope(uint layer, float repeat, float3 p, float3 dx, float3 dy, float3 w) {
    p /= repeat;
    dx /= repeat;
    dy /= repeat;
    float2 x = rockSlopes.SampleGrad(terrainSampler, float3(p.zy, layer), dx.zy, dy.zy);
    float2 y = rockSlopes.SampleGrad(terrainSampler, float3(p.xz, layer), dx.xz, dy.xz);
    float2 z = rockSlopes.SampleGrad(terrainSampler, float3(p.xy, layer), dx.xy, dy.xy);
    return w.x * float3(0,x.y,x.x) + w.y * float3(y.x,0,y.y) + w.z * float3(z.x,z.y,0);
}

// A value and its analytic world-space gradient. Carrying both through the
// warped bedding avoids screen-quad steps and keeps highlights fixed in space.
float4 geologyNoise(float4 u, float4 v, float3 dx, float3 dy) {
    float2 cell = floor(float2(u.w,v.w));
    float2 t = frac(float2(u.w,v.w)), f = t*t*(3-2*t);
    float a = terrainHash(cell), b = terrainHash(cell + float2(1,0));
    float c = terrainHash(cell + float2(0,1)), d = terrainHash(cell + 1);
    float2 slope = 6*t*(1-t) * float2(lerp(b-a,d-c,f.y),lerp(c-a,d-b,f.x));
    float2 duvDx = float2(dot(u.xyz, dx), dot(v.xyz, dx));
    float2 duvDy = float2(dot(u.xyz, dy), dot(v.xyz, dy));
    float weight = noiseWeight(max(length(duvDx), length(duvDy)));
    return float4(weight * (slope.x*u.xyz + slope.y*v.xyz),
                  lerp(0.5, lerp(lerp(a,b,f.x),lerp(c,d,f.x),f.y), weight));
}

struct AlpineSurface {
    float3 color;
    float3 normal;
    float roughness;
};

AlpineSurface alpineSurface(float3 p, float3 n, float3 groundColor, float groundRoughness, float rock, float viewportScale) {
    float3 dx = ddx(p), dy = ddy(p);
    float footprint = max(length(dx), length(dy));
    float3 w = pow(abs(n), 4);
    w /= dot(w, 1.0.xxx);
    float altitude = p.y - lakeLevel;
    float timberline = smoothstep(365, 575, altitude);
    float slope = length(n.xz);
    float4 x = float4(1,0,0,p.x), y = float4(0,1,0,p.y), z = float4(0,0,1,p.z);
    float4 massField = geologyNoise(x * 0.0043, z * 0.0043, dx, dy);
    float mass = massField.w;
    float cliff = max(rock * 0.8, smoothstep(0.51, 0.79, slope + 0.27 * timberline + 0.07 * (mass - 0.5)));
    float3 land = alpineLandforms(p.xz, dx.xz, dy.xz);
    float2 deposit = alpineDeposits(p, n, land.rg, max(length(dx.xz),length(dy.xz)));

    // Tilted bedding is visible at valley scale. A second, oblique family of
    // joints breaks it into blocks. Remove subpixel bands before differencing.
    float4 strike = 0.83*x - 0.56*z;
    float4 bed = 0.44*x + y - 0.26*z + 85 * (massField - float4(0,0,0,0.5));
    float4 foldField = geologyNoise(strike * 0.014, bed * 0.014, dx, dy);
    float fold = foldField.w;
    float4 beds = geologyNoise(strike * 0.012 + 1.3*foldField, bed * 0.051 + 2.6*foldField, dx, dy);
    float4 joint = geologyNoise((strike + 0.34*y)*0.035 + foldField*1.9, (bed - 0.15*strike)*0.009 + massField*1.9, dx, dy);
    float edge = saturate((abs(joint.w - 0.48) - 0.018) / 0.112);
    float jointWidth = abs(dot(joint.xyz,dx)) + abs(dot(joint.xyz,dy));
    float broken = (1 - smoothstep(0.045, 0.19, jointWidth))
                 * (1 - smoothstep(5, 26, footprint)) * smoothstep(0.18, 0.70, fold);
    float joints = (1 - edge*edge*(3-2*edge)) * broken;
    float3 limestone = triplanarRock(12, 8, p, dx, dy, w);
    float3 shale = triplanarRock(13, 15, p, dx, dy, w);
    // Metre-scale grain disappears naturally in the mips. Broader fractured
    // slabs retain relief on the same faces when seen from across the lake.
    float3 macro = p + float3(24 * fold, 17 * mass, 0);
    float3 macroDx = dx + float3(24 * dot(foldField.xyz,dx), 17 * dot(massField.xyz,dx), 0);
    float3 macroDy = dy + float3(24 * dot(foldField.xyz,dy), 17 * dot(massField.xyz,dy), 0);
    float3 blocks = triplanarRock(13, 185, macro, macroDx, macroDy, w);
    float relief = dot(blocks, float3(0.22, 0.70, 0.08));
    float lithology = smoothstep(0.22, 0.82, 0.48 * beds.w + 0.32 * mass + 0.20 * fold);
    float3 stone = lerp(shale * float3(0.93,0.95,0.98), limestone * 1.22, lithology);
    stone *= (0.69 + 2.5 * relief + 0.26 * fold) * (1 - 0.29 * joints);

    float3 gravel = terrainTiles.SampleGrad(terrainSampler, float3(p.xz / 3.2, 15), dx.xz / 3.2, dy.xz / 3.2).rgb;
    float fan = geologyNoise(strike * 0.022, bed * 0.006, dx, dy).w;
    gravel *= lerp(float3(0.75,0.77,0.80), float3(1.18,1.11,0.98), fan);
    float3 snow = lerp(float3(0.69,0.76,0.82), float3(0.90,0.92,0.92), 0.65 * fold + 0.35 * mass);
    AlpineSurface s;
    s.color = lerp(lerp(lerp(groundColor, stone, cliff), gravel, deposit.x), snow, deposit.y);
    s.roughness = lerp(lerp(lerp(groundRoughness, 0.78, cliff), 0.94, deposit.x), 0.72, deposit.y);

    float3 rockGradient = 1.9 * beds.xyz + 5.0 * foldField.xyz
        + 2.7 * broken * sign(joint.w - 0.48) * (6*edge*(1-edge) / 0.112) * joint.xyz;
    rockGradient += triplanarSlope(1, 185, macro, macroDx, macroDy, w);
    rockGradient += 0.75 * lerp(triplanarSlope(1,15,p,dx,dy,w), triplanarSlope(0,8,p,dx,dy,w), lithology);
    float2 grit = rockSlopes.SampleGrad(terrainSampler, float3(p.xz / 3.2,2), dx.xz / 3.2, dy.xz / 3.2);
    float3 gradient = lerp(cliff * rockGradient, float3(grit.x,0,grit.y), deposit.x) * (1 - deposit.y);
    gradient -= n * dot(n,gradient);
    gradient /= max(1.0, length(gradient) / 1.35);
    s.normal = normalize(n - gradient);
    // Replace the mottled gaps between subpixel crowns with mean stand cover.
    float3 eye = viewportScale < 1 ? mirrorPoint(camera.xyz) : camera.xyz;
    float crownPixels = 6.5 * terrainParams.z * viewportScale / max(1, distance(eye,p));
    float canopy = land.b * canopyBlend(crownPixels) * (1 - deposit.y);
    s.color = lerp(s.color, CANOPY_COLOR, canopy * 0.90);
    s.normal = normalize(lerp(s.normal, float3(0,1,0), canopy * 0.65));
    return s;
}
