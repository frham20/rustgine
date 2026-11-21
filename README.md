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
<img width="638" height="360" alt="image" src="https://github.com/user-attachments/assets/01488493-b052-4a34-be4b-ccd731965ec7" />
<img width="1131" height="689" alt="image" src="https://github.com/user-attachments/assets/28d1b891-58cb-42ab-abfd-0848413ffd9e" />
<img width="833" height="520" alt="image" src="https://github.com/user-attachments/assets/8edefe45-0260-4e7a-9c90-75b8007a7dc0" />
<img width="1072" height="701" alt="image" src="https://github.com/user-attachments/assets/249ae336-7bec-4b64-9b10-67cf006ec8ca" />
<img width="1070" height="694" alt="image" src="https://github.com/user-attachments/assets/7c03f63f-f506-452f-9c26-5cf4cafc6e6b" />


## Old Screenshots
<img width="1147" height="740" alt="image" src="https://github.com/user-attachments/assets/826e93c7-ec46-45fc-9b6f-7978cfa6c9db" />
<img width="953" height="545" alt="image" src="https://github.com/user-attachments/assets/e8ce5d20-c11c-4f65-91d7-23fbd17ea8dd" />
<img width="959" height="545" alt="image" src="https://github.com/user-attachments/assets/e268b9a3-c51a-4b91-b221-4cddfba50b9b" />
<img width="953" height="542" alt="image" src="https://github.com/user-attachments/assets/161ec109-cf65-468b-b547-4bc153508336" />
<img width="956" height="548" alt="image" src="https://github.com/user-attachments/assets/0b48c2d6-8e80-4f4c-a90e-a5ba550fefb4" />
<img width="958" height="549" alt="image" src="https://github.com/user-attachments/assets/2bee0193-34bc-4738-ab9b-cc9dd2b0e03e" />
<img width="960" height="545" alt="image" src="https://github.com/user-attachments/assets/8cfd1ece-3d0e-490c-9a49-a1f9191ff93d" />
<img width="956" height="544" alt="image" src="https://github.com/user-attachments/assets/23da88df-2011-4b72-a015-dfa2f6991fa0" />








