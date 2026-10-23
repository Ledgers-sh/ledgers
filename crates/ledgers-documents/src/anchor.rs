//! Document field anchors: locations of semantic fields within PDFs or e-invoices.
//!
//! Each anchor links a field name (e.g., `"total"`, `"invoice_number"`) to its
//! location, either as a page-based bounding box or an XML path for e-invoices.
//! The bounding box is in 1/10000 units (renderer-independent; works for both
//! PDF.js and page images) with origin at top-left. A page anchor applies to
//! one page; XML anchors target structured documents (`CII`, `UBL`, `ZUGFeRD`).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A semantic field location within a document.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Anchor {
    /// The field name (e.g., `"total"`, `"invoice_number"`, `"lines.0.amount"`).
    /// Non-empty, bounded length (255 chars).
    pub field: String,
    /// Where the field is located.
    pub location: Location,
}

/// The location of a field: either a page-relative bounding box or an XML path.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Location {
    /// A page-based location: page number (1-based) and a bounding box.
    #[serde(rename_all = "snake_case")]
    Page {
        /// Page number, 1-based.
        page: u32,
        /// The bounding box.
        bbox: BoundingBox,
    },
    /// An XML path within a structured document.
    #[serde(rename_all = "snake_case")]
    Xml {
        /// `XPath` or simplified path (e.g., `/Invoice/TaxAmount[1]`).
        /// Non-empty, bounded length (500 chars).
        path: String,
    },
}

/// A bounding box in 1/10000 units of the page size, origin top-left.
/// All values are integers; a 1240×1754 px A4 page at 96 DPI translates to
/// approximately (0,0,10000,10000) in normalized units.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BoundingBox {
    /// X-offset in 1/10000 units (0..=10000).
    pub x: u32,
    /// Y-offset in 1/10000 units (0..=10000).
    pub y: u32,
    /// Width in 1/10000 units (>0, x+w ≤ 10000).
    pub w: u32,
    /// Height in 1/10000 units (>0, y+h ≤ 10000).
    pub h: u32,
}

impl BoundingBox {
    /// Validates the bounding box.
    ///
    /// # Errors
    /// When any coordinate is out of range or the box extends past the page.
    pub fn validate(&self) -> Result<(), String> {
        if self.x > 10000 {
            return Err("x must be ≤ 10000".to_owned());
        }
        if self.y > 10000 {
            return Err("y must be ≤ 10000".to_owned());
        }
        if self.w == 0 {
            return Err("w must be > 0".to_owned());
        }
        if self.h == 0 {
            return Err("h must be > 0".to_owned());
        }
        if self.x.saturating_add(self.w) > 10000 {
            return Err("x + w must be ≤ 10000".to_owned());
        }
        if self.y.saturating_add(self.h) > 10000 {
            return Err("y + h must be ≤ 10000".to_owned());
        }
        Ok(())
    }

    /// Maps this normalized bounding box onto a rendered page of the given
    /// pixel dimensions. Returns `(x, y, width, height)` in pixels, using
    /// integer arithmetic: left/top = floor, right/bottom = ceil, width/height = right-left,
    /// so the pixel box always covers the normalized source region.
    ///
    /// For a 1240×1754 px A4 page:
    /// - bbox (0, 0, 10000, 10000) → (0, 0, 1240, 1754)
    /// - bbox (5000, 0, 5000, 5000) → (620, 0, 620, 877)
    /// - bbox (1000, 2000, 3000, 500) → (124, 350, 372, 89)
    ///   where `x_right` = ceil((1000+3000)*1240/10000) = ceil(496.0) = 496;
    ///   w = 496 - 124 = 372; `y_bottom` = ceil((2000+500)*1754/10000) = ceil(438.5) = 439;
    ///   h = 439 - 350 = 89.
    #[must_use]
    #[allow(clippy::cast_possible_truncation)]
    pub fn on_page(self, width_px: u32, height_px: u32) -> PixelRect {
        let w = u64::from(width_px);
        let h = u64::from(height_px);
        // left = floor(x * W / 10000), right = ceil((x + w) * W / 10000)
        let x_px = ((u64::from(self.x) * w) / 10000) as u32;
        let x_right = {
            let scaled = u64::from(self.x) * w + u64::from(self.w) * w;
            (scaled.div_ceil(10000)) as u32
        };
        let w_px = x_right - x_px;
        // top = floor(y * H / 10000), bottom = ceil((y + h) * H / 10000)
        let y_px = ((u64::from(self.y) * h) / 10000) as u32;
        let y_bottom = {
            let scaled = u64::from(self.y) * h + u64::from(self.h) * h;
            (scaled.div_ceil(10000)) as u32
        };
        let h_px = y_bottom - y_px;
        PixelRect {
            x: x_px,
            y: y_px,
            w: w_px,
            h: h_px,
        }
    }
}

/// A bounding box in pixel coordinates, for rendering.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PixelRect {
    /// X-offset in pixels.
    pub x: u32,
    /// Y-offset in pixels.
    pub y: u32,
    /// Width in pixels.
    pub w: u32,
    /// Height in pixels.
    pub h: u32,
}

impl Anchor {
    /// Validates the anchor: field non-empty/bounded, location valid.
    ///
    /// # Errors
    /// When validation fails.
    pub fn validate(&self) -> Result<(), String> {
        if self.field.is_empty() {
            return Err("field must not be empty".to_owned());
        }
        if self.field.len() > 255 {
            return Err("field must be ≤ 255 chars".to_owned());
        }
        match &self.location {
            Location::Page { page, bbox } => {
                if *page == 0 {
                    return Err("page must be ≥ 1".to_owned());
                }
                bbox.validate()?;
            }
            Location::Xml { path } => {
                if path.is_empty() {
                    return Err("xml path must not be empty".to_owned());
                }
                if path.len() > 500 {
                    return Err("xml path must be ≤ 500 chars".to_owned());
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bbox_on_page_scales_correctly() {
        // A4 at 96 DPI: 1240×1754 px
        let a4_px = (1240, 1754);
        let bbox = BoundingBox {
            x: 0,
            y: 0,
            w: 10000,
            h: 10000,
        };
        let rect = bbox.on_page(a4_px.0, a4_px.1);
        assert_eq!(rect.x, 0);
        assert_eq!(rect.y, 0);
        assert_eq!(rect.w, 1240);
        assert_eq!(rect.h, 1754);

        // Half the page from the right, top half
        let bbox2 = BoundingBox {
            x: 5000,
            y: 0,
            w: 5000,
            h: 5000,
        };
        let rect2 = bbox2.on_page(a4_px.0, a4_px.1);
        assert_eq!(rect2.x, 620);
        assert_eq!(rect2.y, 0);
        assert_eq!(rect2.w, 620);
        assert_eq!(rect2.h, 877);

        // Specific test case: x=1000,y=2000,w=3000,h=500 on 1240×1754
        // x: floor(1000*1240/10000) = floor(124.0) = 124
        // x_right: ceil((1000+3000)*1240/10000) = ceil(496.0) = 496, w=496-124=372
        // y: floor(2000*1754/10000) = floor(350.8) = 350
        // y_bottom: ceil((2000+500)*1754/10000) = ceil(438.5) = 439, h=439-350=89
        let bbox3 = BoundingBox {
            x: 1000,
            y: 2000,
            w: 3000,
            h: 500,
        };
        let rect3 = bbox3.on_page(a4_px.0, a4_px.1);
        assert_eq!(rect3.x, 124);
        assert_eq!(rect3.y, 350);
        assert_eq!(rect3.w, 372);
        assert_eq!(rect3.h, 89);
    }

    #[test]
    fn bbox_validation_rejects_out_of_range() {
        let bad = BoundingBox {
            x: 10001,
            y: 0,
            w: 1,
            h: 1,
        };
        assert!(bad.validate().is_err());

        let bad2 = BoundingBox {
            x: 9999,
            y: 0,
            w: 2,
            h: 1,
        };
        assert!(bad2.validate().is_err());

        let bad3 = BoundingBox {
            x: 0,
            y: 0,
            w: 0,
            h: 1,
        };
        assert!(bad3.validate().is_err());
    }

    #[test]
    fn anchor_validation_checks_field_and_location() {
        let valid = Anchor {
            field: "total".to_owned(),
            location: Location::Xml {
                path: "/Invoice/Total".to_owned(),
            },
        };
        assert!(valid.validate().is_ok());

        let empty_field = Anchor {
            field: String::new(),
            location: Location::Xml {
                path: "/x".to_owned(),
            },
        };
        assert!(empty_field.validate().is_err());

        let empty_path = Anchor {
            field: "total".to_owned(),
            location: Location::Xml {
                path: String::new(),
            },
        };
        assert!(empty_path.validate().is_err());
    }
}
