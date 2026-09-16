use anyhow::{Result, bail};
use fontique::{
    Attributes, Blob, Collection, CollectionOptions, FontStyle, FontWeight, FontWidth,
    GenericFamily, QueryFamily, QueryFont, QueryStatus, SourceCache,
};
use parley::FontContext;

/// Controls whether a Parley text system loads operating-system fonts.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SystemFonts {
    /// Enumerate fonts installed on the operating system.
    #[default]
    Load,
    /// Start with an empty catalog. Applications can still register font data.
    Skip,
}

pub(crate) fn new_font_context(system_fonts: SystemFonts) -> FontContext {
    FontContext {
        collection: Collection::new(CollectionOptions {
            shared: false,
            system_fonts: system_fonts == SystemFonts::Load,
        }),
        source_cache: SourceCache::default(),
    }
}

pub(crate) fn validate_font_blobs(fonts: &[Blob<u8>]) -> Result<()> {
    let mut validator = Collection::new(CollectionOptions {
        shared: false,
        system_fonts: false,
    });

    for blob in fonts {
        if validator.register_fonts(blob.clone(), None).is_empty() {
            bail!("font data did not contain a supported font face");
        }
    }

    Ok(())
}

pub(crate) fn register_font_blobs(context: &mut FontContext, fonts: &[Blob<u8>]) {
    for blob in fonts {
        context.collection.register_fonts(blob.clone(), None);
    }
}

#[cfg(test)]
fn register_bytes(context: &mut FontContext, fonts: &[&[u8]]) -> Result<()> {
    let blobs = fonts
        .iter()
        .map(|bytes| Blob::from(bytes.to_vec()))
        .collect::<Vec<_>>();

    validate_font_blobs(&blobs)?;
    register_font_blobs(context, &blobs);

    Ok(())
}

/// Returns the available family names in stable display order.
pub(crate) fn family_names(context: &mut FontContext) -> Vec<String> {
    let mut names = context
        .collection
        .family_names()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    names.sort_unstable();
    names.dedup();

    names
}

/// Font attributes used for direct Fontique queries.
#[derive(Clone, Copy, Debug)]
pub(crate) struct FaceRequest<'a> {
    /// Ordered font families to query.
    pub(crate) families: &'a [FaceFamily<'a>],
    /// OpenType weight, normally in the range 1 through 1000.
    pub(crate) weight: f32,
    /// Requested style.
    pub(crate) style: gpui::FontStyle,
    /// Optional character that the selected face must cover.
    pub(crate) character: Option<char>,
}

/// A named or generic family used for direct font resolution.
#[derive(Clone, Copy, Debug)]
pub(crate) enum FaceFamily<'a> {
    /// A concrete family name.
    Named(&'a str),
    /// The platform's user-interface font.
    SystemUi,
}

pub(crate) fn resolve_face(
    context: &mut FontContext,
    request: &FaceRequest<'_>,
) -> Option<QueryFont> {
    let style = match request.style {
        gpui::FontStyle::Normal => FontStyle::Normal,
        gpui::FontStyle::Italic => FontStyle::Italic,
        gpui::FontStyle::Oblique => FontStyle::Oblique(None),
    };

    let mut query = context.collection.query(&mut context.source_cache);
    query.set_families(request.families.iter().map(|family| match family {
        FaceFamily::Named(name) => QueryFamily::Named(name),
        FaceFamily::SystemUi => QueryFamily::Generic(GenericFamily::SystemUi),
    }));
    query.set_attributes(Attributes::new(
        FontWidth::NORMAL,
        style,
        FontWeight::new(request.weight),
    ));

    let mut selected = None;
    query.matches_with(|font| {
        if request.character.is_some_and(|character| {
            font.charmap()
                .and_then(|charmap| charmap.map(character))
                .is_none()
        }) {
            return QueryStatus::Continue;
        }

        selected = Some(font.clone());

        QueryStatus::Stop
    });

    selected
}

#[cfg(test)]
mod tests {
    use super::*;

    const IBM_PLEX: &[u8] =
        include_bytes!("../../../assets/fonts/ibm-plex-sans/IBMPlexSans-Regular.ttf");
    const IBM_PLEX_SEMIBOLD_ITALIC: &[u8] =
        include_bytes!("../../../assets/fonts/ibm-plex-sans/IBMPlexSans-SemiBoldItalic.ttf");
    const LILEX: &[u8] = include_bytes!("../../../assets/fonts/lilex/Lilex-Regular.ttf");

    #[test]
    fn registered_fonts_are_enumerated_and_resolved() {
        let mut context = new_font_context(SystemFonts::Skip);
        register_bytes(&mut context, &[IBM_PLEX, IBM_PLEX_SEMIBOLD_ITALIC, LILEX]).unwrap();

        assert_eq!(family_names(&mut context), ["IBM Plex Sans", "Lilex"]);

        let mut resolve = |request| resolve_face(&mut context, request);
        let latin = resolve(&FaceRequest {
            families: &[FaceFamily::Named("IBM Plex Sans")],
            weight: 400.0,
            style: gpui::FontStyle::Normal,
            character: Some('m'),
        })
        .unwrap();
        assert_eq!(latin.blob.as_ref(), IBM_PLEX);
        assert_eq!(latin.index, 0);

        let semibold_italic = resolve(&FaceRequest {
            families: &[FaceFamily::Named("IBM Plex Sans")],
            weight: 600.0,
            style: gpui::FontStyle::Italic,
            character: None,
        })
        .unwrap();
        assert_eq!(semibold_italic.blob.as_ref(), IBM_PLEX_SEMIBOLD_ITALIC);

        assert!(
            resolve(&FaceRequest {
                families: &[FaceFamily::Named("IBM Plex Sans")],
                weight: 400.0,
                style: gpui::FontStyle::Normal,
                character: Some('\u{1F9A5}'),
            })
            .is_none()
        );
    }

    #[test]
    fn font_registration_is_atomic() {
        let mut context = new_font_context(SystemFonts::Skip);
        register_bytes(&mut context, &[LILEX]).unwrap();
        let families_before = family_names(&mut context);

        assert!(register_bytes(&mut context, &[IBM_PLEX, b"not a font"]).is_err());
        assert_eq!(family_names(&mut context), families_before);

        assert!(
            resolve_face(
                &mut context,
                &FaceRequest {
                    families: &[FaceFamily::Named("IBM Plex Sans")],
                    weight: 400.0,
                    style: gpui::FontStyle::Normal,
                    character: None,
                },
            )
            .is_none()
        );
    }
}
