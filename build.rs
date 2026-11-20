use std::{env, ffi::OsString, fs, path::PathBuf, process::Command};

fn find_dxc() -> OsString {
    if let Some(path) = env::var_os("DXC_PATH") {
        return path;
    }
    if Command::new("dxc.exe").arg("-help").output().is_ok() {
        return "dxc.exe".into();
    }
    let kits = PathBuf::from(env::var_os("ProgramFiles(x86)").unwrap_or_default())
        .join("Windows Kits/10/bin");
    let mut versions: Vec<_> = fs::read_dir(kits)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let version: Vec<u32> = entry
                .file_name()
                .to_str()?
                .split('.')
                .map(str::parse)
                .collect::<Result<_, _>>()
                .ok()?;
            let path = entry.path().join("x64/dxc.exe");
            path.is_file().then_some((version, path))
        })
        .collect();
    versions.sort_by(|a, b| a.0.cmp(&b.0));
    versions
        .pop()
        .expect("Install Windows SDK with DXC, or set DXC_PATH to dxc.exe")
        .1
        .into()
}

fn main() {
    println!("cargo:rerun-if-env-changed=DXC_PATH");
    println!("cargo:rerun-if-changed=shaders");
    let dxc = find_dxc();
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    for (entry, profile) in [
        ("scene_vs", "vs_6_0"),
        ("shadow_vs", "vs_6_0"),
        ("scene_ps", "ps_6_0"),
        ("wireframe_ps", "ps_6_0"),
        ("output_vs", "vs_6_0"),
        ("output_ps", "ps_6_0"),
        ("grass_vs", "vs_6_0"),
        ("grass_ps", "ps_6_0"),
        ("grass_ms", "ms_6_5"),
        ("grass_classify", "cs_6_0"),
        ("grass_scan", "cs_6_0"),
        ("grass_commands", "cs_6_0"),
        ("grass_scatter", "cs_6_0"),
        ("terrain_init", "cs_6_0"),
        ("terrain_evaluate", "cs_6_0"),
        ("terrain_split_offsets", "cs_6_0"),
        ("terrain_apply", "cs_6_0"),
        ("terrain_reduce_blocks", "cs_6_0"),
        ("terrain_reduce_root", "cs_6_0"),
        ("terrain_seam_clear", "cs_6_0"),
        ("terrain_seam_mark", "cs_6_0"),
        ("terrain_seam_resolve", "cs_6_0"),
        ("terrain_vs", "vs_6_0"),
        ("terrain_ps", "ps_6_0"),
        ("terrain_reflection_ps", "ps_6_0"),
        ("deferred_ps", "ps_6_0"),
        ("clouds_ps", "ps_6_0"),
        ("cloud_shadow_ps", "ps_6_0"),
        ("sky_reflection_ps", "ps_6_0"),
        ("land_vs", "vs_6_0"),
        ("land_ps", "ps_6_0"),
        ("land_reflection_ps", "ps_6_0"),
        ("pine_vs", "vs_6_0"),
        ("anchor_trees", "cs_6_0"),
        ("pine_ps", "ps_6_0"),
        ("pine_reflection_ps", "ps_6_0"),
        ("lake_vs", "vs_6_0"),
        ("lake_ps", "ps_6_0"),
        ("imgui_vs", "vs_6_0"),
        ("imgui_ps", "ps_6_0"),
    ] {
        let source = if matches!(
            entry,
            "deferred_ps" | "clouds_ps" | "sky_reflection_ps" | "cloud_shadow_ps"
        ) {
            "shaders/deferred.hlsl"
        } else if entry == "anchor_trees" {
            "shaders/forest_anchor.hlsl"
        } else if entry.starts_with("land_")
            || entry.starts_with("pine_")
            || entry.starts_with("lake_")
        {
            "shaders/alpine.hlsl"
        } else if entry.starts_with("imgui_") {
            "shaders/imgui.hlsl"
        } else if entry.starts_with("grass_") && profile == "cs_6_0" {
            "shaders/grass_generate.hlsl"
        } else if entry.starts_with("grass_") {
            "shaders/grass.hlsl"
        } else if entry.starts_with("terrain_seam_") {
            "shaders/terrain_seams.hlsl"
        } else if profile == "cs_6_0" {
            "shaders/terrain_lod.hlsl"
        } else if entry.starts_with("terrain_") {
            "shaders/terrain.hlsl"
        } else {
            "shaders/scene.hlsl"
        };
        let status = Command::new(&dxc)
            .args(["-nologo", "-WX", "-O3", "-T", profile, "-E", entry, "-Fo"])
            .arg(out.join(format!("{entry}.dxil")))
            .arg(source)
            .status()
            .expect("launch the DirectX Shader Compiler");
        assert!(status.success(), "DXC failed to compile {entry}");
    }
}
