cbuffer Overlay : register(b0) {
    float2 scale;
    float2 offset;
    float hdr;
};

Texture2D<float> fontAtlas : register(t0);
SamplerState fontSampler : register(s0);

struct Vertex {
    float2 position : POSITION;
    float2 uv : TEXCOORD;
    float4 color : COLOR;
};

struct Pixel {
    float4 position : SV_Position;
    float2 uv : TEXCOORD;
    float4 color : COLOR;
};

Pixel imgui_vs(Vertex vertex) {
    Pixel output;
    output.position = float4(vertex.position * scale + offset, 0, 1);
    output.uv = vertex.uv;
    output.color = vertex.color;
    return output;
}

float4 imgui_ps(Pixel input) : SV_Target {
    float3 rgb = input.color.rgb;
    if (hdr > 0.5) {
        // Convert Dear ImGui's sRGB style colors to scRGB at 160 nits paper white.
        rgb = 2.0 * lerp(rgb / 12.92, pow((rgb + 0.055) / 1.055, 2.4), step(0.04045, rgb));
    }
    return float4(rgb, input.color.a * fontAtlas.Sample(fontSampler, input.uv));
}
