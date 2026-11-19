# Terrain textures

The sixteen 512 × 512 sRGB BC1 DDS tiles each contain a complete mip chain. The renderer embeds them in one texture array. It uses elevation and slope to blend meadow, organic soil, mineral deposits, and exposed rock, with world-aligned sampling at different physical scales. Each tile wraps independently.

The DDS files use the DX10 header to record the sRGB format. [Microsoft DDS format guide](https://learn.microsoft.com/en-us/windows/win32/direct3ddds/dx-graphics-dds-pguide)

The built-in ImageGen tool generated one source image per tile. The source images were cropped to square tiles, corrected for periodic edges, matched to the terrain palette, and converted to mipmapped DDS.

## Source prompts

Each prompt started with the following text. The material description in the table followed `Surface:`.

> Use case: photorealistic-natural. Generate a single square seamless repeating albedo texture tile for a realistic temperate rolling hills terrain in a 3D game engine. Straight-down orthographic macro view of only the ground. Physical material fills the entire square at uniform scale and density. Diffuse flat even color under neutral light, no directional shadows, no vignettes, no specular highlights, no perspective, no uncluttered central area, no text or margins. Treat opposite edges and all corners as continuous with matching features for a seamless tile in both axes. Natural restrained color and subtle high-frequency detail. Surface:

| Tile | Material description |
| --- | --- |
| `meadow-sod.dds` | dense short meadow sod, olive and sage green interwoven miniature blade tips and brown thatch. |
| `fine-fescue.dds` | fine soft yellow-green fescue turf, wiry mats of hair-thin leaves, short dense growth. |
| `clover-meadow.dds` | meadow sod with a scattered understory of small clover leaves, medium desaturated green tones. |
| `dry-grass.dds` | sun-dried golden-brown short grass and pale straw mixed evenly into olive lawn. |
| `damp-moss.dds` | dense tiny ground moss, velvety mottled rich fern greens with tiny brown voids. |
| `mossy-soil.dds` | moist charcoal-brown crumbly soil with intermittent moss flecks and tiny organic fragments. |
| `leaf-litter.dds` | small broken ochre deciduous leaf fragments and browned grass mixed through dark humus; many fragments, no dominant leaf. |
| `dark-loam.dds` | sieved dark brown woodland loam, tiny aggregate clods, pinhead flecks of sand. |
| `compacted-earth.dds` | warm taupe compacted mineral earth, fine granular packed dirt and barely visible tiny stone chips. |
| `sandy-grit.dds` | weathered cool buff sandy grit with evenly distributed millimeter dark minerals and grain clumps. |
| `river-pebbles.dds` | small rounded mixed muted slate gray and gray-brown pebbles densely set into dark wet-looking silt. |
| `angular-gravel.dds` | densely scattered small angular gray limestone chips and warm brown gravel in packed soil. |
| `weathered-limestone.dds` | weathered light gray limestone ground slabs, hairline cracks and pitting, subdued tan mineral mottling. |
| `laminated-shale.dds` | weathered layered gray-brown shale surface, irregular thin laminae and many tiny flakes, low contrast. |
| `lichen-bedrock.dds` | gray weathered stone surface with small pale sage-green and ochre lichen colonies densely mottled in. |
| `coarse-scree.dds` | closely packed angular coarse scree fragments and granular dark soil, soft earthy charcoal gray with taupe. |
