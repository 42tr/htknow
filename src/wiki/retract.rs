//! 文档删除后的 Wiki 回撤。
//!
//! `wiki_file_delete` 触发器在文件删除**之前**把受影响的页面 ID 写进任务 payload，
//! 因为 `wiki_page_sources` 会随文件级联删除，异步任务运行时已经查不到来源关系。

use anyhow::Result;
use log::{debug, info};
use sqlx::SqlitePool;

use super::{STATUS_ARCHIVED, page, queue};

#[derive(Debug, Clone, Default)]
pub struct RetractReport {
    pub pages_deleted: usize,
    /// 人工写过的页面失去全部来源时归档而不是删除
    pub pages_archived: usize,
    pub pages_queued_for_refresh: usize,
}

/// 回撤一个文件的 Wiki 贡献。
///
/// 策略：页面若失去全部来源则直接删除；仍有其他来源的页面排队用剩余证据**确定性重建**，
/// 而不是让模型「减去某个文档的贡献」——后者既难验证，也容易连带删掉仍然成立的内容。
pub async fn retract_pages(pool: &SqlitePool, page_ids: &[i64]) -> Result<RetractReport> {
    let mut report = RetractReport::default();
    let mut touched_kbs: Vec<i64> = Vec::new();

    for page_id in page_ids {
        let Some(page) = page::get_by_id(pool, *page_id).await? else {
            debug!("wiki retract: page {} already gone", page_id);
            continue;
        };
        let remaining = page::source_file_ids(pool, page.id).await?;
        if remaining.is_empty() && page::is_manual_edit_source(&page.last_edit_source) {
            // 证据没了但用户写的字还在：归档保留，历史版本也仍可回滚。
            info!("wiki retract: page {} lost all sources, archiving manual page", page.slug);
            page::set_status(pool, page.id, STATUS_ARCHIVED, &page.last_edit_source, &page.last_editor_id).await?;
            report.pages_archived += 1;
        } else if remaining.is_empty() {
            info!("wiki retract: page {} lost all sources, deleting", page.slug);
            page::delete_by_id(pool, page.id).await?;
            report.pages_deleted += 1;
        } else {
            queue::enqueue_refresh(pool, page.kb_id, &page.slug).await?;
            report.pages_queued_for_refresh += 1;
        }
        if !touched_kbs.contains(&page.kb_id) {
            touched_kbs.push(page.kb_id);
        }
    }

    // 页面被删或改写后，索引页与交叉链接都需要收敛一次。
    for kb_id in touched_kbs {
        queue::enqueue_finalize(pool, kb_id).await?;
    }
    Ok(report)
}
