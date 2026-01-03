use deno_ast::{MediaType, ParseParams, TranspileOptions, TranspileModuleOptions, EmitOptions};
use deno_core::{
    ModuleLoadResponse, ModuleSource, ModuleSourceCode, ModuleSpecifier, ModuleType,
    ModuleLoadOptions, ModuleLoadReferrer, ResolutionKind,
};
use deno_error::JsErrorBox;
use std::sync::Arc;

const EBDEV_MODULE: &str = include_str!("../types/ebdev.js");

pub struct TsModuleLoader(pub std::path::PathBuf);

impl deno_core::ModuleLoader for TsModuleLoader {
    fn resolve(&self, spec: &str, referrer: &str, _: ResolutionKind) -> Result<ModuleSpecifier, JsErrorBox> {
        if spec == "ebdev" {
            return Ok(ModuleSpecifier::parse("ebdev:runtime").unwrap());
        }
        if spec.starts_with("./") || spec.starts_with("../") {
            let base = ModuleSpecifier::parse(referrer).ok()
                .and_then(|s| s.to_file_path().ok())
                .and_then(|p| p.parent().map(|p| p.to_path_buf()))
                .unwrap_or_else(|| self.0.clone());
            let resolved = base.join(spec).canonicalize()
                .map_err(|e| JsErrorBox::generic(format!("{}: {}", spec, e)))?;
            return ModuleSpecifier::from_file_path(resolved).map_err(|_| JsErrorBox::generic("path"));
        }
        deno_core::resolve_import(spec, referrer).map_err(|e| JsErrorBox::generic(e.to_string()))
    }

    fn load(&self, spec: &ModuleSpecifier, _: Option<&ModuleLoadReferrer>, _: ModuleLoadOptions) -> ModuleLoadResponse {
        let spec = spec.clone();
        if spec.scheme() == "ebdev" {
            return ModuleLoadResponse::Sync(Ok(ModuleSource::new(
                ModuleType::JavaScript, ModuleSourceCode::String(EBDEV_MODULE.to_string().into()), &spec, None,
            )));
        }
        ModuleLoadResponse::Sync((|| {
            let path = spec.to_file_path().map_err(|_| JsErrorBox::generic("path"))?;
            let code = std::fs::read_to_string(&path).map_err(|e| JsErrorBox::generic(e.to_string()))?;
            let media = MediaType::from_path(&path);

            let code = if media.is_typed() {
                deno_ast::parse_module(ParseParams {
                    specifier: spec.clone(), text: Arc::from(code), media_type: media,
                    capture_tokens: false, scope_analysis: false, maybe_syntax: None,
                }).map_err(|e| JsErrorBox::generic(e.to_string()))?
                .transpile(&TranspileOptions::default(), &TranspileModuleOptions::default(), &EmitOptions::default())
                .map_err(|e| JsErrorBox::generic(e.to_string()))?.into_source().text
            } else { code };

            let module_type = if media == MediaType::Json { ModuleType::Json } else { ModuleType::JavaScript };
            Ok(ModuleSource::new(module_type, ModuleSourceCode::String(code.into()), &spec, None))
        })())
    }
}
