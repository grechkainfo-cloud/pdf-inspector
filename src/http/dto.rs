//! Request options and response bodies for the HTTP API.
//!
//! The core crate deliberately has no serde dependency, so the wire format
//! lives here as a thin mirror of the library types. Every response field maps
//! 1:1 onto a field of the corresponding `pdf_inspector` result struct.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;

use crate::detector::{DetectionConfig, PdfType, ScanStrategy};
use crate::markdown::{MarkdownOptions, MarkdownProfile};
use crate::types::ItemType;
use crate::{PdfOptions, ProcessMode};

use super::error::ApiError;

/// Default DPI assumed for TSR crops when the caller omits `render_dpi`.
fn default_render_dpi() -> f32 {
    200.0
}

// =========================================================================
// Page selection
// =========================================================================

/// Pages accepted either as a list (`[1, 3, 5]`), a single number (`3`), or a
/// CLI-style spec string (`"1,3,5-10"`).
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum PageSelection {
    List(Vec<u32>),
    Single(u32),
    Spec(String),
}

impl PageSelection {
    /// Resolve to concrete page numbers.
    ///
    /// `zero_indexed` selects which floor is legal: endpoints mirroring the
    /// 1-indexed library calls reject page 0, the per-page markdown endpoint
    /// (0-indexed, like `extract_pages_markdown`) accepts it.
    pub fn resolve(&self, zero_indexed: bool) -> Result<Vec<u32>, ApiError> {
        let pages = match self {
            Self::List(pages) => pages.clone(),
            Self::Single(page) => vec![*page],
            Self::Spec(spec) => parse_page_spec(spec)?,
        };
        if !zero_indexed && pages.iter().any(|page| *page == 0) {
            return Err(ApiError::bad_request(
                "page numbers are 1-indexed for this endpoint",
            ));
        }
        Ok(pages)
    }
}

/// Parse `"1,3,5-10"` into a sorted, de-duplicated page list.
fn parse_page_spec(spec: &str) -> Result<Vec<u32>, ApiError> {
    let mut pages: Vec<u32> = Vec::new();
    for part in spec.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some((start, end)) = part.split_once('-') {
            let start: u32 = start
                .trim()
                .parse()
                .map_err(|_| ApiError::bad_request(format!("invalid page number: {start}")))?;
            let end: u32 = end
                .trim()
                .parse()
                .map_err(|_| ApiError::bad_request(format!("invalid page number: {end}")))?;
            if start > end {
                return Err(ApiError::bad_request(format!("invalid range: {start}-{end}")));
            }
            pages.extend(start..=end);
        } else {
            pages.push(
                part.parse()
                    .map_err(|_| ApiError::bad_request(format!("invalid page number: {part}")))?,
            );
        }
    }
    pages.sort_unstable();
    pages.dedup();
    Ok(pages)
}

// =========================================================================
// Request options
// =========================================================================

/// Options for `/v1/process`, `/v1/detect`, and `/v1/analyze`.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ProcessOptions {
    /// Override the endpoint's processing depth: `detect`, `analyze`, `full`.
    pub mode: Option<String>,
    /// 1-indexed pages to process. Omit for the whole document.
    pub pages: Option<PageSelection>,
    /// Password for an encrypted PDF.
    pub password: Option<String>,
    /// `json` (default) or `markdown` for the raw markdown body.
    pub format: Option<String>,

    // --- markdown options (flattened for query-string ergonomics) ---
    /// `fidelity` (default) or `compact`.
    pub profile: Option<String>,
    /// Shorthand for `profile=compact`.
    pub compact: Option<bool>,
    pub detect_headers: Option<bool>,
    pub detect_lists: Option<bool>,
    pub detect_code: Option<bool>,
    pub detect_bold: Option<bool>,
    pub detect_italic: Option<bool>,
    pub detect_underline: Option<bool>,
    pub base_font_size: Option<f32>,
    pub remove_page_numbers: Option<bool>,
    pub format_urls: Option<bool>,
    pub fix_hyphenation: Option<bool>,
    pub include_images: Option<bool>,
    pub include_links: Option<bool>,
    /// Insert `<!-- Page N -->` markers between pages.
    #[serde(alias = "page_markers")]
    pub include_page_numbers: Option<bool>,
    pub strip_headers_footers: Option<bool>,

    // --- detection options ---
    /// `early_exit`, `full`, `sample`, or `pages`.
    pub strategy: Option<String>,
    /// Page budget for `strategy=sample` (default 8).
    pub sample_size: Option<u32>,
    /// 1-indexed pages for `strategy=pages`.
    pub detect_pages: Option<PageSelection>,
    pub min_text_ops_per_page: Option<u32>,
    pub text_page_ratio_threshold: Option<f32>,
}

impl ProcessOptions {
    /// Build library options, with `default_mode` used unless `mode` overrides.
    pub fn to_pdf_options(&self, default_mode: ProcessMode) -> Result<PdfOptions, ApiError> {
        let mut options = PdfOptions::new()
            .mode(self.resolve_mode(default_mode)?)
            .markdown(self.markdown_options())
            .detection(self.detection_config()?);

        if let Some(pages) = &self.pages {
            let pages = pages.resolve(false)?;
            options.page_filter = Some(pages.into_iter().collect::<HashSet<u32>>());
        }
        options.password = self.password.clone();
        Ok(options)
    }

    /// Named `resolve_mode`, not `mode`, so it cannot be confused with the
    /// field of the same name.
    fn resolve_mode(&self, default_mode: ProcessMode) -> Result<ProcessMode, ApiError> {
        let Some(mode) = &self.mode else {
            return Ok(default_mode);
        };
        match mode.to_ascii_lowercase().as_str() {
            "detect" | "detect_only" | "detect-only" => Ok(ProcessMode::DetectOnly),
            "analyze" | "analyse" => Ok(ProcessMode::Analyze),
            "full" | "markdown" => Ok(ProcessMode::Full),
            other => Err(ApiError::bad_request(format!(
                "unknown mode `{other}`: expected detect, analyze, or full"
            ))),
        }
    }

    fn markdown_options(&self) -> MarkdownOptions {
        let mut md = MarkdownOptions::default();
        if self.compact == Some(true) {
            md.profile = MarkdownProfile::Compact;
        }
        if let Some(profile) = &self.profile {
            md.profile = match profile.to_ascii_lowercase().as_str() {
                "compact" => MarkdownProfile::Compact,
                _ => MarkdownProfile::Fidelity,
            };
        }
        if let Some(value) = self.detect_headers {
            md.detect_headers = value;
        }
        if let Some(value) = self.detect_lists {
            md.detect_lists = value;
        }
        if let Some(value) = self.detect_code {
            md.detect_code = value;
        }
        if let Some(value) = self.detect_bold {
            md.detect_bold = value;
        }
        if let Some(value) = self.detect_italic {
            md.detect_italic = value;
        }
        if let Some(value) = self.detect_underline {
            md.detect_underline = value;
        }
        if let Some(value) = self.base_font_size {
            md.base_font_size = Some(value);
        }
        if let Some(value) = self.remove_page_numbers {
            md.remove_page_numbers = value;
        }
        if let Some(value) = self.format_urls {
            md.format_urls = value;
        }
        if let Some(value) = self.fix_hyphenation {
            md.fix_hyphenation = value;
        }
        if let Some(value) = self.include_images {
            md.include_images = value;
        }
        if let Some(value) = self.include_links {
            md.include_links = value;
        }
        if let Some(value) = self.include_page_numbers {
            md.include_page_numbers = value;
        }
        if let Some(value) = self.strip_headers_footers {
            md.strip_headers_footers = value;
        }
        md
    }

    fn detection_config(&self) -> Result<DetectionConfig, ApiError> {
        let mut config = DetectionConfig::default();

        let strategy = self.strategy.as_ref().map(|s| s.to_ascii_lowercase());
        match strategy.as_deref() {
            Some("early_exit" | "early-exit" | "earlyexit") => {
                config.strategy = ScanStrategy::EarlyExit;
            }
            Some("full" | "all") => config.strategy = ScanStrategy::Full,
            Some("sample") => {
                config.strategy = ScanStrategy::Sample(self.sample_size.unwrap_or(8).max(1));
            }
            Some("pages") => {
                let pages = self
                    .detect_pages
                    .as_ref()
                    .ok_or_else(|| {
                        ApiError::bad_request("strategy=pages requires `detect_pages`")
                    })?
                    .resolve(false)?;
                config.strategy = ScanStrategy::Pages(pages);
            }
            Some(other) => {
                return Err(ApiError::bad_request(format!(
                    "unknown strategy `{other}`: expected early_exit, full, sample, or pages"
                )));
            }
            None => {
                // `sample_size` alone is enough to mean "sample this many".
                if let Some(size) = self.sample_size {
                    config.strategy = ScanStrategy::Sample(size.max(1));
                }
            }
        }

        if let Some(value) = self.min_text_ops_per_page {
            config.min_text_ops_per_page = value;
        }
        if let Some(value) = self.text_page_ratio_threshold {
            config.text_page_ratio_threshold = value;
        }
        Ok(config)
    }
}

/// Options for `/v1/text` and `/v1/items`.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ExtractOptions {
    /// 1-indexed pages. Omit for the whole document.
    pub pages: Option<PageSelection>,
    /// Password for an encrypted PDF.
    pub password: Option<String>,
    /// `json` (default) or `text` for a `text/plain` body (`/v1/text` only).
    pub format: Option<String>,
}

impl ExtractOptions {
    pub fn page_filter(&self) -> Result<Option<HashSet<u32>>, ApiError> {
        match &self.pages {
            Some(pages) => Ok(Some(pages.resolve(false)?.into_iter().collect())),
            None => Ok(None),
        }
    }
}

/// Options for `/v1/pages/markdown`.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct PagesMarkdownOptions {
    /// **0-indexed** pages, matching `extract_pages_markdown`. Omit for all
    /// pages in document order; when given, output follows the caller's order.
    pub pages: Option<PageSelection>,
}

impl PagesMarkdownOptions {
    /// Named `resolve_pages`, not `pages`, so it cannot be confused with the
    /// field of the same name.
    pub fn resolve_pages(&self) -> Result<Option<Vec<u32>>, ApiError> {
        match &self.pages {
            Some(pages) => Ok(Some(pages.resolve(true)?)),
            None => Ok(None),
        }
    }
}

/// One page's regions, in PDF points with a **top-left** origin.
#[derive(Debug, Deserialize)]
pub struct PageRegionsInput {
    /// 0-indexed page number.
    pub page: u32,
    /// `[x1, y1, x2, y2]` boxes.
    #[serde(alias = "bboxes", alias = "boxes")]
    pub regions: Vec<[f32; 4]>,
}

/// Options for `/v1/regions/text` and `/v1/regions/tables`.
#[derive(Debug, Deserialize)]
pub struct RegionsOptions {
    pub page_regions: Vec<PageRegionsInput>,
}

impl RegionsOptions {
    /// Validate and lower to the library's `(page, boxes)` shape.
    pub fn to_page_regions(&self) -> Result<Vec<(u32, Vec<[f32; 4]>)>, ApiError> {
        self.page_regions
            .iter()
            .map(|entry| {
                for (idx, region) in entry.regions.iter().enumerate() {
                    validate_bbox(region, &format!("page {} region {idx}", entry.page))?;
                }
                Ok((entry.page, entry.regions.clone()))
            })
            .collect()
    }
}

/// Options for `/v1/regions/vector-grid`.
#[derive(Debug, Deserialize)]
pub struct VectorGridOptions {
    /// 0-indexed page number.
    pub page: u32,
    /// `[x1, y1, x2, y2]` in PDF points, top-left origin.
    #[serde(alias = "region_pdf_pt_bbox", alias = "bbox")]
    pub region: [f32; 4],
    /// DPI the crop was (or will be) rendered at.
    #[serde(default = "default_render_dpi")]
    pub render_dpi: f32,
}

/// One TSR crop: the structure a table-structure model recognised plus the
/// crop it came from. Cell text is pulled from the native PDF, never OCR.
#[derive(Debug, Deserialize)]
pub struct TsrTableInputDto {
    /// 0-indexed page number.
    pub page: u32,
    /// Crop box `[x1, y1, x2, y2]` in PDF points, top-left origin.
    #[serde(alias = "crop_bbox", alias = "bbox")]
    pub crop_pdf_pt_bbox: [f32; 4],
    /// DPI the crop image was rendered at.
    #[serde(default = "default_render_dpi")]
    pub render_dpi: f32,
    /// Structure tokens in document order (`<tr>`, `<td>`, …).
    pub structure_tokens: Vec<String>,
    /// One bbox per cell in crop image-pixel space: `[x1,y1,x2,y2]` or an
    /// 8-value polygon.
    #[serde(default)]
    pub cell_bboxes: Vec<Vec<f32>>,
}

/// Options for the `/v1/tables/structure*` endpoints.
#[derive(Debug, Deserialize)]
pub struct TsrOptions {
    #[serde(alias = "inputs", alias = "crops")]
    pub tables: Vec<TsrTableInputDto>,
}

impl TsrOptions {
    pub fn to_inputs(&self) -> Result<Vec<crate::TsrTableInput>, ApiError> {
        self.tables
            .iter()
            .enumerate()
            .map(|(idx, table)| {
                validate_bbox(&table.crop_pdf_pt_bbox, &format!("table {idx} crop"))?;
                if !table.render_dpi.is_finite() || table.render_dpi <= 0.0 {
                    return Err(ApiError::bad_request(format!(
                        "table {idx}: render_dpi must be a positive number"
                    )));
                }
                for (cell_idx, bbox) in table.cell_bboxes.iter().enumerate() {
                    if bbox.len() != 4 && bbox.len() != 8 {
                        return Err(ApiError::bad_request(format!(
                            "table {idx} cell {cell_idx}: expected 4 or 8 coordinates, got {}",
                            bbox.len()
                        )));
                    }
                    if bbox.iter().any(|value| !value.is_finite()) {
                        return Err(ApiError::bad_request(format!(
                            "table {idx} cell {cell_idx}: coordinates must be finite"
                        )));
                    }
                }
                Ok(crate::TsrTableInput {
                    page: table.page,
                    crop_pdf_pt_bbox: table.crop_pdf_pt_bbox,
                    render_dpi: table.render_dpi,
                    structure_tokens: table.structure_tokens.clone(),
                    cell_bboxes: table.cell_bboxes.clone(),
                })
            })
            .collect()
    }
}

/// Reject NaN/infinite or inverted boxes before they reach the extractor,
/// where they would silently match nothing.
pub fn validate_bbox(bbox: &[f32; 4], context: &str) -> Result<(), ApiError> {
    let [x1, y1, x2, y2] = *bbox;
    if !(x1.is_finite() && y1.is_finite() && x2.is_finite() && y2.is_finite()) {
        return Err(ApiError::bad_request(format!(
            "{context}: coordinates must be finite numbers"
        )));
    }
    if x2 < x1 || y2 < y1 {
        return Err(ApiError::bad_request(format!(
            "{context}: expected x2>=x1 and y2>=y1, got [{x1}, {y1}, {x2}, {y2}]"
        )));
    }
    Ok(())
}

// =========================================================================
// Responses
// =========================================================================

pub fn pdf_type_str(pdf_type: PdfType) -> &'static str {
    match pdf_type {
        PdfType::TextBased => "text_based",
        PdfType::Scanned => "scanned",
        PdfType::ImageBased => "image_based",
        PdfType::Mixed => "mixed",
    }
}

fn item_type_str(item_type: &ItemType) -> &'static str {
    match item_type {
        ItemType::Text => "text",
        ItemType::Image => "image",
        ItemType::Link(_) => "link",
        ItemType::FormField => "form_field",
    }
}

#[derive(Debug, Serialize)]
pub struct PageOcrReasonsDto {
    pub page: u32,
    pub reasons: Vec<String>,
}

impl From<crate::PageOcrReasons> for PageOcrReasonsDto {
    fn from(value: crate::PageOcrReasons) -> Self {
        Self {
            page: value.page,
            reasons: value.reasons,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct LayoutDto {
    pub is_complex: bool,
    pub pages_with_tables: Vec<u32>,
    pub pages_with_columns: Vec<u32>,
}

/// Response for `/v1/process`, `/v1/detect`, and `/v1/analyze`.
#[derive(Debug, Serialize)]
pub struct ProcessResponse {
    pub pdf_type: &'static str,
    pub page_count: u32,
    pub processing_time_ms: u64,
    pub has_text: bool,
    pub markdown_length: usize,
    pub markdown: Option<String>,
    /// 1-indexed.
    pub pages_needing_ocr: Vec<u32>,
    pub ocr_reasons_by_page: Vec<PageOcrReasonsDto>,
    pub title: Option<String>,
    pub confidence: f32,
    pub layout: LayoutDto,
    pub has_encoding_issues: bool,
}

impl From<crate::PdfProcessResult> for ProcessResponse {
    fn from(result: crate::PdfProcessResult) -> Self {
        Self {
            pdf_type: pdf_type_str(result.pdf_type),
            page_count: result.page_count,
            processing_time_ms: result.processing_time_ms,
            has_text: result.markdown.is_some(),
            markdown_length: result.markdown.as_ref().map(|md| md.len()).unwrap_or(0),
            markdown: result.markdown,
            pages_needing_ocr: result.pages_needing_ocr,
            ocr_reasons_by_page: result
                .ocr_reasons_by_page
                .into_iter()
                .map(Into::into)
                .collect(),
            title: result.title,
            confidence: result.confidence,
            layout: LayoutDto {
                is_complex: result.layout.is_complex,
                pages_with_tables: result.layout.pages_with_tables,
                pages_with_columns: result.layout.pages_with_columns,
            },
            has_encoding_issues: result.has_encoding_issues,
        }
    }
}

/// Response for `/v1/classify`.
#[derive(Debug, Serialize)]
pub struct ClassifyResponse {
    pub pdf_type: &'static str,
    pub page_count: u32,
    /// **0-indexed**, matching `classify_pdf_mem`.
    pub pages_needing_ocr: Vec<u32>,
    pub confidence: f32,
}

impl From<crate::PdfClassification> for ClassifyResponse {
    fn from(result: crate::PdfClassification) -> Self {
        Self {
            pdf_type: pdf_type_str(result.pdf_type),
            page_count: result.page_count,
            pages_needing_ocr: result.pages_needing_ocr,
            confidence: result.confidence,
        }
    }
}

/// Response for `/v1/text`.
#[derive(Debug, Serialize)]
pub struct TextResponse {
    pub text: String,
    pub length: usize,
}

#[derive(Debug, Serialize)]
pub struct TextItemDto {
    pub text: String,
    pub page: u32,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub font: String,
    pub font_size: f32,
    pub is_bold: bool,
    pub is_italic: bool,
    pub is_underline: bool,
    pub is_strikeout: bool,
    pub item_type: &'static str,
    pub mcid: Option<i64>,
    /// Present only for `item_type == "link"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

impl From<crate::TextItem> for TextItemDto {
    fn from(item: crate::TextItem) -> Self {
        let url = match &item.item_type {
            ItemType::Link(url) => Some(url.clone()),
            _ => None,
        };
        Self {
            item_type: item_type_str(&item.item_type),
            text: item.text,
            page: item.page,
            x: item.x,
            y: item.y,
            width: item.width,
            height: item.height,
            font: item.font,
            font_size: item.font_size,
            is_bold: item.is_bold,
            is_italic: item.is_italic,
            is_underline: item.is_underline,
            is_strikeout: item.is_strikeout,
            mcid: item.mcid,
            url,
        }
    }
}

/// Response for `/v1/items`.
#[derive(Debug, Serialize)]
pub struct ItemsResponse {
    pub total_items: usize,
    pub underlined_count: usize,
    pub items: Vec<TextItemDto>,
}

impl From<Vec<crate::TextItem>> for ItemsResponse {
    fn from(items: Vec<crate::TextItem>) -> Self {
        let underlined_count = items.iter().filter(|item| item.is_underline).count();
        Self {
            total_items: items.len(),
            underlined_count,
            items: items.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct PageMarkdownDto {
    /// 0-indexed.
    pub page: u32,
    pub markdown: String,
    pub needs_ocr: bool,
    pub ocr_reason: Option<String>,
}

/// Response for `/v1/pages/markdown`.
#[derive(Debug, Serialize)]
pub struct PagesMarkdownResponse {
    pub pages: Vec<PageMarkdownDto>,
    /// 1-indexed.
    pub pages_with_tables: Vec<u32>,
    /// 1-indexed.
    pub pages_with_columns: Vec<u32>,
    /// 1-indexed.
    pub pages_needing_ocr: Vec<u32>,
    pub ocr_reasons_by_page: Vec<PageOcrReasonsDto>,
    pub is_complex: bool,
}

impl From<crate::PagesExtractionResult> for PagesMarkdownResponse {
    fn from(result: crate::PagesExtractionResult) -> Self {
        Self {
            pages: result
                .pages
                .into_iter()
                .map(|page| PageMarkdownDto {
                    page: page.page,
                    markdown: page.markdown,
                    needs_ocr: page.needs_ocr,
                    ocr_reason: page.ocr_reason,
                })
                .collect(),
            pages_with_tables: result.pages_with_tables,
            pages_with_columns: result.pages_with_columns,
            pages_needing_ocr: result.pages_needing_ocr,
            ocr_reasons_by_page: result
                .ocr_reasons_by_page
                .into_iter()
                .map(Into::into)
                .collect(),
            is_complex: result.is_complex,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct RegionTextDto {
    pub text: String,
    pub needs_ocr: bool,
    pub ocr_reason: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct PageRegionsDto {
    /// 0-indexed.
    pub page: u32,
    /// Parallel to the requested regions.
    pub regions: Vec<RegionTextDto>,
}

/// Response for `/v1/regions/text` and `/v1/regions/tables`.
#[derive(Debug, Serialize)]
pub struct RegionsResponse {
    pub pages: Vec<PageRegionsDto>,
}

impl From<Vec<crate::PageRegionResult>> for RegionsResponse {
    fn from(results: Vec<crate::PageRegionResult>) -> Self {
        Self {
            pages: results
                .into_iter()
                .map(|page| PageRegionsDto {
                    page: page.page,
                    regions: page
                        .regions
                        .into_iter()
                        .map(|region| RegionTextDto {
                            text: region.text,
                            needs_ocr: region.needs_ocr,
                            ocr_reason: region.ocr_reason,
                        })
                        .collect(),
                })
                .collect(),
        }
    }
}

/// Response for `/v1/regions/vector-grid`.
#[derive(Debug, Serialize)]
pub struct VectorGridResponse {
    pub detected: bool,
    pub structure_tokens: Vec<String>,
    /// One crop-pixel bbox per `<td>` token, in document order.
    pub cell_bboxes: Vec<Vec<f32>>,
}

impl From<Option<crate::VectorGridDetection>> for VectorGridResponse {
    fn from(detection: Option<crate::VectorGridDetection>) -> Self {
        match detection {
            Some(grid) => Self {
                detected: true,
                structure_tokens: grid.structure_tokens,
                cell_bboxes: grid.cell_bboxes,
            },
            None => Self {
                detected: false,
                structure_tokens: Vec::new(),
                cell_bboxes: Vec::new(),
            },
        }
    }
}

#[derive(Debug, Serialize)]
pub struct TableMarkdownDto {
    pub markdown: String,
    /// `null` when the TSR path produced the markdown directly; otherwise a
    /// stable label such as `multi_row_in_cell_expanded`.
    pub fallback_reason: Option<String>,
}

/// Response for the `/v1/tables/structure*` markdown endpoints.
#[derive(Debug, Serialize)]
pub struct TablesMarkdownResponse {
    pub tables: Vec<TableMarkdownDto>,
}

#[derive(Debug, Serialize)]
pub struct StructuredCellDto {
    pub row: usize,
    pub col: usize,
    pub rowspan: usize,
    pub colspan: usize,
    pub is_header: bool,
    pub text: String,
    /// `[x1, y1, x2, y2]` in page PDF points, top-left origin.
    pub page_pt_bbox: [f32; 4],
}

#[derive(Debug, Serialize)]
pub struct TableCellsDto {
    pub cells: Vec<StructuredCellDto>,
}

/// Response for `/v1/tables/structure/cells`.
#[derive(Debug, Serialize)]
pub struct TableCellsResponse {
    pub tables: Vec<TableCellsDto>,
}

impl From<Vec<Vec<crate::tables::StructuredCell>>> for TableCellsResponse {
    fn from(tables: Vec<Vec<crate::tables::StructuredCell>>) -> Self {
        Self {
            tables: tables
                .into_iter()
                .map(|cells| TableCellsDto {
                    cells: cells
                        .into_iter()
                        .map(|cell| StructuredCellDto {
                            row: cell.row,
                            col: cell.col,
                            rowspan: cell.rowspan,
                            colspan: cell.colspan,
                            is_header: cell.is_header,
                            text: cell.text,
                            page_pt_bbox: cell.page_pt_bbox,
                        })
                        .collect(),
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_spec_expands_ranges_and_dedupes() {
        let pages = PageSelection::Spec("3,1,1-2".to_string())
            .resolve(false)
            .unwrap();
        assert_eq!(pages, vec![1, 2, 3]);
    }

    #[test]
    fn page_zero_is_rejected_for_one_indexed_endpoints() {
        assert!(PageSelection::List(vec![0, 1]).resolve(false).is_err());
        assert!(PageSelection::List(vec![0, 1]).resolve(true).is_ok());
    }

    #[test]
    fn compact_shorthand_and_profile_both_select_compact() {
        let options = ProcessOptions {
            compact: Some(true),
            ..Default::default()
        };
        assert!(matches!(
            options.markdown_options().profile,
            MarkdownProfile::Compact
        ));

        let options = ProcessOptions {
            profile: Some("Compact".to_string()),
            ..Default::default()
        };
        assert!(matches!(
            options.markdown_options().profile,
            MarkdownProfile::Compact
        ));
    }

    #[test]
    fn sample_size_alone_selects_the_sample_strategy() {
        let options = ProcessOptions {
            sample_size: Some(3),
            ..Default::default()
        };
        assert!(matches!(
            options.detection_config().unwrap().strategy,
            ScanStrategy::Sample(3)
        ));
    }

    #[test]
    fn unknown_mode_is_a_client_error() {
        let options = ProcessOptions {
            mode: Some("turbo".to_string()),
            ..Default::default()
        };
        assert!(options.to_pdf_options(ProcessMode::Full).is_err());
    }

    #[test]
    fn inverted_bbox_is_rejected() {
        assert!(validate_bbox(&[10.0, 0.0, 5.0, 20.0], "region").is_err());
        assert!(validate_bbox(&[0.0, 0.0, 5.0, 20.0], "region").is_ok());
    }
}
