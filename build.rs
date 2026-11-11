fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rerun-if-changed=nijimi.rc");
        println!("cargo:rerun-if-changed=assets/tresbient.ico");
        embed_resource::compile("nijimi.rc", embed_resource::NONE)
            .manifest_optional()
            .expect("failed to embed the nijimi icon resource");
        #[cfg(windows)]
        shaders::compile_all();
    }
}

/// Compiles `src/ambient.hlsl` to bytecode at build time so the app never has to load the
/// HLSL compiler or compile shaders at runtime (which took ~100 ms on every renderer start).
#[cfg(windows)]
mod shaders {
    use std::path::PathBuf;

    use windows::Win32::Graphics::Direct3D::Fxc::{D3DCOMPILE_OPTIMIZATION_LEVEL3, D3DCompile};
    use windows::Win32::Graphics::Direct3D::{ID3DBlob, ID3DInclude};
    use windows::core::PCSTR;

    const SOURCE_PATH: &str = "src/ambient.hlsl";
    const ENTRIES: [(&str, &str); 8] = [
        ("VSMain", "vs_4_0"),
        ("PSPrepare", "ps_4_0"),
        ("PSBlurHorizontal", "ps_4_0"),
        ("PSBlurVertical", "ps_4_0"),
        ("PSBlurWideHorizontal", "ps_4_0"),
        ("PSBlurWideVertical", "ps_4_0"),
        ("PSBlur", "ps_4_0"),
        ("PSComposite", "ps_4_0"),
    ];

    pub fn compile_all() {
        println!("cargo:rerun-if-changed={SOURCE_PATH}");
        let source = std::fs::read(SOURCE_PATH).expect("failed to read the HLSL shader source");
        let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
        for (entry, target) in ENTRIES {
            let bytecode = compile(&source, entry, target);
            std::fs::write(out_dir.join(format!("{entry}.cso")), bytecode)
                .expect("failed to write compiled shader");
        }
    }

    fn compile(source: &[u8], entry: &str, target: &str) -> Vec<u8> {
        let entry_c = format!("{entry}\0");
        let target_c = format!("{target}\0");
        let mut blob: Option<ID3DBlob> = None;
        let mut errors: Option<ID3DBlob> = None;
        let result = unsafe {
            D3DCompile(
                source.as_ptr().cast(),
                source.len(),
                PCSTR(c"ambient.hlsl".as_ptr().cast()),
                None,
                None::<&ID3DInclude>,
                PCSTR(entry_c.as_ptr()),
                PCSTR(target_c.as_ptr()),
                D3DCOMPILE_OPTIMIZATION_LEVEL3,
                0,
                &mut blob,
                Some(&mut errors),
            )
        };
        if let Err(error) = result {
            let message = errors
                .map(|errors| unsafe { blob_bytes(&errors) })
                .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
                .unwrap_or_default();
            panic!("failed to compile shader {entry} ({target}): {error}\n{message}");
        }
        unsafe { blob_bytes(&blob.expect("D3DCompile returned no bytecode")) }
    }

    unsafe fn blob_bytes(blob: &ID3DBlob) -> Vec<u8> {
        unsafe {
            std::slice::from_raw_parts(blob.GetBufferPointer().cast::<u8>(), blob.GetBufferSize())
                .to_vec()
        }
    }
}
