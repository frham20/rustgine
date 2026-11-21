# AI generated Rust based DX12 engine

The default scene opens on a hillside above a winding alpine lake. It combines a 5 km × 5 km adaptive basin, distant mountain ranges, depth-colored water with planar reflections, and instanced conifer stands. Close trees use branch meshes and distant trees use camera-facing silhouettes.

Cliffs use triplanar limestone and shale with filtered detail normals. Sheltered gullies retain snow and loose scree collects below steep rock. Terrain textures use full mip chains and 16× anisotropic filtering. Distant stands blend into canopy cover as individual crowns shrink below a few pixels.

Run `cargo run --release --locked` for the alpine overlook. To capture the same alpine viewpoint and animation time, run `cargo run --release --locked -- --time 0 --capture target/alpine-overlook.ppm`.

Press F5 and open **Clouds** to change sky resolution, ray samples, lighting samples, and alpine cloud shadow samples while the scene runs. Set either lighting or cloud shadow samples to zero to skip that work.

In the F5 **God rays** tab, adjust **Radial samples** to resolve finer shafts and **Edge refinement** to smooth them near silhouettes. Set radial samples to zero to turn off god rays. Reset restores 12 radial samples and 5 edge refinement steps.

- PBR Rendering
- HDR Rendering
- Deferred Rendering
- GPU terrain
- GPU grass
- Four cascaded alpine shadow maps for terrain and conifers
- Rayleigh scattering and altitude-dependent valley haze
- Raymarched clouds with moving shadows on land and water
- ImGUI integration for debug operations

## Screenshots
<img width="958" height="549" alt="image" src="https://github.com/user-attachments/assets/2bee0193-34bc-4738-ab9b-cc9dd2b0e03e" />
<img width="960" height="545" alt="image" src="https://github.com/user-attachments/assets/8cfd1ece-3d0e-490c-9a49-a1f9191ff93d" />
<img width="956" height="544" alt="image" src="https://github.com/user-attachments/assets/23da88df-2011-4b72-a015-dfa2f6991fa0" />

## WIP
<img width="956" height="548" alt="image" src="https://github.com/user-attachments/assets/0b48c2d6-8e80-4f4c-a90e-a5ba550fefb4" />
<img width="953" height="542" alt="image" src="https://github.com/user-attachments/assets/161ec109-cf65-468b-b547-4bc153508336" />
<img width="959" height="545" alt="image" src="https://github.com/user-attachments/assets/e268b9a3-c51a-4b91-b221-4cddfba50b9b" />
<img width="953" height="545" alt="image" src="https://github.com/user-attachments/assets/e8ce5d20-c11c-4f65-91d7-23fbd17ea8dd" />
<img width="1147" height="740" alt="image" src="https://github.com/user-attachments/assets/826e93c7-ec46-45fc-9b6f-7978cfa6c9db" />





