# Procedural grass for Rustgine

The default scene combines the valley, the floating cube and wind-animated grass. Patch seeds are tied to world-grid cells. Each blade samples the triangle drawn at its root for the current terrain LOD. Moving the camera changes which nearby cells are drawn.

## Implemented and verified

Grass is enabled at launch. Space pauses the cube and wind together. `--grass-path auto|mesh|instanced|off` selects a rendering path. `--grass-full-density` keeps all 32 blades in each visible patch for comparison. `--time` fixes the simulation time for a capture, and the capture log reports patch count, triangle count and scene GPU timings.

Insert and Delete adjust the grass draw distance from 5 m to 1 km. The default is 1 km. Steps are 4 m up to 45 m and grow beyond that. Grass patches become sparser with distance and fade over the last 3 m of the selected range.

Adjacent grass grids blend over 90–96 m, 200–224 m, and 560–640 m from the camera.

```powershell
cargo run --locked -- --grass-path mesh --time 0 --capture target/grass-mesh.ppm
cargo run --locked -- --grass-path instanced --time 0 --capture target/grass-instanced.ppm
cargo run --locked -- --warp --grass-path instanced --time 0 --capture target/grass-warp.ppm
cargo test --locked --test desktop -- --ignored --nocapture --test-threads=1
```

At 1280 × 720 on the RTX 3080, the two hardware paths differed in three pixels out of 921,600 at the same seed and pose. The live window selected mesh shaders and HDR scRGB. Debug and release desktop tests passed, including repeatable WARP captures, visible wind motion, and window resize, minimize and restore. WARP on this machine also supports mesh shaders, so the fallback test explicitly selects instancing.

| Configuration | Visible patches | Grass triangles | Scene GPU time |
| --- | ---: | ---: | ---: |
| Grass off | 0 | 0 | 0.034 ms |
| Mesh shaders, full density | 776 | 148,992 | 0.063 ms |
| Mesh shaders, distance LOD | 776 | 57,354 | 0.054 ms |

GPU times are single-frame capture samples. The renderer uses a gradual height and width fade near the meadow boundary to blend the blades into the floor. PIX inspection and a moving-camera LOD sweep remain open checks.

## How AMD's method fits this renderer

AMD generates one patch per mesh-shader workgroup. A blade is a tapered ribbon following a quadratic Bézier curve. Four samples along each edge produce eight vertices and six triangles. At 32 blades, a group emits at most 256 vertices and 192 triangles. AMD reduces the blade count with distance, widens the survivors to maintain coverage, and scales the final blade continuously across count transitions. Wind bends the tip while an approximate arc-length correction keeps the blade from stretching. [AMD GPUOpen, *Procedural grass rendering*](https://gpuopen.com/learn/mesh_shaders/mesh_shaders-procedural_grass_rendering/)

Both grass backends check the terrain blend at each blade root. A blade is drawn only when meadow textures make up more than half of that blend. Eligible blades write their materials and two-sided normals into the G-buffer after the terrain and cube. Deferred lighting applies directional light, shadow visibility and Rayleigh aerial perspective before the SDR or scRGB output transform. Root darkening is part of the blade albedo.

## Original grass prototype sequence

### 1. Draw one patch on hardware

- In `src/display.rs`, keep the Shader Model 6.0 device check. Separately probe `D3D12_FEATURE_D3D12_OPTIONS7` for mesh shader tier 1 and query shader model support with 6.5 as the requested maximum. Select instancing when either capability is unavailable. Default to automatic selection and expose `--grass-path auto|mesh|instanced|off` for comparisons. Log the active path and report an unsupported forced mesh request explicitly.
- Extend `build.rs` to compile a grass mesh shader at `ms_6_5`, a fallback vertex shader at `vs_6_0`, and a shared pixel shader at `ps_6_0`. Track the new HLSL sources and includes for Cargo rebuilds.
- In `src/pipeline.rs`, create the mesh pipeline with a pipeline-state stream and `ID3D12Device2::CreatePipelineState`. Use two-sided rasterization, depth testing and writes, and the existing scene and depth formats. In `src/renderer.rs`, use `ID3D12GraphicsCommandList6::DispatchMesh(1, 1, 1)` for the first patch. Create this pipeline only on a capable device. [Microsoft DirectX mesh shader specification](https://microsoft.github.io/DirectX-Specs/d3d/MeshShader.html)

**Checkpoint.** A stationary patch appears beside the cube. Inspect it in PIX with the D3D12 debug layer enabled. A full patch should report 256 vertices and 192 triangles, and the cube should occlude blades behind it.

### 2. Generate a stable meadow on both paths

- Add `src/grass.rs` for patch placement and visibility. Start with a bounded grid on the XZ floor. Give each cell a permanent seed, center, radius and mean blade height. Exclude the clearing, accounting for the scattering radius at the border. Cull patch bounds conservatively enough to contain the bent tips.
- Put the blade evaluator in a shared HLSL include. Hash the cell seed with the blade index for a repeatable root, heading and height. Scatter roots uniformly over each disk using a square-root radial distribution. Use X and Z as the tangent basis for the flat Y-up floor. Derive the surface normal from the curve tangent and ribbon width direction.
- Upload visible patch records to a structured buffer for the current `FrameSlot`, after its fence completes. Add a root SRV at `t2` visible to the generating stages. Keep seeds independent of upload order so camera culling cannot rearrange the grass.
- On mesh hardware, dispatch one 128-thread group per visible patch. Set the actual output counts once before writing, then let each lane write additional vertices and triangles at a stride of 128. On the SM 6.0 path, use one reusable 16-bit index pattern for a patch. Each instance selects a patch, and `SV_VertexID` selects the blade and ribbon sample. Both shaders call the same evaluator.

**Checkpoint.** At a fixed time and seed, forced instancing and mesh dispatch on the same GPU show matching roots, silhouettes and patch boundaries. A WARP capture contains visible grass and the checkered clearing.

### 3. Add wind, distance LOD and lighting

- In `src/main.rs`, drive wind from elapsed simulation time. The cube angle wraps every revolution and would make the wind jump if reused as a clock. Pass time through the Rust `Frame` and matching HLSL `b0` layout. Space should freeze both animations, and `--time` should reproduce both.
- Offset the tip in XZ with traveling sine waves whose phase varies with world-space noise. Anchor the root and rescale the two control-point segments to the target length using AMD's chord-and-control-polygon estimate. Shade both faces, soften normals toward +Y, darken roots and vary green over broad areas. Feed these linear material values into the forward lighting pass. [AMD's wind and shading discussion](https://gpuopen.com/learn/mesh_shaders/mesh_shaders-procedural_grass_rendering/#wind-animation)
- Compute one fractional count `b` per visible patch, from 32 near the camera to 2 at the far end of the meadow. Emit `ceil(b)` blades. For zero-based blade `i`, multiply width by `(32 / b) * clamp(b - i, 0, 1)`. This keeps the final blade full-width at an exact integer. For instancing, group records by integer count and draw the first `18 * ceil(b)` indices per instance so distant patches also cost fewer triangles on that path.

**Checkpoint.** Sweep a test camera across an integer LOD boundary with wind frozen. Surviving roots should stay fixed and the disappearing blade should narrow continuously. With the camera fixed and wind running, roots should stay planted as tips move.

## Verification and handoff

| Check | Evidence to collect |
| --- | --- |
| Determinism and motion | Extend `tests/desktop.rs` to capture the same seed and time twice on WARP, then a later time. Compare a grass-only region away from the cube and its shadow. Check repeatability first, then motion. Keep separate checks for the cube and exposed tiles. |
| PBR, shadows and HDR | Inspect a fixed-time SDR capture and a live HDR frame. In PIX, follow one lit blade and one inside the cube's shadow through the FP16 scene target and final output. Verify the scene still carries highlights above 1.0 before tone mapping. |
| Cost and stability | At 1280 × 720 on the RTX 3080, record visible patches, emitted triangles and GPU time for grass disabled, full density and distance LOD at the same camera pose. Repeat the existing resize, minimize and restore test with grass enabled. |

Start with the single-patch checkpoint. It isolates capability detection, pipeline setup, winding and depth before density or wind complicates the image. The existing engine plan contains the local toolchain setup and desktop test commands. [Engine build and verification instructions](ENGINE_PLAN.md)
