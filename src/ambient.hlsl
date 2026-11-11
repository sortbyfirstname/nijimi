Texture2D<float4> desktop : register(t0);
Texture2D<float4> ambient : register(t1);
Texture2D<float4> narrowAmbient : register(t2);
Texture2D<float4> prefiltered : register(t3);
SamplerState linearClamp : register(s0);

cbuffer SceneInfo : register(b0)
{
    float4 contentRect;
    // The content minus the outermost edge the edge blend covers. Only this area is sampled, so
    // the overlay never reads back its own pixels when it is visible to screen capture.
    float4 sampleRect;
    float blurSigma;
    float blurRadius;
    float edgeBlend;
    float downsample;
    float edgeInset;
    float glowDownsample;
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
    float2 minSample = sampleRect.xy + halfTexel;
    float2 maxSample = sampleRect.zw - halfTexel;
    float2 outside = max(max(sampleRect.xy - uv, uv - sampleRect.zw), 0.0);
    float reflectedX = clamp(
        reflectCoordinate(uv.x, sampleRect.x, sampleRect.z),
        minSample.x,
        maxSample.x);
    float reflectedY = clamp(
        reflectCoordinate(uv.y, sampleRect.y, sampleRect.w),
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

// Averages the downsample x downsample block of desktop texels behind each blur texel, using
// bilinear taps that each cover a 2x2 quad.
float4 PSPrepare(VertexOutput input) : SV_TARGET
{
    uint width, height;
    desktop.GetDimensions(width, height);
    float2 texel = 1.0 / float2(width, height);
    int taps = max(1, (int)downsample / 2);
    float center = (taps - 1) * 0.5;
    float4 color = 0.0;
    [loop]
    for (int y = 0; y < taps; ++y)
    {
        [loop]
        for (int x = 0; x < taps; ++x)
        {
            float2 offset = (float2(x, y) - center) * 2.0 * texel;
            color += sampleReflected(input.uv + offset);
        }
    }
    return color / (taps * taps);
}

// Gaussian blur along one axis. Pairs of taps are merged into a single bilinear fetch placed
// between them at their weighted centre, halving the number of samples.
float4 gaussianBlur(float2 uv, float2 step)
{
    float sigma = max(blurSigma, 0.01);
    float falloff = -0.5 / (sigma * sigma);
    float4 color = ambient.SampleLevel(linearClamp, uv, 0);
    float total = 1.0;
    int radius = (int)blurRadius;
    [loop]
    for (int i = 1; i <= radius; i += 2)
    {
        float nearWeight = exp(falloff * i * i);
        float farWeight = exp(falloff * (i + 1) * (i + 1));
        float weight = nearWeight + farWeight;
        float offset = (i * nearWeight + (i + 1) * farWeight) / weight;
        color += ambient.SampleLevel(linearClamp, uv + step * offset, 0) * weight;
        color += ambient.SampleLevel(linearClamp, uv - step * offset, 0) * weight;
        total += 2.0 * weight;
    }
    return color / total;
}

float4 PSBlurHorizontal(VertexOutput input) : SV_TARGET
{
    uint width, height;
    ambient.GetDimensions(width, height);
    return gaussianBlur(input.uv, float2(1.0 / width, 0.0));
}

float4 PSBlurVertical(VertexOutput input) : SV_TARGET
{
    uint width, height;
    ambient.GetDimensions(width, height);
    return gaussianBlur(input.uv, float2(0.0, 1.0 / height));
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
    // As bright as the picture where it meets it, settling to the glow's own level far enough
    // out that the drop never reads as a line.
    float alpha = lerp(1.0, 0.86, smoothstep(0.0, 0.5 * fadeRadius, distancePixels)) * fade;
    float3 broadColor = ambient.SampleLevel(linearClamp, input.uv, 0).rgb;
    float3 detailColor = narrowAmbient.SampleLevel(linearClamp, input.uv, 0).rgb;
    float3 color = lerp(broadColor, detailColor, 0.35);
    return float4(color * alpha, alpha);
}

// Up to half a step of 8-bit noise, so smooth gradients don't band on the 8-bit swap chain.
float dither(float2 position)
{
    return frac(52.9829189 * frac(dot(position, float2(0.06711056, 0.00583715)))) - 0.5;
}

// Reads one mip level with a cubic B-spline instead of bilinear filtering. Bilinear turns the
// coarse, box-filtered mips into a visible grid of ramps; the B-spline is smooth across texel
// boundaries. Its 4x4 footprint is gathered with four bilinear taps.
float3 sampleBSpline(float2 uv, uint level)
{
    uint width, height, levels;
    prefiltered.GetDimensions(level, width, height, levels);
    float2 size = float2(width, height);
    float2 position = uv * size - 0.5;
    float2 base = floor(position);
    float2 f = position - base;
    float2 f2 = f * f;
    float2 f3 = f2 * f;
    float2 w0 = (1.0 - 3.0 * f + 3.0 * f2 - f3) / 6.0;
    float2 w1 = (4.0 - 6.0 * f2 + 3.0 * f3) / 6.0;
    float2 w2 = (1.0 + 3.0 * f + 3.0 * f2 - 3.0 * f3) / 6.0;
    float2 w3 = f3 / 6.0;
    float2 g0 = w0 + w1;
    float2 g1 = w2 + w3;
    float2 uv0 = (base - 0.5 + w1 / g0) / size;
    float2 uv1 = (base + 1.5 + w3 / g1) / size;
    return g0.y * (g0.x * prefiltered.SampleLevel(linearClamp, float2(uv0.x, uv0.y), level).rgb
                 + g1.x * prefiltered.SampleLevel(linearClamp, float2(uv1.x, uv0.y), level).rgb)
         + g1.y * (g0.x * prefiltered.SampleLevel(linearClamp, float2(uv0.x, uv1.y), level).rgb
                 + g1.x * prefiltered.SampleLevel(linearClamp, float2(uv1.x, uv1.y), level).rgb);
}

// A blur whose radius, in screen pixels, can change from pixel to pixel. It blends between the
// two nearest levels of the prefiltered texture's mip chain, and fades in from the sharp picture
// below the radius of a single prefiltered texel.
float3 blurAt(float2 uv, float radius)
{
    // A mip texel read through the B-spline blurs about as much as a Gaussian of 0.65 times its
    // width (0.29 from the box average, the rest from the B-spline).
    float texelRadius = 0.65 * downsample;
    if (radius < texelRadius)
    {
        float3 sharp = sampleReflected(uv).rgb;
        return lerp(sharp, prefiltered.SampleLevel(linearClamp, uv, 0).rgb, radius / texelRadius);
    }
    uint width, height, levels;
    prefiltered.GetDimensions(0, width, height, levels);
    float lod = min(log2(radius / texelRadius), levels - 1.0);
    uint lower = (uint)lod;
    uint upper = min(lower + 1, levels - 1);
    return lerp(sampleBSpline(uv, lower), sampleBSpline(uv, upper), lod - lower);
}

// Measured outwards from the edge of the sampled area, the blur radius only ever grows, so
// sharpness falls off smoothly instead of stopping at a line.
//
// Over the covered edge of the picture (edge softness), it grows to half the band's width while
// the overlay fades in, reaching full opacity a pixel before the picture ends so the picture's
// own hard edge is always hidden. Where the picture still shows through, the overlay is nearly
// as sharp as it, so nothing ghosts.
//
// Across the bar (edge blend), it carries on from there to the glow's radius, handing over to the
// glow itself across the whole band. A shorter handover would cut off glow spilling in from
// bright things further out with a straight line. Without edge blend the bar is just the glow.
float4 blendEdge(float2 uv, float4 glow, bool inside)
{
    uint width, height;
    desktop.GetDimensions(width, height);
    float2 outside = max(max(sampleRect.xy - uv, uv - sampleRect.zw), 0.0);
    float distance = length(outside * float2(width, height));
    float edgeRadius = 0.5 * edgeInset;
    if (inside)
    {
        float opacity = smoothstep(0.0, max(edgeInset - 1.0, 0.5), distance);
        return float4(blurAt(uv, edgeRadius * distance / max(edgeInset, 1.0)), 1.0) * opacity;
    }
    if (edgeBlend <= 0.0)
        return glow;
    float progress = saturate((distance - edgeInset) / edgeBlend);
    if (progress >= 1.0)
        return glow;
    float radius = lerp(edgeRadius, blurSigma * glowDownsample, progress);
    float4 color = float4(blurAt(uv, radius), 1.0);
    return lerp(color, glow, smoothstep(0.0, 1.0, progress));
}

float4 PSComposite(VertexOutput input) : SV_TARGET
{
    bool inside = insideContent(input.uv);
    float4 color = ambient.SampleLevel(linearClamp, input.uv, 0);
    if (edgeBlend > 0.0 || edgeInset > 0.0)
        color = blendEdge(input.uv, color, inside);
    else if (inside)
        return 0.0;
    color.rgb += dither(input.position.xy) / 255.0 * color.a;
    color.rgb = clamp(color.rgb, 0.0, color.a);
    return color;
}

// Settings preview: lays the sample image over the content area of a black screen, cropped to
// fill it. Bind any screen-sized texture as `desktop` so its size is known.
float4 PSPreviewScreen(VertexOutput input) : SV_TARGET
{
    if (!insideContent(input.uv))
        return float4(0.0, 0.0, 0.0, 1.0);
    uint screenWidth, screenHeight, sampleWidth, sampleHeight;
    desktop.GetDimensions(screenWidth, screenHeight);
    ambient.GetDimensions(sampleWidth, sampleHeight);
    float2 contentSize = (contentRect.zw - contentRect.xy) * float2(screenWidth, screenHeight);
    float contentAspect = contentSize.x / contentSize.y;
    float sampleAspect = (float)sampleWidth / sampleHeight;
    float2 crop = contentAspect > sampleAspect
        ? float2(1.0, sampleAspect / contentAspect)
        : float2(contentAspect / sampleAspect, 1.0);
    float2 local = (input.uv - contentRect.xy) / (contentRect.zw - contentRect.xy);
    return float4(ambient.SampleLevel(linearClamp, 0.5 + (local - 0.5) * crop, 0).rgb, 1.0);
}

// Settings preview: the premultiplied overlay over the screen, as the compositor would show it.
float4 PSPreviewFinal(VertexOutput input) : SV_TARGET
{
    int3 texel = int3(input.position.xy, 0);
    float4 overlay = ambient.Load(texel);
    float3 screen = desktop.Load(texel).rgb;
    return float4(overlay.rgb + screen * (1.0 - overlay.a), 1.0);
}
