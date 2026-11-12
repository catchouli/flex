//! Lists a TFLite model's layers (every subgraph), with their tensors, small constants and options.
use tract_tflite::tflite::TensorType;

fn main() {
    for path in std::env::args().skip(1) {
        let bytes = std::fs::read(&path).unwrap();
        let model = tract_tflite::tflite::root_as_model(&bytes).unwrap();
        let codes = model.operator_codes().unwrap();
        let buffers = model.buffers().unwrap();
        println!("{path}");
        for (gi, g) in model.subgraphs().unwrap().iter().enumerate() {
            let tensors = g.tensors().unwrap();
            let show = |i: i32| {
                if i < 0 {
                    return "-".to_string();
                }
                let t = tensors.get(i as usize);
                let data = buffers.get(t.buffer() as usize).data().map(|d| d.bytes().to_vec()).unwrap_or_default();
                let name = t.name().unwrap_or("");
                let short = name.rsplit(';').next().unwrap_or(name).rsplit('/').take(2).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("/");
                let shape = t.shape().map(|s| s.iter().collect::<Vec<_>>()).unwrap_or_default();
                let mut s = format!("#{i} {short}{shape:?}");
                if !data.is_empty() && data.len() <= 64 {
                    let vals: Vec<String> = data.as_chunks::<4>().0.iter().map(|b| match t.type_() {
                        TensorType::FLOAT32 => format!("{}", f32::from_le_bytes(*b)),
                        _ => format!("{}", i32::from_le_bytes(*b)),
                    }).collect();
                    s += &format!("={}", vals.join(","));
                } else if !data.is_empty() {
                    s += &format!(" ({}B)", data.len());
                }
                if t.type_() != TensorType::FLOAT32 { s += "·"; }
                s
            };
            println!("== subgraph {gi}: inputs {:?} outputs {:?}", g.inputs().unwrap().iter().map(show).collect::<Vec<_>>(), g.outputs().unwrap().iter().map(show).collect::<Vec<_>>());
            for op in g.operators().unwrap() {
                let code = codes.get(op.opcode_index() as usize);
                let ins: Vec<String> = op.inputs().unwrap().iter().map(show).collect();
                let outs: Vec<String> = op.outputs().unwrap().iter().map(show).collect();
                let opts = [
                    op.builtin_options_as_conv_2_doptions().map(|o| format!("{o:?}")),
                    op.builtin_options_as_pool_2_doptions().map(|o| format!("{o:?}")),
                    op.builtin_options_as_leaky_relu_options().map(|o| format!("{o:?}")),
                    op.builtin_options_as_while_options().map(|o| format!("{o:?}")),
                    op.builtin_options_as_fully_connected_options().map(|o| format!("fused {:?} keep {}", o.fused_activation_function(), o.keep_num_dims())),
                    op.builtin_options_as_gather_options().map(|o| format!("{o:?}")),
                    op.builtin_options_as_strided_slice_options().map(|o| format!("{o:?}")),
                ].into_iter().flatten().collect::<Vec<_>>().join(" ");
                println!(" {:?} {ins:?} -> {outs:?} {opts}", code.builtin_code());
            }
        }
    }
}
