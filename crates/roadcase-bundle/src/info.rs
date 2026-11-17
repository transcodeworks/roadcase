//! The application's Info.plist.

use crate::config::{DocumentType, Rank, Role, TypeDeclaration, TypeOwnership};
use crate::{Project, Result, files};
use plist::{Dictionary, Value};

fn text(value: impl Into<String>) -> Value {
    Value::String(value.into())
}

fn texts(values: &[String]) -> Value {
    Value::Array(values.iter().cloned().map(Value::String).collect())
}

/// The Info.plist of the application's bundle: the `info_plist` template if
/// there is one, with every key this config describes set over it, then
/// `[macos.plist]`, then the variant's changes.
///
/// # Errors
///
/// [`crate::Error::Config`] without `[macos]`; [`crate::Error::Plist`] when the
/// template does not read; [`crate::Error::Value`].
pub fn app(project: &Project) -> Result<Dictionary> {
    let macos = project.macos()?;
    let context = &project.context;
    let mut plist = match &macos.info_plist {
        Some(path) => files::read_dictionary(&context.path(path)?)?,
        None => Dictionary::new(),
    };
    let name = context.name().to_owned();
    let executable = executable_name(project)?;

    if !plist.contains_key("CFBundleDevelopmentRegion") {
        plist.insert("CFBundleDevelopmentRegion".into(), text("en"));
    }
    plist.insert("CFBundleDisplayName".into(), text(&name));
    plist.insert("CFBundleExecutable".into(), text(executable));
    plist.insert("CFBundleIdentifier".into(), text(context.id()));
    plist.insert("CFBundleInfoDictionaryVersion".into(), text("6.0"));
    plist.insert("CFBundleName".into(), text(&name));
    plist.insert("CFBundlePackageType".into(), text("APPL"));
    plist.insert(
        "CFBundleShortVersionString".into(),
        text(short_version(context.version())),
    );
    plist.insert("CFBundleVersion".into(), text(context.build_number()));
    if let Some(min) = &macos.min_system {
        plist.insert("LSMinimumSystemVersion".into(), text(min));
    }
    if let Some(category) = &macos.category {
        plist.insert("LSApplicationCategoryType".into(), text(category));
    }
    if macos.high_resolution {
        plist.insert("NSHighResolutionCapable".into(), Value::Boolean(true));
    }
    if let Some(copyright) = &project.config.app.copyright {
        plist.insert(
            "NSHumanReadableCopyright".into(),
            text(context.fill(copyright)?),
        );
    }
    if macos.icon.is_some() {
        plist.insert("CFBundleIconFile".into(), text(&name));
    }
    if let Some(icon) = &macos.icon_name {
        plist.insert("CFBundleIconName".into(), text(icon));
    }
    if !project.config.app.url_schemes.is_empty() {
        let mut entry = Dictionary::new();
        entry.insert("CFBundleURLName".into(), text(context.id()));
        entry.insert(
            "CFBundleURLSchemes".into(),
            texts(&project.config.app.url_schemes),
        );
        plist.insert(
            "CFBundleURLTypes".into(),
            Value::Array(vec![Value::Dictionary(entry)]),
        );
    }
    if !project.config.document_types.is_empty() {
        plist.insert(
            "CFBundleDocumentTypes".into(),
            Value::Array(
                project
                    .config
                    .document_types
                    .iter()
                    .map(document)
                    .map(Value::Dictionary)
                    .collect(),
            ),
        );
    }
    for (key, declarations) in [
        ("UTExportedTypeDeclarations", &macos.exported_types),
        ("UTImportedTypeDeclarations", &macos.imported_types),
    ] {
        if !declarations.is_empty() {
            plist.insert(
                key.into(),
                Value::Array(
                    declarations
                        .iter()
                        .map(declaration)
                        .map(Value::Dictionary)
                        .collect(),
                ),
            );
        }
    }
    for (key, value) in context.plist_table(&macos.plist)? {
        plist.insert(key, value);
    }
    if let Some(variant) = project.variant() {
        apply_variant(&mut plist, project, variant)?;
    }
    Ok(plist)
}

/// The main executable's name in `Contents/MacOS`.
///
/// # Errors
///
/// [`crate::Error::Value`].
pub fn executable_name(project: &Project) -> Result<String> {
    let macos = project.macos()?;
    match &macos.executable.name {
        Some(name) => project.context.fill(name),
        None => Ok(project.context.name().to_owned()),
    }
}

/// `CFBundleShortVersionString` takes three numbers: `1.2.0-beta.1` is `1.2.0`.
pub fn short_version(version: &str) -> String {
    version
        .split(['-', '+'])
        .next()
        .unwrap_or(version)
        .to_owned()
}

fn document(document: &DocumentType) -> Dictionary {
    let mut entry = Dictionary::new();
    entry.insert("CFBundleTypeName".into(), text(&document.name));
    if !document.extensions.is_empty() {
        entry.insert("CFBundleTypeExtensions".into(), texts(&document.extensions));
    }
    if !document.content_types.is_empty() {
        entry.insert("LSItemContentTypes".into(), texts(&document.content_types));
    }
    entry.insert(
        "CFBundleTypeRole".into(),
        text(match document.role {
            Role::Editor => "Editor",
            Role::Viewer => "Viewer",
            Role::Shell => "Shell",
            Role::None => "None",
        }),
    );
    entry.insert("LSHandlerRank".into(), text(rank(document.rank)));
    if let Some(icon) = &document.icon {
        entry.insert("CFBundleTypeIconFile".into(), text(icon));
    }
    entry
}

fn rank(rank: Rank) -> &'static str {
    match rank {
        Rank::Owner => "Owner",
        Rank::Default => "Default",
        Rank::Alternate => "Alternate",
        Rank::None => "None",
    }
}

fn declaration(declaration: &TypeDeclaration) -> Dictionary {
    let mut entry = Dictionary::new();
    entry.insert("UTTypeIdentifier".into(), text(&declaration.identifier));
    if let Some(description) = &declaration.description {
        entry.insert("UTTypeDescription".into(), text(description));
    }
    if !declaration.conforms_to.is_empty() {
        entry.insert("UTTypeConformsTo".into(), texts(&declaration.conforms_to));
    }
    let mut tags = Dictionary::new();
    if !declaration.extensions.is_empty() {
        tags.insert(
            "public.filename-extension".into(),
            texts(&declaration.extensions),
        );
    }
    if !declaration.mime_types.is_empty() {
        tags.insert("public.mime-type".into(), texts(&declaration.mime_types));
    }
    if !tags.is_empty() {
        entry.insert("UTTypeTagSpecification".into(), Value::Dictionary(tags));
    }
    if let Some(icon) = &declaration.icon {
        entry.insert("UTTypeIconFile".into(), text(icon));
    }
    entry
}

/// A variant's changes, applied to the whole plist (template included), so a
/// variant takes nothing from the application it installs beside.
fn apply_variant(
    plist: &mut Dictionary,
    project: &Project,
    variant: &crate::config::Variant,
) -> Result<()> {
    if let Some(new_rank) = variant.document_rank
        && let Some(Value::Array(documents)) = plist.get_mut("CFBundleDocumentTypes")
    {
        for document in documents {
            if let Value::Dictionary(document) = document {
                document.insert("LSHandlerRank".into(), text(rank(new_rank)));
            }
        }
    }
    if variant.types == Some(TypeOwnership::Imported)
        && let Some(Value::Array(exported)) = plist.remove("UTExportedTypeDeclarations")
    {
        let mut imported = match plist.remove("UTImportedTypeDeclarations") {
            Some(Value::Array(imported)) => imported,
            _ => Vec::new(),
        };
        imported.extend(exported);
        plist.insert("UTImportedTypeDeclarations".into(), Value::Array(imported));
    }
    if let Some(schemes) = &variant.url_schemes {
        if schemes.is_empty() {
            plist.remove("CFBundleURLTypes");
        } else {
            let mut entry = Dictionary::new();
            entry.insert("CFBundleURLName".into(), text(project.context.id()));
            entry.insert("CFBundleURLSchemes".into(), texts(schemes));
            plist.insert(
                "CFBundleURLTypes".into(),
                Value::Array(vec![Value::Dictionary(entry)]),
            );
        }
    }
    for (key, value) in project.context.plist_table(&variant.plist)? {
        plist.insert(key, value);
    }
    Ok(())
}
