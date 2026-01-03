use deno_ast::{MediaType, ParseParams, TranspileOptions, TranspileModuleOptions, EmitOptions};
use deno_core::{
    ModuleLoadResponse, ModuleSource, ModuleSourceCode, ModuleSpecifier, ModuleType,
    ModuleLoadOptions, ModuleLoadReferrer, ResolutionKind,
};
use deno_error::JsErrorBox;
use std::sync::Arc;

/// Virtual "ebdev" module providing defineConfig, presets, etc.
const EBDEV_MODULE: &str = include_str!("../types/ebdev.js");

pub struct TsModuleLoader {
    base_dir: std::path::PathBuf,
}

impl TsModuleLoader {
    pub fn new(base_dir: std::path::PathBuf) -> Self {
        Self { base_dir }
    }
}

impl deno_core::ModuleLoader for TsModuleLoader {
    fn resolve(&self, specifier: &str, referrer: &str, _kind: ResolutionKind) -> Result<ModuleSpecifier, JsErrorBox> {
        // Virtual "ebdev" module
        if specifier == "ebdev" {
            return Ok(ModuleSpecifier::parse("ebdev:runtime").unwrap());
        }

        // Relative imports
        if specifier.starts_with("./") || specifier.starts_with("../") {
            let base = ModuleSpecifier::parse(referrer)
                .ok()
                .and_then(|s| s.to_file_path().ok())
                .and_then(|p| p.parent().map(|p| p.to_path_buf()))
                .unwrap_or_else(|| self.base_dir.clone());

            let resolved = base.join(specifier).canonicalize()
                .map_err(|e| JsErrorBox::generic(format!("Cannot resolve {}: {}", specifier, e)))?;

            return ModuleSpecifier::from_file_path(resolved)
                .map_err(|_| JsErrorBox::generic("Invalid path"));
        }

        deno_core::resolve_import(specifier, referrer)
            .map_err(|e| JsErrorBox::generic(e.to_string()))
    }

    fn load(&self, specifier: &ModuleSpecifier, _referrer: Option<&ModuleLoadReferrer>, _opts: ModuleLoadOptions) -> ModuleLoadResponse {
        let specifier = specifier.clone();

        // Virtual ebdev module
        if specifier.scheme() == "ebdev" {
            return ModuleLoadResponse::Sync(Ok(ModuleSource::new(
                ModuleType::JavaScript,
                ModuleSourceCode::String(EBDEV_MODULE.to_string().into()),
                &specifier,
                None,
            )));
        }

        // File modules with TypeScript transpilation
        ModuleLoadResponse::Sync((|| {
            let path = specifier.to_file_path()
                .map_err(|_| JsErrorBox::generic(format!("Invalid path: {}", specifier)))?;

            let code = std::fs::read_to_string(&path)
                .map_err(|e| JsErrorBox::generic(format!("Cannot read {}: {}", path.display(), e)))?;

            let media_type = MediaType::from_path(&path);
            let needs_transpile = matches!(media_type,
                MediaType::TypeScript | MediaType::Tsx | MediaType::Mts | MediaType::Cts |
                MediaType::Jsx | MediaType::Dts | MediaType::Dmts | MediaType::Dcts
            );

            let code = if needs_transpile {
                let parsed = deno_ast::parse_module(ParseParams {
                    specifier: specifier.clone(),
                    text: Arc::from(code),
                    media_type,
                    capture_tokens: false,
                    scope_analysis: false,
                    maybe_syntax: None,
                }).map_err(|e| JsErrorBox::generic(e.to_string()))?;

                parsed.transpile(&TranspileOptions::default(), &TranspileModuleOptions::default(), &EmitOptions::default())
                    .map_err(|e| JsErrorBox::generic(e.to_string()))?
                    .into_source().text
            } else {
                code
            };

            let module_type = if media_type == MediaType::Json { ModuleType::Json } else { ModuleType::JavaScript };

            Ok(ModuleSource::new(module_type, ModuleSourceCode::String(code.into()), &specifier, None))
        })())
    }
}
