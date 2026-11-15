use super::*;

impl From<bool> for common::ShaderBool {
    fn from(value: bool) -> Self {
        if value { Self::Enabled } else { Self::Disabled }
    }
}

impl From<gpui::Bounds<gpui::ScaledPixels>> for common::Bounds {
    fn from(bounds: gpui::Bounds<gpui::ScaledPixels>) -> Self {
        Self {
            origin: wgsl_rs::std::vec2f(bounds.origin.x.0, bounds.origin.y.0),
            size: wgsl_rs::std::vec2f(bounds.size.width.0, bounds.size.height.0),
        }
    }
}

impl From<gpui::Edges<gpui::ScaledPixels>> for common::Edges {
    fn from(edges: gpui::Edges<gpui::ScaledPixels>) -> Self {
        Self {
            top: edges.top.0,
            right: edges.right.0,
            bottom: edges.bottom.0,
            left: edges.left.0,
        }
    }
}

impl From<gpui::ContentMask<gpui::ScaledPixels>> for common::ContentMask {
    fn from(mask: gpui::ContentMask<gpui::ScaledPixels>) -> Self {
        Self {
            bounds: mask.bounds.into(),
            fade_out: mask.fade_out.into(),
        }
    }
}

impl From<gpui::Corners<gpui::ScaledPixels>> for common::Corners {
    fn from(corners: gpui::Corners<gpui::ScaledPixels>) -> Self {
        Self {
            top_left: corners.top_left.0,
            top_right: corners.top_right.0,
            bottom_right: corners.bottom_right.0,
            bottom_left: corners.bottom_left.0,
        }
    }
}

impl From<gpui::SceneHsla> for common::Hsla {
    fn from(color: gpui::SceneHsla) -> Self {
        let color: gpui::Hsla = color.into();

        Self {
            h: color.hue.into_positive_degrees() / 360.0,
            s: color.saturation,
            l: color.lightness,
            a: color.alpha,
        }
    }
}

impl From<gpui::Background> for common::Background {
    fn from(background: gpui::Background) -> Self {
        let empty = common::Hsla {
            h: 0.0,
            s: 0.0,
            l: 0.0,
            a: 0.0,
        };
        let mut result = Self {
            tag: common::BackgroundTag::Solid,
            color_space: match background.interpolation_space() {
                gpui::ColorSpace::Srgb => common::ColorSpace::Srgb,
                gpui::ColorSpace::Oklab => common::ColorSpace::Oklab,
            },
            solid: empty,
            gradient_angle_or_pattern_height: 0.0,
            colors: [common::LinearColorStop {
                color: empty,
                percentage: 0.0,
            }; 2],
            padding: 0,
        };

        match background.kind() {
            gpui::BackgroundKind::Solid(color) => {
                result.solid = gpui::SceneHsla::from(color).into();
            }
            gpui::BackgroundKind::LinearGradient { angle, stops } => {
                result.tag = common::BackgroundTag::LinearGradient;
                result.gradient_angle_or_pattern_height = angle;
                result.colors = stops.map(|stop| common::LinearColorStop {
                    color: stop.color.into(),
                    percentage: stop.percentage,
                });
            }
            gpui::BackgroundKind::PatternSlash {
                color,
                width,
                interval,
            } => {
                result.tag = common::BackgroundTag::PatternSlash;
                result.solid = gpui::SceneHsla::from(color).into();
                result.gradient_angle_or_pattern_height =
                    (width * 255.0).round() * 65535.0 + (interval * 255.0).round();
            }
            gpui::BackgroundKind::Checkerboard { color, size } => {
                result.tag = common::BackgroundTag::Checkerboard;
                result.solid = gpui::SceneHsla::from(color).into();
                result.gradient_angle_or_pattern_height = size;
            }
        }

        result
    }
}
