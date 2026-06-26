//! PDF 高亮标注模块
//!
//! 用于在 PDF 文件中添加高亮标注

use anyhow::Result;
use log::info;
use lopdf::{
    Document, Object, ObjectId, Stream, content::{Content, Operation}, dictionary
};
use serde::{Deserialize, Serialize};

/// 高亮位置信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HighlightPosition {
    /// 页码（0-based）
    pub page_idx: i32,
    /// 边界框 [x1, y1, x2, y2]
    pub bbox: [i32; 4],
}

/// 页面坐标系边界（用于缩放转换）
#[derive(Debug, Clone, Copy)]
pub struct PageCoordBounds {
    pub min_x: f32,
    pub min_y: f32,
    pub max_x: f32,
    pub max_y: f32,
}

/// 高亮颜色配置（RGB，范围 0.0-1.0）
const HIGHLIGHT_COLOR: [f32; 3] = [1.0, 0.8, 0.0]; // 橙黄色
/// 高亮透明度
const HIGHLIGHT_OPACITY: f32 = 0.3;

/// 为 PDF 添加高亮标注
///
/// # Arguments
/// * `pdf_bytes` - 原始 PDF 字节数据
/// * `positions` - 高亮位置列表
///
/// # Returns
/// 带高亮标注的 PDF 字节数据
pub fn add_highlights_to_pdf(pdf_bytes: &[u8], positions: &[HighlightPosition]) -> Result<Vec<u8>> {
    add_highlights_to_pdf_with_bounds(pdf_bytes, positions, None)
}

/// 为 PDF 添加高亮标注（可选页面坐标边界用于缩放）
pub fn add_highlights_to_pdf_with_bounds(
    pdf_bytes: &[u8], positions: &[HighlightPosition],
    coord_bounds_by_page: Option<&std::collections::HashMap<i32, PageCoordBounds>>,
) -> Result<Vec<u8>> {
    if positions.is_empty() {
        return Ok(pdf_bytes.to_vec());
    }

    info!("Adding {} highlights to PDF", positions.len());

    let mut doc = Document::load_mem(pdf_bytes)?;

    // 获取所有页面
    let pages = doc.get_pages();
    info!("PDF has {} pages", pages.len());

    // 按页码分组高亮位置
    let mut positions_by_page: std::collections::HashMap<i32, Vec<&HighlightPosition>> =
        std::collections::HashMap::new();
    for pos in positions {
        positions_by_page.entry(pos.page_idx).or_default().push(pos);
    }

    // 为每个页面添加高亮
    for (page_idx, page_positions) in positions_by_page {
        // PDF 页码从 1 开始，positions 的 page_idx 从 0 开始
        let page_num = (page_idx + 1) as u32;

        if let Some(&page_id) = pages.get(&page_num) {
            // 获取页面的 MediaBox/CropBox 来确定页面尺寸与偏移（用于坐标转换）
            let page_box =
                get_page_box(&doc, page_id).unwrap_or(PageBox { x0: 0.0, y0: 0.0, width: 595.0, height: 842.0 });
            info!(
                "Page {} (idx {}): size = {}x{}, origin = ({}, {}), {} highlights",
                page_num,
                page_idx,
                page_box.width,
                page_box.height,
                page_box.x0,
                page_box.y0,
                page_positions.len()
            );

            let coord_bounds = coord_bounds_by_page
                .and_then(|m| m.get(&page_idx))
                .copied()
                .or_else(|| estimate_bounds_from_positions(&page_positions));
            if let Some(bounds) = coord_bounds {
                info!(
                    "Page {} (idx {}): coord bounds = ({:.2}, {:.2}) -> ({:.2}, {:.2})",
                    page_num, page_idx, bounds.min_x, bounds.min_y, bounds.max_x, bounds.max_y
                );
            } else {
                info!("Page {} (idx {}): coord bounds missing, no scaling/offset", page_num, page_idx);
            }
            let bbox_transform = calc_bbox_transform_from_bounds(&page_box, coord_bounds);
            info!(
                "Page {} (idx {}): bbox transform scale x={:.4} y={:.4}",
                page_num, page_idx, bbox_transform.scale_x, bbox_transform.scale_y
            );

            // 方法：添加 Highlight 注释（类似 PyMuPDF 的 add_highlight_annot）
            add_highlight_annotations_to_page(&mut doc, page_id, &page_positions, &page_box, bbox_transform)?;
        } else {
            info!("Page {} not found in PDF", page_num);
        }
    }

    // 保存修改后的 PDF 到内存
    let mut output = Vec::new();
    doc.save_to(&mut output)?;
    info!("Generated highlighted PDF: {} bytes", output.len());
    Ok(output)
}

/// 页面尺寸与原点
#[derive(Debug, Clone, Copy)]
struct PageBox {
    x0: f32,
    y0: f32,
    width: f32,
    height: f32,
}

/// 获取页面 MediaBox/CropBox 的尺寸与原点
fn get_page_box(doc: &Document, page_id: ObjectId) -> Option<PageBox> {
    let page_dict = doc.get_dictionary(page_id).ok()?;

    if let Some(page_box) =
        get_box_from_dict(page_dict, b"MediaBox").or_else(|| get_box_from_dict(page_dict, b"CropBox"))
    {
        return Some(page_box);
    }

    // 如果没有，尝试从父页面获取（可继承）
    if let Ok(parent_ref) = page_dict.get(b"Parent")
        && let Ok(parent_id) = parent_ref.as_reference()
        && let Ok(parent_dict) = doc.get_dictionary(parent_id)
        && let Some(page_box) =
            get_box_from_dict(parent_dict, b"MediaBox").or_else(|| get_box_from_dict(parent_dict, b"CropBox"))
    {
        return Some(page_box);
    }

    None
}

fn get_box_from_dict(dict: &lopdf::Dictionary, name: &[u8]) -> Option<PageBox> {
    let obj = dict.get(name).ok()?;
    let arr = obj.as_array().ok()?;
    if arr.len() < 4 {
        return None;
    }

    let x1 = get_number(&arr[0]).unwrap_or(0.0);
    let y1 = get_number(&arr[1]).unwrap_or(0.0);
    let x2 = get_number(&arr[2]).unwrap_or(0.0);
    let y2 = get_number(&arr[3]).unwrap_or(0.0);

    let x_min = x1.min(x2);
    let x_max = x1.max(x2);
    let y_min = y1.min(y2);
    let y_max = y1.max(y2);

    Some(PageBox { x0: x_min, y0: y_min, width: x_max - x_min, height: y_max - y_min })
}

fn estimate_bounds_from_positions(positions: &[&HighlightPosition]) -> Option<PageCoordBounds> {
    if positions.is_empty() {
        return None;
    }

    let first = positions[0];
    let mut min_x = first.bbox[0].min(first.bbox[2]) as f32;
    let mut min_y = first.bbox[1].min(first.bbox[3]) as f32;
    let mut max_x = first.bbox[0].max(first.bbox[2]) as f32;
    let mut max_y = first.bbox[1].max(first.bbox[3]) as f32;

    for pos in positions.iter().skip(1) {
        let x1 = pos.bbox[0] as f32;
        let y1 = pos.bbox[1] as f32;
        let x2 = pos.bbox[2] as f32;
        let y2 = pos.bbox[3] as f32;

        min_x = min_x.min(x1.min(x2));
        min_y = min_y.min(y1.min(y2));
        max_x = max_x.max(x1.max(x2));
        max_y = max_y.max(y1.max(y2));
    }

    Some(PageCoordBounds { min_x, min_y, max_x, max_y })
}

#[derive(Debug, Clone, Copy)]
struct BboxTransform {
    scale_x: f32,
    scale_y: f32,
    offset_x: f32,
    offset_y: f32,
}

fn calc_bbox_transform_from_bounds(page_box: &PageBox, bounds: Option<PageCoordBounds>) -> BboxTransform {
    let Some(bounds) = bounds else {
        return BboxTransform { scale_x: 1.0, scale_y: 1.0, offset_x: 0.0, offset_y: 0.0 };
    };

    if !bounds.min_x.is_finite() || !bounds.min_y.is_finite() || !bounds.max_x.is_finite() || !bounds.max_y.is_finite()
    {
        return BboxTransform { scale_x: 1.0, scale_y: 1.0, offset_x: 0.0, offset_y: 0.0 };
    }

    // MinerU docs show two possible normalized coordinate ranges:
    // - [0, 1] (percent)
    // - [0, 1000] (normalized to 1000)
    let max_x = bounds.max_x.max(bounds.min_x);
    let max_y = bounds.max_y.max(bounds.min_y);

    if max_x <= 1.5 && max_y <= 1.5 {
        return BboxTransform { scale_x: page_box.width, scale_y: page_box.height, offset_x: 0.0, offset_y: 0.0 };
    }

    if max_x <= 1000.0 && max_y <= 1000.0 {
        return BboxTransform {
            scale_x: page_box.width / 1000.0,
            scale_y: page_box.height / 1000.0,
            offset_x: 0.0,
            offset_y: 0.0,
        };
    }

    // Fallback: treat as absolute coords or pixels; shrink only if larger than page.
    let mut scale_x = 1.0;
    let mut scale_y = 1.0;

    if max_x > page_box.width * 1.02 {
        scale_x = page_box.width / max_x;
    }
    if max_y > page_box.height * 1.02 {
        scale_y = page_box.height / max_y;
    }

    BboxTransform { scale_x, scale_y, offset_x: 0.0, offset_y: 0.0 }
}

/// 从 Object 获取数字值
fn get_number(obj: &Object) -> Option<f32> {
    match obj {
        Object::Integer(i) => Some(*i as f32),
        Object::Real(r) => Some(*r),
        _ => None,
    }
}

/// 在页面上添加 Highlight 注释
fn add_highlight_annotations_to_page(
    doc: &mut Document, page_id: ObjectId, positions: &[&HighlightPosition], page_box: &PageBox,
    bbox_transform: BboxTransform,
) -> Result<()> {
    // 为每个位置添加 Highlight 注释
    for pos in positions {
        // 先减去偏移量，将坐标归一化到从 0 开始，然后缩放，最后加上页面原点
        let x1 = (pos.bbox[0] as f32 - bbox_transform.offset_x) * bbox_transform.scale_x + page_box.x0;
        let y1_mineru = (pos.bbox[1] as f32 - bbox_transform.offset_y) * bbox_transform.scale_y;
        let x2 = (pos.bbox[2] as f32 - bbox_transform.offset_x) * bbox_transform.scale_x + page_box.x0;
        let y2_mineru = (pos.bbox[3] as f32 - bbox_transform.offset_y) * bbox_transform.scale_y;

        // 转换 Y 坐标（MinerU 是从顶部向下，PDF 是从底部向上）
        let y1 = page_box.y0 + (page_box.height - y2_mineru);
        let y2 = page_box.y0 + (page_box.height - y1_mineru);

        let (x1, x2) = if x1 <= x2 { (x1, x2) } else { (x2, x1) };
        let (y1, y2) = if y1 <= y2 { (y1, y2) } else { (y2, y1) };

        let width = x2 - x1;
        let height = y2 - y1;

        info!(
            "Bbox raw: [{}, {}, {}, {}] -> PDF coords: ({:.2}, {:.2}) size {:.2}x{:.2}",
            pos.bbox[0], pos.bbox[1], pos.bbox[2], pos.bbox[3], x1, y1, width, height
        );

        // 检查坐标是否在合理范围内
        if width <= 0.0 || height <= 0.0 {
            info!("Skipping invalid highlight: width={}, height={}", width, height);
            continue;
        }
        if x1 < page_box.x0 - 100.0
            || x2 > page_box.x0 + page_box.width + 100.0
            || y1 < page_box.y0 - 100.0
            || y2 > page_box.y0 + page_box.height + 100.0
        {
            info!("Warning: highlight outside page bounds: ({:.2}, {:.2}) -> ({:.2}, {:.2})", x1, y1, x2, y2);
        }

        // Highlight 注释需要 Rect + QuadPoints（PDF 坐标系，原点在左下）
        let rect = Object::Array(vec![x1.into(), y1.into(), x2.into(), y2.into()]);
        let quad_points =
            Object::Array(vec![x1.into(), y2.into(), x2.into(), y2.into(), x2.into(), y1.into(), x1.into(), y1.into()]);

        let appearance_id = create_highlight_appearance(doc, width, height)?;

        let annot = dictionary! {
            "Type" => Object::Name(b"Annot".to_vec()),
            "Subtype" => Object::Name(b"Highlight".to_vec()),
            "Rect" => rect,
            "QuadPoints" => quad_points,
            "C" => Object::Array(vec![HIGHLIGHT_COLOR[0].into(), HIGHLIGHT_COLOR[1].into(), HIGHLIGHT_COLOR[2].into()]),
            "CA" => Object::Real(HIGHLIGHT_OPACITY),
            "F" => Object::Integer(4), // Print
            "P" => Object::Reference(page_id),
            "AP" => dictionary! { "N" => Object::Reference(appearance_id) },
        };

        let annot_id = doc.add_object(annot);
        add_annot_to_page(doc, page_id, annot_id)?;
    }

    Ok(())
}

/// 将注释添加到页面的 Annots 列表
fn add_annot_to_page(doc: &mut Document, page_id: ObjectId, annot_id: ObjectId) -> Result<()> {
    let page_obj = doc.get_object(page_id)?.clone();
    if let Object::Dictionary(page_dict) = &page_obj {
        match page_dict.get(b"Annots") {
            Ok(Object::Reference(annots_ref)) => {
                let annots_obj = doc.get_object(*annots_ref)?.clone();
                let mut new_arr = match annots_obj {
                    Object::Array(arr) => arr,
                    _ => Vec::new(),
                };
                new_arr.push(Object::Reference(annot_id));
                let annots_obj_mut = doc.get_object_mut(*annots_ref)?;
                *annots_obj_mut = Object::Array(new_arr);
            }
            Ok(Object::Array(arr)) => {
                let mut new_arr = arr.clone();
                new_arr.push(Object::Reference(annot_id));
                let page_obj_mut = doc.get_object_mut(page_id)?;
                if let Object::Dictionary(page_dict_mut) = page_obj_mut {
                    page_dict_mut.set("Annots", Object::Array(new_arr));
                }
            }
            _ => {
                let page_obj_mut = doc.get_object_mut(page_id)?;
                if let Object::Dictionary(page_dict_mut) = page_obj_mut {
                    page_dict_mut.set("Annots", Object::Array(vec![Object::Reference(annot_id)]));
                }
            }
        }
    }

    Ok(())
}

fn create_highlight_appearance(doc: &mut Document, width: f32, height: f32) -> Result<ObjectId> {
    let gs_dict = dictionary! {
        "Type" => Object::Name(b"ExtGState".to_vec()),
        "ca" => Object::Real(HIGHLIGHT_OPACITY),
        "CA" => Object::Real(HIGHLIGHT_OPACITY),
    };
    let gs_id = doc.add_object(gs_dict);

    let resources = dictionary! {
        "ExtGState" => dictionary! {
            "GS1" => Object::Reference(gs_id),
        },
    };

    let operations = vec![
        Operation::new("q", vec![]),
        Operation::new("gs", vec![Object::Name(b"GS1".to_vec())]),
        Operation::new("rg", vec![HIGHLIGHT_COLOR[0].into(), HIGHLIGHT_COLOR[1].into(), HIGHLIGHT_COLOR[2].into()]),
        Operation::new("re", vec![0.into(), 0.into(), width.into(), height.into()]),
        Operation::new("f", vec![]),
        Operation::new("Q", vec![]),
    ];

    let content = Content { operations };
    let stream = Stream::new(
        dictionary! {
            "Type" => Object::Name(b"XObject".to_vec()),
            "Subtype" => Object::Name(b"Form".to_vec()),
            "BBox" => Object::Array(vec![0.into(), 0.into(), width.into(), height.into()]),
            "Resources" => Object::Dictionary(resources),
        },
        content.encode()?,
    );

    Ok(doc.add_object(stream))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_highlight_position_serialization() {
        let pos = HighlightPosition { page_idx: 0, bbox: [100, 200, 300, 250] };
        let json = serde_json::to_string(&pos).unwrap();
        assert!(json.contains("page_idx"));
        assert!(json.contains("bbox"));
    }
}
