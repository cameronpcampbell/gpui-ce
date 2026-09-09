use super::*;

fn braced_declaration<'a>(source: &'a str, declaration: &str) -> &'a str {
    let start = source
        .find(declaration)
        .unwrap_or_else(|| panic!("missing shader declaration {declaration}"));
    let body_start = source[start..]
        .find('{')
        .map(|offset| start + offset)
        .unwrap_or_else(|| panic!("shader declaration {declaration} has no body"));
    let mut depth = 0_u32;
    for (offset, character) in source[body_start..].char_indices() {
        match character {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &source[start..body_start + offset + 1];
                }
            }
            _ => {}
        }
    }
    panic!("shader declaration {declaration} has an unterminated body")
}

#[test]
fn validates_subpixel_shader() {
    let generated = subpixel_sprite::WGSL_SOURCE.wgsl_source().unwrap();
    wgsl_rs::validate_wgsl_source(&format!("enable dual_source_blending;\n{generated}")).unwrap();
}

#[test]
fn build_generated_shaders_match_rust_sources() {
    assert_eq!(
        base::WGSL_SOURCE.wgsl_source().unwrap(),
        crate::artifacts::BASE_WGSL
    );
    let subpixel = subpixel_sprite::WGSL_SOURCE.wgsl_source().unwrap();
    assert_eq!(
        format!("enable dual_source_blending;\n{subpixel}"),
        crate::artifacts::SUBPIXEL_DUAL_SOURCE_WGSL
    );
}

#[test]
fn shader_interface_matches_generated_sources() {
    let standard_source = base::WGSL_SOURCE.wgsl_source().unwrap();
    let subpixel_source = subpixel_sprite::WGSL_SOURCE.wgsl_source().unwrap();
    for pipeline in interface::ALL {
        let source = if pipeline.label == interface::SUBPIXEL_SPRITES.label {
            &subpixel_source
        } else {
            &standard_source
        };
        assert!(
            source.contains(&format!("fn {}(", pipeline.vertex_entry)),
            "missing vertex entry point {}",
            pipeline.vertex_entry,
        );
        assert!(
            source.contains(&format!("fn {}(", pipeline.fragment_entry)),
            "missing fragment entry point {}",
            pipeline.fragment_entry,
        );
    }

    assert_eq!(common::GLOBALS.group, interface::GLOBAL_BIND_GROUP);
    assert_eq!(common::GLOBALS.binding, interface::GLOBAL_UNIFORMS_BINDING);
    assert_eq!(
        common::FONT_RASTERIZATION.group,
        interface::GLOBAL_BIND_GROUP
    );
    assert_eq!(
        common::FONT_RASTERIZATION.binding,
        interface::FONT_RASTERIZATION_BINDING
    );
    assert_eq!(quad::QUADS.group(), interface::DATA_BIND_GROUP);
    assert_eq!(quad::QUADS.binding(), interface::DATA_BUFFER_BINDING);
}

#[test]
fn ordinary_entry_points_keep_compact_corner_geometry() {
    let source = base::WGSL_SOURCE.wgsl_source().unwrap();
    for entry in [
        "fn vertex_quad(",
        "fn fragment_quad(",
        "fn vertex_shadow(",
        "fn fragment_shadow(",
        "fn vertex_polychrome_sprite(",
        "fn fragment_polychrome_sprite(",
        "fn vertex_blur_composite(",
        "fn fragment_blur_composite(",
    ] {
        let declaration = braced_declaration(&source, entry);
        for expanded_operation in [
            "prepare_corners",
            "prepared_corner_signed_distance",
            "figma_smooth_rectangle_sample",
            "corner_smoothing",
        ] {
            assert!(
                !declaration.contains(expanded_operation),
                "ordinary shader {entry} contains {expanded_operation}"
            );
        }
    }

    for varying in [
        "struct QuadVarying",
        "struct ShadowVarying",
        "struct PolychromeSpriteVarying",
        "struct BlurVarying",
    ] {
        let declaration = braced_declaration(&source, varying);
        for expanded_varying in [
            "horizontal_corner_reaches",
            "vertical_corner_reaches",
            "smoothing_factors",
            "superellipse_power",
        ] {
            assert!(
                !declaration.contains(expanded_varying),
                "ordinary shader {varying} contains {expanded_varying}"
            );
        }
    }
}

#[test]
fn shader_buffer_layouts_match_host_layouts() {
    let modules = [
        crate::artifacts::BASE_WGSL,
        crate::artifacts::SUBPIXEL_DUAL_SOURCE_WGSL,
    ]
    .map(|source| naga::front::wgsl::parse_str(source).unwrap());

    for layouts in [gpui::SCENE_BUFFER_LAYOUTS, interface::RENDER_BUFFER_LAYOUTS] {
        for layout in layouts {
            let ty = modules
                .iter()
                .find_map(|module| {
                    module
                        .types
                        .iter()
                        .find_map(|(_, ty)| (ty.name.as_deref() == Some(layout.name)).then_some(ty))
                })
                .unwrap_or_else(|| panic!("missing shader ABI type {}", layout.name));
            let naga::TypeInner::Struct { members, span } = &ty.inner else {
                panic!("shader ABI type {} must be a struct", layout.name);
            };

            assert_eq!(
                *span as usize, layout.size,
                "shader ABI size: {}",
                layout.name
            );
            assert_eq!(
                members.len(),
                layout.fields.len(),
                "shader ABI fields: {}",
                layout.name
            );
            for (member, (name, offset)) in members.iter().zip(layout.fields) {
                assert_eq!(
                    member.name.as_deref(),
                    Some(*name),
                    "shader ABI field: {}",
                    layout.name
                );
                assert_eq!(
                    member.offset as usize, *offset,
                    "shader ABI offset: {}.{name}",
                    layout.name
                );
            }
        }
    }
}

#[test]
fn shader_discriminants_match_scene_types() {
    assert_eq!(common::ShaderBool::Disabled as u32, 0);
    assert_eq!(common::ShaderBool::Enabled as u32, 1);
    assert_eq!(
        common::BackgroundTag::Solid as u32,
        gpui::BackgroundTag::Solid as u32
    );
    assert_eq!(
        common::BackgroundTag::LinearGradient as u32,
        gpui::BackgroundTag::LinearGradient as u32
    );
    assert_eq!(
        common::BackgroundTag::PatternSlash as u32,
        gpui::BackgroundTag::PatternSlash as u32
    );
    assert_eq!(
        common::BackgroundTag::Checkerboard as u32,
        gpui::BackgroundTag::Checkerboard as u32
    );
    assert_eq!(
        common::ColorSpace::Srgb as u32,
        gpui::ColorSpace::Srgb as u32
    );
    assert_eq!(
        common::ColorSpace::Oklab as u32,
        gpui::ColorSpace::Oklab as u32
    );
    assert_eq!(
        common::BorderStyle::Solid as u32,
        gpui::BorderStyle::Solid as u32
    );
    assert_eq!(
        common::BorderStyle::Dashed as u32,
        gpui::BorderStyle::Dashed as u32
    );
}

#[test]
fn linear_gradients_preserve_native_dithering() {
    use common::*;
    use wgsl_rs::std::*;

    let color = Hsla {
        h: 0.0,
        s: 0.0,
        l: 0.5,
        a: 0.5,
    };
    let background = Background {
        tag: BackgroundTag::LinearGradient,
        color_space: ColorSpace::Srgb,
        solid: color,
        gradient_angle_or_pattern_height: 90.0,
        colors: [
            LinearColorStop {
                color,
                percentage: 0.0,
            },
            LinearColorStop {
                color,
                percentage: 1.0,
            },
        ],
        padding: 0,
    };
    let bounds = Bounds {
        origin: vec2f(0.0, 0.0),
        size: vec2f(100.0, 100.0),
    };
    let prepared = prepare_background(background);
    let first = background_color(background, vec2f(10.0, 10.0), bounds, prepared);
    let second = background_color(background, vec2f(11.0, 10.0), bounds, prepared);

    assert_ne!(first.w, second.w);
}
