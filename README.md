# AI generated Rust based DX12 engine

The default scene opens on a hillside above a winding alpine lake. It combines a 5 km × 5 km adaptive basin, distant mountain ranges, depth-colored water with planar reflections, and instanced conifer stands. Close trees use branch meshes and distant trees use camera-facing silhouettes.

Cliffs use triplanar limestone and shale with filtered detail normals. Sheltered gullies retain snow and loose scree collects below steep rock. Terrain textures use full mip chains and 16× anisotropic filtering. Distant stands blend into canopy cover as individual crowns shrink below a few pixels.

Run `cargo run --release --locked` for the alpine overlook. Use `--scene meadow` for the original cube and grass demo. To capture the same alpine viewpoint and animation time, run `cargo run --release --locked -- --time 0 --capture target/alpine-overlook.ppm`.

Press F5 and open **Clouds** to change sky resolution, ray samples, lighting samples, and alpine cloud shadow samples while the scene runs. Set either lighting or cloud shadow samples to zero to skip that work.

- PBR Rendering
- HDR Rendering
- Deferred Rendering
- GPU terrain
- GPU grass
- Four cascaded alpine shadow maps for terrain and conifers
- Rayleigh scattering and altitude-dependent valley haze
- Raymarched clouds with moving shadows on land and water
- ImGUI integration for debug operations

<img width="958" height="549" alt="image" src="https://github.com/user-attachments/assets/2bee0193-34bc-4738-ab9b-cc9dd2b0e03e" />
<img width="960" height="545" alt="image" src="https://github.com/user-attachments/assets/8cfd1ece-3d0e-490c-9a49-a1f9191ff93d" />
<img width="956" height="544" alt="image" src="https://github.com/user-attachments/assets/23da88df-2011-4b72-a015-dfa2f6991fa0" />



