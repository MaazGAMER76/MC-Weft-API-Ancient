//! Can the engine's bindless texture table be lowered to something this GPU compiles?
//!
//! The engine's shaders all sample `textures[i]`, where `i` comes from vertex data or a buffer -
//! a `binding_array<texture_2d<f32>, 65536>` indexed at runtime. This example feeds exactly that
//! pattern to naga's HLSL writer and prints what comes out, so the answer is the generated text
//! rather than a guess about it.
//!
//! The engine's own shader files are not parsed here: they need `crates/engine`'s `gpu_layout`
//! prelude (the structs, bindings and constants each one assumes), and pulling the engine into
//! this example's build would take longer than the question is worth.
//!
//! `cargo run -p weft_backend_d3d10 --example hlsl_lower`

/// The smallest shader that uses the feature under test: a dynamic index into the bindless table.
const BINDLESS: &str = r#"
@fragment
fn main(@location(0) uv: vec2<f32>, @location(1) handle: f32) -> @location(0) vec4<f32> {
    let index = u32(handle);
    return textureSample(textures[index], linear_clamp, uv);
}
"#;

fn main() {
    let shader = match weft_render::Shader::from_wgsl("bindless", BINDLESS) {
        Ok(shader) => shader,
        Err(error) => {
            println!("bindless probe: the WGSL itself did not compile: {error}");
            return;
        }
    };
    println!("bindless probe: {} bytes of WGSL parsed and validated", BINDLESS.len());
    for model in [naga::back::hlsl::ShaderModel::V5_0, naga::back::hlsl::ShaderModel::V6_5] {
        let name = model.to_str();
        match lower(&shader, model) {
            Err(error) => println!("\nhlsl {name}: rejected - {error}"),
            Ok(hlsl) => {
                println!("\nhlsl {name}: {} bytes", hlsl.len());
                for line in hlsl
                    .lines()
                    .filter(|l| l.contains("textures") || l.contains("register") || l.contains("Sample("))
                {
                    println!("    {}", line.trim_end());
                }
            }
        }
    }
    println!("\nthe Ironlake driver's ceiling is shader model 4.0 (feature level 10_0): it cannot load 5_0 or 6_5 bytecode.");
    println!("naga's HLSL writer has no model below V5_0, so this table has no target that this GPU accepts.");
}

fn lower(shader: &weft_render::Shader, shader_model: naga::back::hlsl::ShaderModel) -> Result<String, String> {
    let mut binding_map = naga::back::hlsl::BindingMap::default();
    for (group, binding, array_size) in bindings(&shader.module) {
        binding_map.insert(
            naga::ResourceBinding { group, binding },
            naga::back::hlsl::BindTarget { space: group as u8, register: binding, binding_array_size: array_size, ..Default::default() },
        );
    }
    let options =
        naga::back::hlsl::Options { shader_model, binding_map, fake_missing_bindings: true, restrict_indexing: true, ..Default::default() };
    let pipeline_options = naga::back::hlsl::PipelineOptions::default();
    let mut out = String::new();
    let mut writer = naga::back::hlsl::Writer::new(&mut out, &options, &pipeline_options);
    writer
        .write(&shader.module, &shader.info, None)
        .map_err(|e| format!("{e}"))?;
    Ok(out)
}

/// Every bound global: (group, binding, bindless array size).
fn bindings(module: &naga::Module) -> Vec<(u32, u32, Option<u32>)> {
    module
        .global_variables
        .iter()
        .filter_map(|(_, var)| {
            let b = var.binding.as_ref()?;
            let size = match &module.types[var.ty].inner {
                naga::TypeInner::BindingArray { size, .. } => match size {
                    naga::ArraySize::Constant(n) => Some(n.get()),
                    _ => None,
                },
                _ => None,
            };
            Some((b.group, b.binding, size))
        })
        .collect()
}
