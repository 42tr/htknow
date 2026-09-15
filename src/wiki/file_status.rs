//! 文件的整体处理进度：解析就绪后，继续等待该文档的 Wiki 任务。
//! 状态从持久队列与构建记录推导，重启、重试和批量重建使用同一口径。

use sqlx::{QueryBuilder, Sqlite, SqlitePool};

use crate::api::File;

/// 返回可作为 JOIN 子查询使用的状态表。这里只插入受控的布尔默认值。
pub(crate) fn relation() -> String {
    let enabled = i32::from(crate::config::get().wiki.enabled);
    format!(
        "(SELECT state.*, CASE
            WHEN parse_status != 1 THEN parse_status
            WHEN wiki_status = 'failed' THEN -1
            WHEN wiki_status IN ('pending', 'running', 'retrying') THEN 2
            ELSE 1 END AS processing_status
          FROM (
            SELECT f.id, f.status AS parse_status,
              CASE WHEN f.status != 1 OR kb.id IS NULL OR COALESCE(
                CASE WHEN json_valid(kb.wiki_config) THEN
                  CASE WHEN json_type(kb.wiki_config, '$.enabled') IN ('true', 'false')
                    THEN json_extract(kb.wiki_config, '$.enabled') END END, {enabled}) = 0
              THEN NULL
              ELSE COALESCE(
                (SELECT CASE WHEN t.status = 'claimed' THEN 'running'
                             WHEN t.fail_count > 0 THEN 'retrying' ELSE 'pending' END
                   FROM wiki_tasks t WHERE t.file_id = f.id AND t.kb_id = f.kb_id
                    AND t.task_type = 'wiki:ingest' AND t.status IN ('pending', 'claimed')
                   ORDER BY t.id DESC LIMIT 1),
                (SELECT CASE WHEN b.status = 'completed' AND b.page_count = 0 THEN 'skipped'
                             ELSE b.status END FROM wiki_builds b WHERE b.file_id = f.id),
                'pending') END AS wiki_status,
              COALESCE(
                (SELECT NULLIF(t.last_error, '') FROM wiki_tasks t
                  WHERE t.file_id = f.id AND t.kb_id = f.kb_id AND t.task_type = 'wiki:ingest'
                  ORDER BY t.id DESC LIMIT 1),
                (SELECT NULLIF(b.error, '') FROM wiki_builds b WHERE b.file_id = f.id)
              ) AS wiki_error
            FROM files f LEFT JOIN knowledge_bases kb ON kb.id = f.kb_id
          ) state)"
    )
}

#[derive(sqlx::FromRow)]
pub(crate) struct FileProgress {
    pub id: i64,
    pub processing_status: i32,
    pub wiki_status: Option<String>,
    pub wiki_error: Option<String>,
}

pub(crate) async fn load(
    pool: &SqlitePool, ids: &[i64],
) -> Result<std::collections::HashMap<i64, FileProgress>, sqlx::Error> {
    let mut result = std::collections::HashMap::new();
    for batch in ids.chunks(500) {
        let mut query = QueryBuilder::<Sqlite>::new(format!(
            "SELECT id, processing_status, wiki_status, wiki_error FROM {} p WHERE p.id IN (",
            relation()
        ));
        let mut values = query.separated(",");
        for id in batch {
            values.push_bind(*id);
        }
        query.push(")");
        for mut row in query.build_query_as::<FileProgress>().fetch_all(pool).await? {
            if !matches!(row.wiki_status.as_deref(), Some("failed" | "retrying")) {
                row.wiki_error = None;
            }
            result.insert(row.id, row);
        }
    }
    Ok(result)
}

/// 分批补充对外文件数据，避免逐文件查询，也不读取原文。
pub(crate) async fn populate(pool: &SqlitePool, files: &mut [File]) -> Result<(), sqlx::Error> {
    let ids: Vec<i64> = files.iter().map(|f| f.id).collect();
    let mut rows = load(pool, &ids).await?;
    for file in files {
        if let Some(row) = rows.remove(&file.id) {
            file.processing_status = Some(row.processing_status);
            file.wiki_status = row.wiki_status;
            file.wiki_error = row.wiki_error;
        }
    }
    Ok(())
}

/// 入队失败或重试耗尽也必须有持久的失败记录，包括 LLM 调用前的错误。
pub(crate) async fn record_failure(
    pool: &SqlitePool, kb_id: i64, file_id: i64, error: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO wiki_builds(file_id, status, error)
         SELECT id, 'failed', ? FROM files WHERE id = ? AND kb_id = ? AND status = 1
         ON CONFLICT(file_id) DO UPDATE SET status = 'failed', error = excluded.error,
           run_id = '', fingerprint = '', updated_at = strftime('%s','now')",
    )
    .bind(error)
    .bind(file_id)
    .bind(kb_id)
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wiki::{
        queue,
        tests::{database, seed_file, seed_kb_with_wiki_config},
    };

    async fn progress(pool: &SqlitePool) -> FileProgress {
        load(pool, &[1]).await.unwrap().remove(&1).unwrap()
    }

    #[tokio::test]
    async fn wiki_file_status_tracks_queue_retries_and_terminal_failure() {
        let pool = database().await;
        seed_kb_with_wiki_config(&pool, 1, r#"{"enabled":true}"#).await;
        seed_file(&pool, 1, Some(1), "document.txt").await;
        let pending = progress(&pool).await;
        assert_eq!(pending.processing_status, 2);
        assert_eq!(pending.wiki_status.as_deref(), Some("pending"));
        queue::enqueue_ingest(&pool, 1, 1).await.unwrap();
        let mut task = queue::claim_batch(&pool, 1, "test").await.unwrap().remove(0);
        assert_eq!(progress(&pool).await.wiki_status.as_deref(), Some("running"));
        queue::mark_failed(&pool, &task, "upstream unavailable").await.unwrap();
        let retry = progress(&pool).await;
        assert_eq!(retry.processing_status, 2);
        assert_eq!(retry.wiki_status.as_deref(), Some("retrying"));
        assert_eq!(retry.wiki_error.as_deref(), Some("upstream unavailable"));
        task.fail_count = crate::config::get().wiki.max_fail_retries as i64;
        queue::mark_failed(&pool, &task, "retry budget exhausted").await.unwrap();
        let failed = progress(&pool).await;
        assert_eq!(failed.processing_status, -1);
        assert_eq!(failed.wiki_error.as_deref(), Some("retry budget exhausted"));
        assert_eq!(queue::pending_count(&pool, 1).await.unwrap(), 0);

        queue::enqueue_ingest(&pool, 1, 1).await.unwrap();
        let task = queue::claim_batch(&pool, 1, "retry").await.unwrap().remove(0);
        sqlx::query("UPDATE wiki_builds SET status = 'completed', page_count = 1, error = '' WHERE file_id = 1")
            .execute(&pool)
            .await
            .unwrap();
        // 构建写完，但队列尚未确认时仍处于生成阶段。
        assert_eq!(progress(&pool).await.processing_status, 2);
        queue::mark_done(&pool, task.id).await.unwrap();
        let completed = progress(&pool).await;
        assert_eq!(completed.processing_status, 1);
        assert_eq!(completed.wiki_status.as_deref(), Some("completed"));
        assert_eq!(completed.wiki_error, None);
    }

    #[tokio::test]
    async fn wiki_file_status_respects_config_and_invalidates_old_parse_builds() {
        let pool = database().await;
        seed_kb_with_wiki_config(&pool, 1, r#"{"enabled":true}"#).await;
        seed_kb_with_wiki_config(&pool, 2, r#"{"enabled":false}"#).await;
        seed_file(&pool, 1, Some(1), "document.txt").await;
        sqlx::query("INSERT INTO wiki_builds(file_id, status, page_count) VALUES(1, 'completed', 0)")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(progress(&pool).await.wiki_status.as_deref(), Some("skipped"));
        queue::enqueue_ingest(&pool, 1, 1).await.unwrap();
        let mut old_task = queue::claim_batch(&pool, 1, "old").await.unwrap().remove(0);
        sqlx::query("UPDATE files SET status = 0 WHERE id = 1").execute(&pool).await.unwrap();
        assert_eq!(progress(&pool).await.processing_status, 0);
        assert_eq!(queue::pending_count(&pool, 1).await.unwrap(), 0);
        sqlx::query("UPDATE files SET status = 1 WHERE id = 1").execute(&pool).await.unwrap();
        assert_eq!(progress(&pool).await.wiki_status.as_deref(), Some("pending"));
        old_task.fail_count = crate::config::get().wiki.max_fail_retries as i64;
        queue::mark_failed(&pool, &old_task, "stale task").await.unwrap();
        assert_eq!(progress(&pool).await.wiki_status.as_deref(), Some("pending"));
        sqlx::query("UPDATE files SET kb_id = 2 WHERE id = 1").execute(&pool).await.unwrap();
        let disabled = progress(&pool).await;
        assert_eq!(disabled.processing_status, 1);
        assert_eq!(disabled.wiki_status, None);
        assert_eq!(disabled.wiki_error, None);
        sqlx::query("UPDATE files SET kb_id = NULL WHERE id = 1").execute(&pool).await.unwrap();
        assert_eq!(progress(&pool).await.processing_status, 1);
    }
}
