//! The data shape of a look (`themes.py`: `TextBox`, `ThemeSlide`, `Looks`): which theme slides the verse and point slides take
//! their layout from. Only the shape is here: reading ProPresenter theme files and choosing the slides is S6, so a plan made
//! without themes has `looks: None` (and so has every oracle case).

use serde::{Deserialize, Serialize};

/// A text box of a theme slide.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextBox {
    pub name: String,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    #[serde(default)]
    pub font: String,
    #[serde(default)]
    pub size: f64,
}

/// One slide of a ProPresenter theme.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThemeSlide {
    pub group: String,
    pub theme: String,
    pub name: String,
    /// 1-based within its theme.
    pub number: u32,
    pub path: String,
    #[serde(default)]
    pub boxes: Vec<TextBox>,
    /// Shapes, images, video backgrounds.
    #[serde(default)]
    pub other_objects: u32,
    pub size: (u32, u32),
}

impl ThemeSlide {
    /// `theme_label`: `group/theme`, or the theme alone.
    pub fn theme_label(&self) -> String {
        if self.group.is_empty() {
            self.theme.clone()
        } else {
            format!("{}/{}", self.group, self.theme)
        }
    }

    /// `label`: `group/theme · slide name` (`Slide N` when the slide has no name).
    pub fn label(&self) -> String {
        let name = if self.name.is_empty() {
            format!("Slide {}", self.number)
        } else {
            self.name.clone()
        };
        format!("{} · {name}", self.theme_label())
    }
}

/// The theme slides chosen for verses and for points.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Looks {
    pub verse: Option<ThemeSlide>,
    pub point: Option<ThemeSlide>,
}

/// The `describe()` of a look as the review report shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LooksDescription {
    pub verse: String,
    pub point: String,
}

impl Looks {
    pub fn describe(&self) -> LooksDescription {
        LooksDescription {
            verse: self
                .verse
                .as_ref()
                .map(ThemeSlide::label)
                .unwrap_or_default(),
            point: self
                .point
                .as_ref()
                .map(ThemeSlide::label)
                .unwrap_or_default(),
        }
    }
}
