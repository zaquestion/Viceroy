fn mangle_imports(bytes: &[u8]) -> anyhow::Result<wasm_encoder::Module> {
    let mut module = wasm_encoder::Module::new();

    for payload in wasmparser::Parser::new(0).parse_all(&bytes) {
        let payload = payload?;
        match payload {
            wasmparser::Payload::Version {
                encoding: wasmparser::Encoding::Component,
                ..
            } => {
                anyhow::bail!("Mangling only supports core-wasm modules, not components");
            }

            wasmparser::Payload::ImportSection(section) => {
                let mut imports = wasm_encoder::ImportSection::new();

                for import in section {
                    let import = import?;
                    let entity = wasm_encoder::EntityType::try_from(import.ty).map_err(|_| {
                        anyhow::anyhow!(
                            "Failed to translate type for import {}:{}",
                            import.module,
                            import.name
                        )
                    })?;

                    // Leave the existing preview1 imports alone
                    if import.module == "wasi_snapshot_preview1" {
                        imports.import(import.module, import.name, entity);
                    } else {
                        let module = "wasi_snapshot_preview1";
                        let name = format!("{}#{}", import.module, import.name);
                        imports.import(module, &name, entity);
                    }
                }

                module.section(&imports);
            }

            payload => {
                if let Some((id, range)) = payload.as_section() {
                    module.section(&wasm_encoder::RawSection {
                        id,
                        data: &bytes[range],
                    });
                }
            }
        }
    }

    Ok(module)
}

/// Given bytes that represent a core wasm module, adapt it to a component using the viceroy
/// adapter.
pub fn adapt_bytes(bytes: &[u8]) -> anyhow::Result<Vec<u8>> {
    let module = mangle_imports(bytes)?;

    let component = wit_component::ComponentEncoder::default()
        .module(module.as_slice())?
        .adapter("wasi_snapshot_preview1", viceroy_artifacts::ADAPTER_BYTES)?
        .validate(true)
        .encode()?;

    Ok(component)
}
