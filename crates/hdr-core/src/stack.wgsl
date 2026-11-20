struct Params {
    frames: u32,
    pixels: u32,
    method: u32,
    param: f32,
};

@group(0) @binding(0) var<storage, read> input_data: array<u32>;
@group(0) @binding(1) var<storage, read_write> output_data: array<f32>;
@group(0) @binding(2) var<uniform> params: Params;

const MAX_FRAMES: u32 = 64u;

// Two f16 samples per u32; the cache writes little-endian half floats.
fn load_value(index: u32) -> f32 {
    let word = input_data[index >> 1u];
    let pair = unpack2x16float(word);
    if ((index & 1u) == 0u) {
        return pair.x;
    }
    return pair.y;
}

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let total = params.pixels * 3u;
    let idx = gid.x;
    if (idx >= total) {
        return;
    }

    var values: array<f32, 64>;
    var count: u32 = 0u;
    for (var f: u32 = 0u; f < params.frames; f = f + 1u) {
        values[count] = load_value(f * total + idx);
        count = count + 1u;
    }
    if (count == 0u) {
        output_data[idx] = 0.0;
        return;
    }

    var result: f32 = 0.0;
    let method = params.method;

    if (method == 0u || method == 1u) {
        // sum / mean: no ordering needed at all
        var acc: f32 = 0.0;
        for (var i: u32 = 0u; i < count; i = i + 1u) {
            acc = acc + values[i];
        }
        if (method == 0u) {
            result = acc;
        } else {
            result = acc / f32(count);
        }
    } else if (method == 5u) {
        // max
        var best: f32 = values[0];
        for (var i: u32 = 1u; i < count; i = i + 1u) {
            best = max(best, values[i]);
        }
        result = best;
    } else {
        // insertion sort, ascending: N is <= 64 and usually ~40
        for (var i: u32 = 1u; i < count; i = i + 1u) {
            let key = values[i];
            var j: i32 = i32(i) - 1;
            loop {
                if (j < 0) {
                    break;
                }
                if (values[u32(j)] <= key) {
                    break;
                }
                values[u32(j) + 1u] = values[u32(j)];
                j = j - 1;
            }
            values[u32(j) + 1u] = key;
        }

        if (method == 2u) {
            // median
            if (count % 2u == 1u) {
                result = values[count / 2u];
            } else {
                result = 0.5 * (values[count / 2u - 1u] + values[count / 2u]);
            }
        } else if (method == 3u) {
            // percentile, param = p in 0..=100
            var index = u32(round(params.param / 100.0 * f32(count - 1u)));
            index = min(index, count - 1u);
            result = values[index];
        } else if (method == 4u) {
            // trimmed max, param = how many top values to drop
            let dropped = u32(params.param);
            let index = count - 1u - min(dropped, count - 1u);
            result = values[index];
        }
    }

    output_data[idx] = result;
}
