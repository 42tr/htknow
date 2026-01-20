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
const BBOX_SCALE_TOLERANCE: f32 = 1.02;

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
            let bbox_transform = calc_bbox_transform(&page_box, coord_bounds);
            if (bbox_transform.scale_x - 1.0).abs() > f32::EPSILON
                || (bbox_transform.scale_y - 1.0).abs() > f32::EPSILON
                || bbox_transform.offset_x.abs() > f32::EPSILON
                || bbox_transform.offset_y.abs() > f32::EPSILON
            {
                info!(
                    "Page {} (idx {}): applying bbox transform scale x={} y={}, offset x={} y={}",
                    page_num,
                    page_idx,
                    bbox_transform.scale_x,
                    bbox_transform.scale_y,
                    bbox_transform.offset_x,
                    bbox_transform.offset_y
                );
            }

            // 方法：直接在页面内容流中绘制半透明矩形
            add_highlight_to_page_content(&mut doc, page_id, &page_positions, &page_box, bbox_transform)?;
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
        get_box_from_dict(&page_dict, b"MediaBox").or_else(|| get_box_from_dict(&page_dict, b"CropBox"))
    {
        return Some(page_box);
    }

    // 如果没有，尝试从父页面获取（可继承）
    if let Ok(parent_ref) = page_dict.get(b"Parent") {
        if let Ok(parent_id) = parent_ref.as_reference() {
            if let Ok(parent_dict) = doc.get_dictionary(parent_id) {
                if let Some(page_box) =
                    get_box_from_dict(&parent_dict, b"MediaBox").or_else(|| get_box_from_dict(&parent_dict, b"CropBox"))
                {
                    return Some(page_box);
                }
            }
        }
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
    let mut min_x = f32::MAX;
    let mut min_y = f32::MAX;
    let mut max_x = f32::MIN;
    let mut max_y = f32::MIN;

    for pos in positions {
        let x1 = pos.bbox[0] as f32;
        let y1 = pos.bbox[1] as f32;
        let x2 = pos.bbox[2] as f32;
        let y2 = pos.bbox[3] as f32;

        min_x = min_x.min(x1.min(x2));
        min_y = min_y.min(y1.min(y2));
        max_x = max_x.max(x1.max(x2));
        max_y = max_y.max(y1.max(y2));
    }

    if min_x.is_finite() && min_y.is_finite() && max_x.is_finite() && max_y.is_finite() {
        Some(PageCoordBounds { min_x, min_y, max_x, max_y })
    } else {
        None
    }
}

#[derive(Debug, Clone, Copy)]
struct BboxTransform {
    scale_x: f32,
    scale_y: f32,
    offset_x: f32,
    offset_y: f32,
}

fn calc_bbox_transform(page_box: &PageBox, bounds: Option<PageCoordBounds>) -> BboxTransform {
    let Some(bounds) = bounds else {
        return BboxTransform { scale_x: 1.0, scale_y: 1.0, offset_x: 0.0, offset_y: 0.0 };
    };

    let min_x = bounds.min_x.max(0.0);
    let min_y = bounds.min_y.max(0.0);
    let padded_range_x = bounds.max_x + min_x;
    let padded_range_y = bounds.max_y + min_y;

    let ratio_x = if padded_range_x > 0.0 { page_box.width / padded_range_x } else { 1.0 };
    let ratio_y = if padded_range_y > 0.0 { page_box.height / padded_range_y } else { 1.0 };

    let scale_x = if padded_range_x > page_box.width * BBOX_SCALE_TOLERANCE { ratio_x.min(1.0) } else { 1.0 };
    let scale_y = if padded_range_y > page_box.height * BBOX_SCALE_TOLERANCE { ratio_y.min(1.0) } else { 1.0 };

    let offset_x = if bounds.min_x < 0.0 { bounds.min_x } else { 0.0 };
    let offset_y = if bounds.min_y < 0.0 { bounds.min_y } else { 0.0 };

    BboxTransform { scale_x, scale_y, offset_x, offset_y }
}

/// 从 Object 获取数字值
fn get_number(obj: &Object) -> Option<f32> {
    match obj {
        Object::Integer(i) => Some(*i as f32),
        Object::Real(r) => Some(*r),
        _ => None,
    }
}

/// 在页面内容流中添加高亮矩形
fn add_highlight_to_page_content(
    doc: &mut Document, page_id: ObjectId, positions: &[&HighlightPosition], page_box: &PageBox,
    bbox_transform: BboxTransform,
) -> Result<()> {
    // 构建绘制高亮的 PDF 操作序列
    let mut operations = vec![
        // 保存图形状态
        Operation::new("q", vec![]),
        // 设置填充颜色（RGB）
        Operation::new("rg", vec![HIGHLIGHT_COLOR[0].into(), HIGHLIGHT_COLOR[1].into(), HIGHLIGHT_COLOR[2].into()]),
    ];

    // 创建 ExtGState 用于透明度
    let gs_dict = dictionary! {
        "Type" => Object::Name(b"ExtGState".to_vec()),
        "ca" => Object::Real(HIGHLIGHT_OPACITY),
        "CA" => Object::Real(HIGHLIGHT_OPACITY),
    };
    let gs_id = doc.add_object(gs_dict);

    // 添加 ExtGState 到页面资源
    let gs_name = add_extgstate_to_page(doc, page_id, gs_id)?;

    // 使用透明度状态
    operations.push(Operation::new("gs", vec![Object::Name(gs_name.into_bytes())]));

    // 为每个位置绘制矩形
    for pos in positions {
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

        info!("Drawing highlight at ({}, {}) size {}x{}", x1, y1, width, height);

        // 绘制填充矩形: x y width height re f
        operations.push(Operation::new("re", vec![x1.into(), y1.into(), width.into(), height.into()]));
        operations.push(Operation::new("f", vec![]));
    }

    // 恢复图形状态
    operations.push(Operation::new("Q", vec![]));

    // 创建新的内容流
    let highlight_content = Content { operations };
    let highlight_stream = Stream::new(dictionary! {}, highlight_content.encode()?);
    let highlight_stream_id = doc.add_object(highlight_stream);

    // 将高亮内容流添加到页面（放在原内容之后，这样高亮会覆盖在图片上方）
    let page_obj = doc.get_object(page_id)?.clone();
    if let Object::Dictionary(page_dict) = &page_obj {
        let existing_contents = page_dict.get(b"Contents").ok();

        let new_contents = match existing_contents {
            Some(Object::Reference(content_ref)) => {
                // 单个内容流引用
                Object::Array(vec![Object::Reference(*content_ref), Object::Reference(highlight_stream_id)])
            }
            Some(Object::Array(arr)) => {
                // 已经是数组
                let mut new_arr = arr.clone();
                new_arr.push(Object::Reference(highlight_stream_id));
                Object::Array(new_arr)
            }
            _ => {
                // 没有内容或其他情况
                Object::Reference(highlight_stream_id)
            }
        };

        // 更新页面
        let page_obj_mut = doc.get_object_mut(page_id)?;
        if let Object::Dictionary(page_dict_mut) = page_obj_mut {
            page_dict_mut.set("Contents", new_contents);
        }
    }

    Ok(())
}

/// 将 ExtGState 添加到页面资源并返回名称
fn add_extgstate_to_page(doc: &mut Document, page_id: ObjectId, gs_id: ObjectId) -> Result<String> {
    let gs_name = format!("GS{}", gs_id.0);

    // 获取或创建页面的 Resources
    let page_obj = doc.get_object(page_id)?.clone();
    if let Object::Dictionary(page_dict) = &page_obj {
        let resources = if let Ok(res) = page_dict.get(b"Resources") {
            match res {
                Object::Reference(res_ref) => {
                    // Resources 是引用，获取实际对象
                    let res_obj = doc.get_object(*res_ref)?.clone();
                    Some((*res_ref, res_obj))
                }
                Object::Dictionary(res_dict) => {
                    // Resources 是内联字典，需要转换为引用
                    let res_id = doc.add_object(Object::Dictionary(res_dict.clone()));
                    // 更新页面指向新的 Resources 引用
                    let page_obj_mut = doc.get_object_mut(page_id)?;
                    if let Object::Dictionary(page_dict_mut) = page_obj_mut {
                        page_dict_mut.set("Resources", Object::Reference(res_id));
                    }
                    Some((res_id, Object::Dictionary(res_dict.clone())))
                }
                _ => None,
            }
        } else {
            None
        };

        if let Some((res_id, Object::Dictionary(mut res_dict))) = resources {
            // 获取或创建 ExtGState 字典
            let extgstate = if let Ok(egs) = res_dict.get(b"ExtGState") {
                match egs {
                    Object::Dictionary(d) => d.clone(),
                    _ => lopdf::Dictionary::new(),
                }
            } else {
                lopdf::Dictionary::new()
            };

            let mut new_extgstate = extgstate;
            new_extgstate.set(gs_name.as_bytes(), Object::Reference(gs_id));
            res_dict.set("ExtGState", Object::Dictionary(new_extgstate));

            // 更新 Resources
            let res_obj_mut = doc.get_object_mut(res_id)?;
            *res_obj_mut = Object::Dictionary(res_dict);
        } else {
            // 创建新的 Resources
            let mut extgstate = lopdf::Dictionary::new();
            extgstate.set(gs_name.as_bytes(), Object::Reference(gs_id));

            let mut resources = lopdf::Dictionary::new();
            resources.set("ExtGState", Object::Dictionary(extgstate));

            let res_id = doc.add_object(Object::Dictionary(resources));

            let page_obj_mut = doc.get_object_mut(page_id)?;
            if let Object::Dictionary(page_dict_mut) = page_obj_mut {
                page_dict_mut.set("Resources", Object::Reference(res_id));
            }
        }
    }

    Ok(gs_name)
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
