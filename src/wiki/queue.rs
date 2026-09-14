//! Wiki 待办队列。基于 SQLite，不依赖 Redis：
//! 用 `status` + `run_id` + `claimed_at` 做令牌式认领，`not_before` 实现防抖与退避。

use anyhow::Result;
use log::{debug, warn};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, SqlitePool};

use super::{TASK_FINALIZE, TASK_INGEST, TASK_REFRESH, TASK_RETRACT};

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

#[derive(Debug, Clone, FromRow)]
pub struct WikiTask {
    pub id: i64,
    pub kb_id: i64,
    pub task_type: String,
    pub op: String,
    pub file_id: Option<i64>,
    pub slug: String,
    pub payload: String,
    pub fail_count: i64,
}

/// 入队结果的序列化载荷。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RetractPayload {
    #[serde(default)]
    pub page_ids: Vec<i64>,
}

/// 解析 retract 触发器写入的 payload。
///
/// 触发器用 `group_concat` 存裸的逗号分隔 ID 列表（避免依赖 JSON1 函数），
/// 这里同时兼容 JSON 形式，便于测试与手工重放。
pub fn parse_retract_payload(payload: &str) -> RetractPayload {
    let trimmed = payload.trim();
    if trimmed.is_empty() {
        return RetractPayload::default();
    }
    if trimmed.starts_with('{') {
        if let Ok(parsed) = serde_json::from_str::<RetractPayload>(trimmed) {
            return parsed;
        }
    }
    let page_ids = trimmed.split(',').filter_map(|part| part.trim().parse::<i64>().ok()).collect();
    RetractPayload { page_ids }
}

/// 在不存在同类未完成任务时插入一行。返回是否新插入。
async fn enqueue_unless_pending(
    pool: &SqlitePool, task_type: &str, kb_id: i64, op: &str, file_id: Option<i64>, slug: &str, payload: &str,
    not_before: i64,
) -> Result<bool> {
    let inserted = sqlx::query(
        "INSERT INTO wiki_tasks(kb_id, task_type, op, file_id, slug, payload, not_before)
         SELECT ?, ?, ?, ?, ?, ?, ?
         WHERE NOT EXISTS (
             SELECT 1 FROM wiki_tasks
             WHERE task_type = ? AND status = 'pending'
               AND kb_id IS ? AND file_id IS ? AND slug = ?
         )",
    )
    .bind(kb_id)
    .bind(task_type)
    .bind(op)
    .bind(file_id)
    .bind(slug)
    .bind(payload)
    .bind(not_before)
    .bind(task_type)
    .bind(kb_id)
    .bind(file_id)
    .bind(slug)
    .execute(pool)
    .await?
    .rows_affected();
    Ok(inserted > 0)
}

/// 文档解析完成后入队一次 Wiki 生成。
pub async fn enqueue_ingest(pool: &SqlitePool, kb_id: i64, file_id: i64) -> Result<bool> {
    enqueue_unless_pending(pool, TASK_INGEST, kb_id, "add", Some(file_id), "", "", now()).await
}

/// 入队知识库级收敛（索引页、交叉链接、死链清理）。
///
/// 已存在未执行的 finalize 时不重复入队、也不推迟 `not_before`：finalize 读取的是
/// 当前数据库状态，一次执行即可收敛整批变更，连续上传不会把它无限延后。
pub async fn enqueue_finalize(pool: &SqlitePool, kb_id: i64) -> Result<bool> {
    let delay = crate::config::get().wiki.finalize_delay_secs as i64;
    enqueue_unless_pending(pool, TASK_FINALIZE, kb_id, "change", None, "", "", now() + delay).await
}

/// 入队单页重建（回撤后用剩余来源确定性重建）。
pub async fn enqueue_refresh(pool: &SqlitePool, kb_id: i64, slug: &str) -> Result<bool> {
    enqueue_unless_pending(pool, TASK_REFRESH, kb_id, "refresh", None, slug, "", now()).await
}

/// 入队回撤任务。正常路径由 `wiki_file_delete` 触发器写入，这里供测试与手工重放使用。
pub async fn enqueue_retract(pool: &SqlitePool, kb_id: i64, file_id: i64, page_ids: &[i64]) -> Result<bool> {
    let payload = serde_json::to_string(&RetractPayload { page_ids: page_ids.to_vec() })?;
    enqueue_unless_pending(pool, TASK_RETRACT, kb_id, "remove", Some(file_id), "", &payload, now()).await
}

/// 原子认领一批到期任务。
pub async fn claim_batch(pool: &SqlitePool, limit: usize, run_id: &str) -> Result<Vec<WikiTask>> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    let rows = sqlx::query_as::<_, WikiTask>(
        "UPDATE wiki_tasks
            SET status = 'claimed', run_id = ?, claimed_at = ?, updated_at = strftime('%s','now')
          WHERE id IN (
              SELECT id FROM wiki_tasks WHERE status = 'pending' AND not_before <= ? ORDER BY id LIMIT ?
          )
         RETURNING id, kb_id, task_type, op, file_id, slug, payload, fail_count",
    )
    .bind(run_id)
    .bind(now())
    .bind(now())
    .bind(limit as i64)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// 任务成功：直接删除行，保持队列表小。进度信息由 `wiki_builds` 与页面数据承载。
pub async fn mark_done(pool: &SqlitePool, task_id: i64) -> Result<()> {
    sqlx::query("DELETE FROM wiki_tasks WHERE id = ?").bind(task_id).execute(pool).await?;
    Ok(())
}

/// 任务失败：未超重试上限则复位为 pending 并按指数退避延后；超限则丢弃并告警。
pub async fn mark_failed(pool: &SqlitePool, task: &WikiTask, error: &str) -> Result<()> {
    let cfg = crate::config::get();
    let fail_count = task.fail_count + 1;
    if fail_count > cfg.wiki.max_fail_retries as i64 {
        warn!(
            "wiki task {} ({}) for kb {} exceeded retry budget ({}), dropping: {}",
            task.id, task.task_type, task.kb_id, fail_count, error
        );
        sqlx::query("DELETE FROM wiki_tasks WHERE id = ?").bind(task.id).execute(pool).await?;
        return Ok(());
    }
    // 2^n 退避，上限 30 分钟：限流风暴下靠拉长调度间隔降速，而不是硬失败。
    let backoff = (2i64.pow(fail_count.min(10) as u32) * 5).min(1800);
    debug!("wiki task {} failed (attempt {}), retry in {}s: {}", task.id, fail_count, backoff, error);
    sqlx::query(
        "UPDATE wiki_tasks
            SET status = 'pending', fail_count = ?, not_before = ?, claimed_at = NULL, run_id = '',
                last_error = ?, updated_at = strftime('%s','now')
          WHERE id = ?",
    )
    .bind(fail_count)
    .bind(now() + backoff)
    .bind(error)
    .bind(task.id)
    .execute(pool)
    .await?;
    Ok(())
}

/// 回收崩溃残留的认领：进程重启或任务超时后把过期 claim 复位，避免队列被卡死。
pub async fn recover_stale(pool: &SqlitePool) -> Result<u64> {
    let stale_before = now() - crate::config::get().wiki.claim_stale_secs as i64;
    let result = sqlx::query(
        "UPDATE wiki_tasks
            SET status = 'pending', claimed_at = NULL, run_id = '', updated_at = strftime('%s','now')
          WHERE status = 'claimed' AND (claimed_at IS NULL OR claimed_at < ?)",
    )
    .bind(stale_before)
    .execute(pool)
    .await?;
    let recovered = result.rows_affected();
    if recovered > 0 {
        warn!("wiki queue: recovered {} stale claimed tasks", recovered);
    }
    Ok(recovered)
}

/// 启动时无条件复位所有 claimed 任务：单实例部署下这些只可能是上次进程的残留。
pub async fn recover_all_claimed(pool: &SqlitePool) -> Result<u64> {
    let result = sqlx::query(
        "UPDATE wiki_tasks
            SET status = 'pending', claimed_at = NULL, run_id = '', updated_at = strftime('%s','now')
          WHERE status = 'claimed'",
    )
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

/// 某个知识库的待处理任务数，用于前端「索引中」提示。
pub async fn pending_count(pool: &SqlitePool, kb_id: i64) -> Result<i64> {
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM wiki_tasks WHERE kb_id = ? AND status IN ('pending','claimed')")
            .bind(kb_id)
            .fetch_one(pool)
            .await?;
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_retract_payload_accepts_both_forms() {
        assert_eq!(parse_retract_payload("1,2,3").page_ids, vec![1, 2, 3]);
        assert_eq!(parse_retract_payload("").page_ids, Vec::<i64>::new());
        assert_eq!(parse_retract_payload(r#"{"page_ids":[7]}"#).page_ids, vec![7]);
        assert_eq!(parse_retract_payload("1, ,2").page_ids, vec![1, 2]);
    }

    #[tokio::test]
    async fn enqueue_dedups_and_claim_is_atomic() {
        let pool = crate::wiki::tests::database().await;
        crate::wiki::tests::seed_kb(&pool, 1).await;

        assert!(enqueue_ingest(&pool, 1, 10).await.unwrap());
        // 同一文件的重复入队被合并，避免重复生成。
        assert!(!enqueue_ingest(&pool, 1, 10).await.unwrap());
        assert_eq!(pending_count(&pool, 1).await.unwrap(), 1);

        let claimed = claim_batch(&pool, 10, "run-1").await.unwrap();
        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].file_id, Some(10));
        // 已认领的任务不会被第二个 worker 抢走。
        assert!(claim_batch(&pool, 10, "run-2").await.unwrap().is_empty());

        mark_failed(&pool, &claimed[0], "boom").await.unwrap();
        assert_eq!(pending_count(&pool, 1).await.unwrap(), 1);
        let retried = sqlx::query_scalar::<_, i64>("SELECT fail_count FROM wiki_tasks WHERE id = ?")
            .bind(claimed[0].id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(retried, 1);

        mark_done(&pool, claimed[0].id).await.unwrap();
        assert_eq!(pending_count(&pool, 1).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn finalize_respects_debounce_window() {
        let pool = crate::wiki::tests::database().await;
        crate::wiki::tests::seed_kb(&pool, 1).await;

        assert!(enqueue_finalize(&pool, 1).await.unwrap());
        assert!(!enqueue_finalize(&pool, 1).await.unwrap());
        // not_before 在未来，因此到期前认领不到。
        assert!(claim_batch(&pool, 10, "run-1").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn recover_all_claimed_releases_stuck_tasks() {
        let pool = crate::wiki::tests::database().await;
        crate::wiki::tests::seed_kb(&pool, 1).await;
        enqueue_ingest(&pool, 1, 10).await.unwrap();
        claim_batch(&pool, 10, "run-1").await.unwrap();

        assert_eq!(recover_all_claimed(&pool).await.unwrap(), 1);
        assert_eq!(claim_batch(&pool, 10, "run-2").await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn retry_budget_drops_task_when_exceeded() {
        let pool = crate::wiki::tests::database().await;
        crate::wiki::tests::seed_kb(&pool, 1).await;
        enqueue_ingest(&pool, 1, 10).await.unwrap();
        let mut task = claim_batch(&pool, 10, "run-1").await.unwrap().remove(0);
        task.fail_count = crate::config::get().wiki.max_fail_retries as i64;

        mark_failed(&pool, &task, "permanent").await.unwrap();
        assert_eq!(pending_count(&pool, 1).await.unwrap(), 0);
    }
}
