struct Screen {
    size: vec2<f32>,
};

@group(0) @binding(0)
var<uniform> screen: Screen;

@vertex
fn vs_main(
    @builtin(vertex_index) in_vertex_index: u32,
) -> @builtin(position) vec4<f32> {
    var pos = array<vec2f, 6>(
        vec2(-1.0, 1.0),
        vec2(-1.0, -1.0),
        vec2(1.0, 1.0),
        vec2(1.0, 1.0),
        vec2(-1.0, -1.0),
        vec2(1.0, -1.0),
    );

    return vec4<f32>(pos[in_vertex_index].x, pos[in_vertex_index].y, 0.0, 1.0);
}

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    var posf = pos.xy / screen.size;

    // keep it in sync with the cpu implementation
    var color = oklab_to_linear_srgb(vec3<f32>(
        0.7,
        posf.x * 0.8 - 0.4,
        posf.y * 0.8 - 0.4,
    ));

    return vec4<f32>(color.x, color.y, color.z, 1.0);
}


fn oklab_to_linear_srgb(oklab: vec3<f32>) -> vec3<f32> {
    let l_ =  0.2158037573 * oklab.z + (0.3963377774 * oklab.y + oklab.x);
    let m_ = -0.0638541728 * oklab.z + (-0.1055613458 * oklab.y + oklab.x);
    let s_ = -1.2914855480 * oklab.z + (-0.0894841775 * oklab.y + oklab.x);
    let l = l_ * l_ * l_;
    let m = m_ * m_ * m_;
    let s = s_ * s_ * s_;
    return vec3<f32>(
        0.2309699292 * s + (4.0767416621 * l + -3.3077115913 * m),
        -0.3413193965 * s + (-1.2684380046 * l + 2.6097574011 * m),
        1.7076147010 * s + (-0.0041960863 * l + -0.7034186147 * m),
    );
}

