struct Input {
    size: vec2<f32>,
    voronoi_progress: f32,
};

@group(0) @binding(0)
var<uniform> input: Input;

@group(1) @binding(0)
var<storage, read> desktop_colors: array<vec4f>;

@vertex
fn vs_main(
    @builtin(vertex_index) in_vertex_index: u32,
) -> @builtin(position) vec4<f32> {
    // full-screen quad
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
    var posf = pos.xy / input.size;

    var color = vec3<f32>(
        0.7,
        posf.x * 0.8 - 0.4,
        posf.y * 0.7 - 0.4,
    );

    var best = vec3f(0.0, 0.0, 0.0);
    var best_score = 1000000000000.0;
    for (var i: u32 = 0; i < arrayLength(&desktop_colors); i++) {
        var elem = desktop_colors[i].xyz;
        var score = diff_colors(elem, color);
        if (score < best_score) {
            best = elem;
            best_score = score;
        }
    }
    var voronoi_color = best;
     
    color = mix(color, voronoi_color, input.voronoi_progress);

    // keep it in sync with the cpu implementation
    var srgbcolor = oklab_to_linear_srgb(color);

    return vec4<f32>(srgbcolor.x, srgbcolor.y, srgbcolor.z, 1.0);
}

// keep it in sync with the cpu implementation
fn diff_colors(oklab_a: vec3f, oklab_b: vec3f) -> f32 {
    var diff = oklab_a - oklab_b;
    var diff_sq = diff * diff;
    return diff_sq.x + diff_sq.y + diff_sq.z;
}

fn oklab_to_linear_srgb(oklab: vec3f) -> vec3f {
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
