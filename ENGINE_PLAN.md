# Rustgine · Direct3D 12 engine plan

Launch `target/release/rustgine.exe` for the alpine lake overlook. Use `--scene meadow` to explore the original 5 km � 5 km valley. A teal cube spins with its bottom face two metres above the ground at its centre. Wind-animated grass grows on the active terrain triangles around the camera. The scene uses linear floating-point lighting and the display�s SDR or HDR output transform.

The demo now includes procedural grass. The feature plan records how to select the render path, reproduce the captures, and verify the meadow. [Procedural grass implementation and verification](GRASS_RENDERING_PLAN.md)

## Build and run

Use x64 Windows, the MSVC Rust toolchain, Visual Studio's C++ build tools, and a Windows SDK with the DirectX Shader Compiler. The renderer needs a Direct3D 12 device with Shader Model 6.0. Hardware is preferred and WARP is available for software rendering. Cargo's build script finds `dxc.exe` on PATH or in the Windows SDK. Set `DXC_PATH` to override detection. The compiled shaders are embedded in the executable. [Rust for Windows](https://github.com/microsoft/windows-rs) · [DirectX Shader Compiler](https://github.com/microsoft/DirectXShaderCompiler)

In this checkout, Rust is installed under the ignored `.tooling` directory. In a new PowerShell session, run the following from the project root if Cargo is unavailable on PATH.

```powershell
$env:CARGO_HOME = (Resolve-Path .tooling/cargo).Path
$env:RUSTUP_HOME = (Resolve-Path .tooling/rustup).Path
$env:PATH = (Join-Path $env:CARGO_HOME 'bin') + [IO.Path]::PathSeparator + $env:PATH
```

```powershell
cargo build --release --locked
.\target\release\rustgine.exe
```

| Action | Control |
| --- | --- |
| Pause or resume the cube and wind | Space |
| Adjust exposure in quarter-stop increments | + or − |
| Fly forward, left, backward, or right | W, A, S, D |
| Double flight speed while held | Shift |
| Cycle Normal, Wireframe Overlay, and Wireframe Only | F1 |
| Start or pause the simulated clock at two minutes per second | F3 |
| Advance the time of day by 15 minutes | F4 |
| Look around and aim the direction of flight | Click the scene, then move the mouse |
| Release the mouse or close the window when released | Escape |
| Exercise SDR presentation on an HDR desktop | `cargo run --locked -- --sdr` |
| Use the software adapter | `cargo run --locked -- --warp` |
| Disable grass in the shared scene | `cargo run --locked -- --grass-path off` |
| Fly over the 5 km × 5 km terrain | `cargo run --release --locked -- --terrain` |
| Save a repeatable SDR preview and measure scene-linear highlights | `cargo run --locked -- --scene meadow --time 0 --capture target/reference.ppm` |

## Terrain scene

The engine generates a deterministic 513 × 513 heightfield with about 9.77 m between samples. The GPU updates a bounded pool of active longest-edge bisectors using a concurrent binary tree, then writes one index buffer for the color, shadow, depth and wireframe passes. Once the GPU confirms that no splits or merges occurred, the renderer reuses the mesh until the camera position or projected pixel scale changes. The GPU resolves T-junctions by interpolating fine vertices along the coarsest active edge that crosses them. The color pass, shadow map, wireframe and grass use the same resolved terrain geometry. [Benyoub and Dupuy, HPG 2024](https://arxiv.org/abs/2407.02215) · [Lee, 2026](https://www.mdpi.com/3042-6308/2/3/12)

The fly camera starts beside the cube and moves at 50 km/h. Hold Shift to fly at 100 km/h. Add `--capture target/terrain.ppm` to save the initial view after refinement and report the active triangle count and GPU time for terrain preparation, shadows, opaque geometry, grass geometry, deferred lighting and the output transform. Capture timings exclude presentation and image readback.

## Implementation milestones

| Milestone | Implementation | Acceptance check |
| --- | --- | --- |
| 1. Window and device | Win32 window, adapter selection, direct queue, two flip-discard buffers and a shared fence with a completion value for each frame slot. | Open, resize, minimize, restore, and close the window with D3D12 validation enabled. |
| 2. First scene | Procedural beveled cube and plane, perspective camera, elapsed-time Y rotation, and a world-space checker filtered over the pixel footprint. | Capture two poses. The cube silhouette and its shadow move while the floor grid stays fixed. |
| 3. Directional PBR | A metallic-roughness BRDF uses GGX, height-correlated Smith visibility, Schlick Fresnel and diffuse reflection. A 2048² depth map supplies filtered directional shadows. Environment fill and reflections light the unlit faces. | At fixed exposure, the moving bevel carries a highlight, and the floor remains visibly rougher than the cube. [Filament's BRDF reference](https://google.github.io/filament/main/filament.html) |
| 4. HDR lighting and presentation | Both output modes share an `R16G16B16A16_FLOAT` scene target. SDR uses a filmic curve and one sRGB encoding into an 8-bit UNORM back buffer. HDR uses an FP16 scRGB back buffer, 160-nit reference white and a shoulder based on the display's peak. | Confirm scene values above 1.0, run the SDR path, and confirm HDR scRGB selection with Windows HDR enabled. [Windows Advanced Color guidance](https://learn.microsoft.com/en-us/windows/win32/direct3darticles/high-dynamic-range) |

## Code map and frame lifetime

| File | Responsibility |
| --- | --- |
| `src/main.rs`, `src/window_input.rs` | Window messages, DPI, keyboard and raw mouse input, animation clock, minimization, and command-line options. |
| `src/camera.rs` | Fly-camera movement, mouse look and the view matrix. |
| `src/display.rs` | Hardware and WARP selection, monitor HDR detection, and DXGI color-space negotiation. |
| `src/scene.rs` | CPU geometry, camera, light transforms, and the Rust layout of the frame constants. |
| `src/gpu.rs` | Resources, descriptor handles, barriers, upload copies, and GPU fence waits. |
| `src/renderer.rs` | Frame slots, size-dependent targets, swap-chain reconfiguration, and pass submission. |
| `src/pipeline.rs`, `shaders/scene.hlsl`, `build.rs` | Root signature and pipeline states, lighting and output shaders, and build-time shader compilation. |
| `src/capture.rs`, `tests/desktop.rs` | Pitched GPU readback, RGB preview output, scene peak measurement, and desktop regression tests. |

1. Acquire the current swap-chain index. Wait for that slot's fence value before resetting its allocator or writing its constant buffer.
2. Draw the terrain and cube from the light into the shadow map. Transition the map from depth-write to pixel-shader input.
3. Draw the terrain, cube and grass into two G-buffer targets with camera depth testing. One target stores sRGB albedo and metallic fraction. The other stores the octahedral world normal, roughness and a material class. The class distinguishes uncovered pixels, standard lighting and matte ground lighting.
4. Reconstruct world positions from camera depth and evaluate directional lighting, filtered shadows, environment light and atmospheric scattering in a full-screen pass into the FP16 scene target. Single Rayleigh scattering integrates wavelength-dependent sunlight through an exponential atmosphere. Uncovered pixels sample the sky, and covered pixels integrate only as far as the visible surface. Draw wireframe overlays against the geometry depth. Wireframe-only mode uses a depth prepass and skips lighting.
5. Sample scene color in a full-screen triangle. Apply exposure, then the selected display transform. Return the back buffer to the present state, present it, and signal the slot's fence.

All passes share one ordered direct queue. Resize drains outstanding work, releases back-buffer references, and rebuilds the views and size-dependent targets. The app rechecks the active monitor once per second. On HDR output it writes linear scRGB, where 1.0 corresponds to 80 nits. If the requested color space cannot be presented, it reconfigures the chain for SDR. [D3D12 initialization and frame lifecycle](https://learn.microsoft.com/windows/win32/direct3d12/creating-a-basic-direct3d-12-component) · [DXGI display encoding](https://learn.microsoft.com/en-us/windows/win32/direct3darticles/high-dynamic-range)

## Verification baseline

Run the desktop tests from an interactive Windows session. One opens and closes a window. The other uses WARP to compare GPU captures at two timestamps, check the cube color and tile contrast, and detect clipping in the lighting buffer.

```powershell
cargo fmt --all -- --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked --test desktop -- --ignored --nocapture --test-threads=1
cargo test --release --locked --test desktop -- --ignored --nocapture --test-threads=1
```

Verified in this workspace on November 18, 2026, with Rust 1.100.0, Visual Studio 2022, and the Windows SDK shader compiler.

- Debug and release builds passed both desktop tests. The debug layer reported no warnings or errors in the final hardware capture or native-window run.
- On the GeForce RTX 3080, the default-pose capture measured a peak RGB component of **24.625** with **595 pixels above 1.0** before tone mapping. This is scene-relative radiance. The PPM records the SDR result.
- The normal application selected **HDR scRGB** on the connected display. DXGI reported approximately **604 nits** peak luminance. SDR capture and WARP also rendered successfully. Physical luminance calibration and moving the running window between HDR and SDR monitors remain open checks.

## Next implementation steps

| Order | Work | Acceptance check |
| --- | --- | --- |
| 1 | Introduce a Rust `Material` and a draw item with a mesh range, transform and material index. Move base color, metallic and roughness out of the HLSL floor/cube branch. Keep the checker as a floor material option. Feed the same draw transforms into the shadow and color passes. | The existing capture tests pass. Two instances of one mesh can have different roughness, transforms, and matching shadows. |
| 2 | Add a display regression run on a machine with HDR and SDR monitors. Move the open window in both directions and toggle Windows HDR. Inspect the output in PIX at a fixed exposure. | The log records the expected mode each time. The scene target retains values above 1.0, the SDR pass encodes once, and the HDR back buffer retains highlights above diffuse white. |
| 3 | Extend the material path with an environment cubemap, diffuse irradiance, prefiltered specular mip levels and a BRDF lookup texture. Use a small roughness sweep as the comparison scene. | Metallic objects reflect the environment, highlights broaden with roughness, and the default cube and floor continue to render through both display paths. |
