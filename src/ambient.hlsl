Texture2D<float4> desktop : register(t0);
Texture2D<float4> ambient : register(t1);
Texture2D<float4> narrowAmbient : register(t2);
SamplerState linearClamp : register(s0);

cbuffer SceneInfo : register(b0)
{
    float4 contentRect;
};

struct VertexOutput
{
    float4 position : SV_POSITION;
    float2 uv : TEXCOORD0;
};

VertexOutput VSMain(uint id : SV_VertexID)
{
    float2 p = float2((id << 1) & 2, id & 2);
    VertexOutput output;
    output.position = float4(p * float2(2.0, -2.0) + float2(-1.0, 1.0), 0.0, 1.0);
    output.uv = p;
    return output;
}

bool insideContent(float2 uv)
{
    return uv.x >= contentRect.x && uv.x < contentRect.z
        && uv.y >= contentRect.y && uv.y < contentRect.w;
}

float reflectCoordinate(float value, float minimum, float maximum)
{
    float span = max(maximum - minimum, 1.0 / 65535.0);
    float phase = frac((value - minimum) / (2.0 * span));
    return minimum + span * (1.0 - abs(phase * 2.0 - 1.0));
}

float4 sampleReflected(float2 uv)
{
    uint width, height;
    desktop.GetDimensions(width, height);
    float2 halfTexel = 0.5 / float2(width, height);
    float2 minSample = contentRect.xy + halfTexel;
    float2 maxSample = contentRect.zw - halfTexel;
    float2 outside = max(max(contentRect.xy - uv, uv - contentRect.zw), 0.0);
    float reflectedX = clamp(
        reflectCoordinate(uv.x, minSample.x, maxSample.x),
        minSample.x,
        maxSample.x);
    float reflectedY = clamp(
        reflectCoordinate(uv.y, minSample.y, maxSample.y),
        minSample.y,
        maxSample.y);
    float4 color = desktop.SampleLevel(linearClamp, float2(reflectedX, reflectedY), 0);

    if (outside.x > 0.0 && outside.y > 0.0)
    {
        float2 clampedUv = clamp(uv, minSample, maxSample);
        float4 sideColor = desktop.SampleLevel(linearClamp, float2(reflectedX, clampedUv.y), 0);
        float4 topColor = desktop.SampleLevel(linearClamp, float2(clampedUv.x, reflectedY), 0);
        float edgeWeight = outside.y / (outside.x + outside.y);
        float4 edgeColor = lerp(topColor, sideColor, edgeWeight);
        float cornerDistance = min(outside.x * width, outside.y * height);
        float cornerBlend = smoothstep(0.0, max(8.0, min(width, height) * 0.02), cornerDistance);
        color = lerp(color, edgeColor, cornerBlend);
    }
    return color;
}

float4 PSPrepare(VertexOutput input) : SV_TARGET
{
    uint width, height;
    desktop.GetDimensions(width, height);
    float2 texel = 1.0 / float2(width, height);
    float4 color = 0.0;
    [unroll]
    for (int y = 0; y < 4; ++y)
    {
        [unroll]
        for (int x = 0; x < 4; ++x)
        {
            float2 offset = (float2(x, y) - 1.5) * texel;
            color += sampleReflected(input.uv + offset) * (1.0 / 16.0);
        }
    }
    return color;
}


static const float BLUR_CENTER_WEIGHT = 0.2042;
static const float BLUR_NEAR_WEIGHT = 0.1802;
static const float BLUR_MIDDLE_WEIGHT = 0.1238;
static const float BLUR_FAR_WEIGHT = 0.0663;
static const float BLUR_OUTER_WEIGHT = 0.0276;

float4 PSBlurHorizontal(VertexOutput input) : SV_TARGET
{
    uint width, height;
    ambient.GetDimensions(width, height);
    float2 texel = 1.0 / float2(width, height);
    float4 color = ambient.SampleLevel(linearClamp, input.uv, 0) * BLUR_CENTER_WEIGHT;
    color += ambient.SampleLevel(linearClamp, input.uv + float2(texel.x * 2.0, 0), 0) * BLUR_NEAR_WEIGHT;
    color += ambient.SampleLevel(linearClamp, input.uv - float2(texel.x * 2.0, 0), 0) * BLUR_NEAR_WEIGHT;
    color += ambient.SampleLevel(linearClamp, input.uv + float2(texel.x * 4.0, 0), 0) * BLUR_MIDDLE_WEIGHT;
    color += ambient.SampleLevel(linearClamp, input.uv - float2(texel.x * 4.0, 0), 0) * BLUR_MIDDLE_WEIGHT;
    color += ambient.SampleLevel(linearClamp, input.uv + float2(texel.x * 8.0, 0), 0) * BLUR_FAR_WEIGHT;
    color += ambient.SampleLevel(linearClamp, input.uv - float2(texel.x * 8.0, 0), 0) * BLUR_FAR_WEIGHT;
    color += ambient.SampleLevel(linearClamp, input.uv + float2(texel.x * 12.0, 0), 0) * BLUR_OUTER_WEIGHT;
    color += ambient.SampleLevel(linearClamp, input.uv - float2(texel.x * 12.0, 0), 0) * BLUR_OUTER_WEIGHT;
    return color;
}

float4 PSBlurVertical(VertexOutput input) : SV_TARGET
{
    uint width, height;
    ambient.GetDimensions(width, height);
    float2 texel = 1.0 / float2(width, height);
    float4 color = ambient.SampleLevel(linearClamp, input.uv, 0) * BLUR_CENTER_WEIGHT;
    color += ambient.SampleLevel(linearClamp, input.uv + float2(0, texel.y * 2.0), 0) * BLUR_NEAR_WEIGHT;
    color += ambient.SampleLevel(linearClamp, input.uv - float2(0, texel.y * 2.0), 0) * BLUR_NEAR_WEIGHT;
    color += ambient.SampleLevel(linearClamp, input.uv + float2(0, texel.y * 4.0), 0) * BLUR_MIDDLE_WEIGHT;
    color += ambient.SampleLevel(linearClamp, input.uv - float2(0, texel.y * 4.0), 0) * BLUR_MIDDLE_WEIGHT;
    color += ambient.SampleLevel(linearClamp, input.uv + float2(0, texel.y * 8.0), 0) * BLUR_FAR_WEIGHT;
    color += ambient.SampleLevel(linearClamp, input.uv - float2(0, texel.y * 8.0), 0) * BLUR_FAR_WEIGHT;
    color += ambient.SampleLevel(linearClamp, input.uv + float2(0, texel.y * 12.0), 0) * BLUR_OUTER_WEIGHT;
    color += ambient.SampleLevel(linearClamp, input.uv - float2(0, texel.y * 12.0), 0) * BLUR_OUTER_WEIGHT;
    return color;
}

float4 PSBlurWideHorizontal(VertexOutput input) : SV_TARGET
{
    uint width, height;
    ambient.GetDimensions(width, height);
    float2 texel = 1.0 / float2(width, height);
    float4 color = ambient.SampleLevel(linearClamp, input.uv, 0) * 0.36;
    color += ambient.SampleLevel(linearClamp, input.uv + float2(texel.x, 0), 0) * 0.22;
    color += ambient.SampleLevel(linearClamp, input.uv - float2(texel.x, 0), 0) * 0.22;
    color += ambient.SampleLevel(linearClamp, input.uv + float2(texel.x * 4.0, 0), 0) * 0.10;
    color += ambient.SampleLevel(linearClamp, input.uv - float2(texel.x * 4.0, 0), 0) * 0.10;
    return color;
}

float4 PSBlurWideVertical(VertexOutput input) : SV_TARGET
{
    uint width, height;
    ambient.GetDimensions(width, height);
    float2 texel = 1.0 / float2(width, height);
    float4 color = ambient.SampleLevel(linearClamp, input.uv, 0) * 0.36;
    color += ambient.SampleLevel(linearClamp, input.uv + float2(0, texel.y), 0) * 0.22;
    color += ambient.SampleLevel(linearClamp, input.uv - float2(0, texel.y), 0) * 0.22;
    color += ambient.SampleLevel(linearClamp, input.uv + float2(0, texel.y * 4.0), 0) * 0.10;
    color += ambient.SampleLevel(linearClamp, input.uv - float2(0, texel.y * 4.0), 0) * 0.10;
    return color;
}

float4 PSBlur(VertexOutput input) : SV_TARGET
{
    uint width, height;
    desktop.GetDimensions(width, height);
    float2 distanceFromContent = max(
        max(contentRect.xy - input.uv, input.uv - contentRect.zw), 0.0);
    float distancePixels = length(distanceFromContent * float2(width, height));
    float fadeRadius = max(1.0, min(width, height) * 0.45);
    float fade = exp(-0.5 * (distancePixels / fadeRadius) * (distancePixels / fadeRadius));
    float alpha = 0.86 * fade;
    float3 broadColor = ambient.SampleLevel(linearClamp, input.uv, 0).rgb;
    float3 detailColor = narrowAmbient.SampleLevel(linearClamp, input.uv, 0).rgb;
    float3 color = lerp(broadColor, detailColor, 0.35);
    return float4(color * alpha, alpha);
}

float4 PSComposite(VertexOutput input) : SV_TARGET
{
    if (insideContent(input.uv))
        return 0.0;
    return ambient.SampleLevel(linearClamp, input.uv, 0);
}
