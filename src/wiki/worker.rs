//! Wiki 后台 worker：轮询 `wiki_tasks`、认领、分发。
//!
//! 结构与 `FileProcessor` 保持一致（自适应轮询 + 令牌式认领），不引入 Redis/asynq：
//! HTKnow 是单实例部署，SQLite 的原子 UPDATE ... RETURNING 已经足够做互斥认领。
//!
//! 注意：spawn 出去的 future 里一律只搬运**拥有所有权**的值（`SqlitePool`/`WikiTask`），
//! 不要跨 `.await` 持有 `&self`、`&str` 这类泛型生命周期借用，否则会撞上
//! "implementation of `Send` is not general enough"。

use std::{collections::HashMap, sync::Arc, time::Duration};

use anyhow::{Result, anyhow};
use log::{debug, error, info, warn};
use sqlx::SqlitePool;
use tokio::sync::Semaphore;

use super::queue::{self, WikiTask, parse_retract_payload};
use super::{TASK_FINALIZE, TASK_INGEST, TASK_REFRESH, TASK_RETRACT};

/// 每知识库在途任务上限，防止一个大知识库的批量导入独占整个 worker。
#[derive(Clone)]
struct KbPermits {
    inner: Arc<std::sync::Mutex<HashMap<i64, Arc<Semaphore>>>>,
    limit: usize,
}

impl KbPermits {
    fn new(limit: usize) -> Self {
        Self { inner: Arc::new(std::sync::Mutex::new(HashMap::new())), limit: limit.max(1) }
    }

    fn for_kb(&self, kb_id: i64) -> Arc<Semaphore> {
        let mut map = self.inner.lock().expect("wiki kb permit map poisoned");
        map.entry(kb_id).or_insert_with(|| Arc::new(Semaphore::new(self.limit))).clone()
    }
}

pub struct WikiWorker {
    pool: SqlitePool,
}

impl WikiWorker {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// 启动后台轮询任务。
    pub fn start(self) {
        tokio::spawn(worker_loop(self.pool));
    }

    /// 认领并处理一批任务，返回处理数量（0 表示队列空闲）。供测试与手动触发使用。
    pub async fn run_cycle(&self) -> Result<usize> {
        run_cycle(self.pool.clone()).await
    }
}

async fn worker_loop(pool: SqlitePool) {
    let cfg = crate::config::get();
    info!("Wiki worker started with interval: {}s", cfg.wiki.worker_interval_secs);

    // 单实例部署：启动时所有 claimed 行都只可能是上次进程的残留。
    match queue::recover_all_claimed(&pool).await {
        Ok(0) => {}
        Ok(recovered) => warn!("Wiki worker recovered {} interrupted tasks", recovered),
        Err(e) => error!("Failed to recover wiki tasks: {}", e),
    }

    // 自适应轮询：空闲时逐步拉长间隔，有任务时立即回到基础间隔。
    let base_interval = Duration::from_secs(cfg.wiki.worker_interval_secs.max(1));
    let max_idle_interval = base_interval * 3;
    let mut idle_interval = base_interval;

    loop {
        if crate::processor::is_parse_paused() {
            debug!("Wiki worker paused for index maintenance");
            tokio::time::sleep(base_interval).await;
            continue;
        }
        // worker_loop 只 spawn 一次：cycle 里的 panic 必须挡在这一层，
        // 否则整个 Wiki 队列会静默停摆到进程重启。
        match tokio::spawn(run_cycle(pool.clone())).await {
            Ok(Ok(0)) => idle_interval = (idle_interval * 2).min(max_idle_interval),
            Ok(Ok(_)) => idle_interval = base_interval,
            Ok(Err(e)) => {
                error!("Wiki worker cycle failed: {}", e);
                idle_interval = (idle_interval * 2).min(max_idle_interval);
            }
            Err(join_error) => {
                error!("Wiki worker cycle panicked: {}", join_error);
                idle_interval = (idle_interval * 2).min(max_idle_interval);
            }
        }
        tokio::time::sleep(idle_interval).await;
    }
}

async fn run_cycle(pool: SqlitePool) -> Result<usize> {
    let cfg = crate::config::get();
    let run_id = uuid::Uuid::new_v4().simple().to_string();
    let tasks = queue::claim_batch(&pool, cfg.wiki.batch_size, &run_id).await?;
    if tasks.is_empty() {
        recover_stale_if_needed(&pool).await?;
        return Ok(0);
    }

    // 认领即开始计时，批次跑完前必须周期性续约，否则长任务会被当成残留抢走。
    let _heartbeat = HeartbeatGuard(spawn_claim_heartbeat(pool.clone(), run_id.clone()));

    let permits = KbPermits::new(cfg.wiki.max_inflight_per_kb);
    // 每个任务单独 spawn：任务内部 panic 只作废它自己，按失败记账交给队列退避重试，
    // 不会连带丢掉整批结果、更不会把 worker_loop 打死。
    // 逐个搬运拥有所有权的值：闭包 + 借用捕获会让 spawn 链路的 Send 证明
    // 退化成高阶生命周期问题。
    let mut spawned = Vec::with_capacity(tasks.len());
    for task in tasks {
        spawned.push((task.clone(), tokio::spawn(run_one(pool.clone(), permits.clone(), task))));
    }
    // 任务已全部并发跑起来，按序 join 只是收结果，不再限制并发度。
    let mut outcomes = Vec::with_capacity(spawned.len());
    for (task, handle) in spawned {
        let result = match handle.await {
            Ok(result) => result,
            Err(join_error) => Err(anyhow!("wiki task panicked: {}", join_error)),
        };
        outcomes.push((task, result));
    }

    let mut processed = 0usize;
    for (task, result) in outcomes {
        match result {
            Ok(()) => {
                queue::mark_done(&pool, task.id).await?;
                processed += 1;
            }
            Err(error) => {
                let message = error.to_string();
                warn!("wiki task {} ({}) failed: {}", task.id, task.task_type, message);
                queue::mark_failed(&pool, &task, &message).await?;
                processed += 1;
            }
        }
    }
    recover_stale_if_needed(&pool).await?;
    Ok(processed)
}

async fn run_one(pool: SqlitePool, permits: KbPermits, task: WikiTask) -> Result<()> {
    // 每知识库在途上限：ingest/finalize 都受约束，避免单库饿死其他库。
    let semaphore = permits.for_kb(task.kb_id);
    let _permit =
        semaphore.acquire().await.map_err(|_| anyhow!("wiki kb permit semaphore closed for kb {}", task.kb_id))?;
    run_task(&pool, &task).await
}

/// 心跳守卫：批次结束、出错返回或 panic 展开时都要停掉续约。
///
/// 漏掉 abort 的后果不是「多跑一个协程」：心跳会一直刷新 `claimed_at`，
/// `recover_stale` 永远判定这批任务还活着，队列就此卡死。
struct HeartbeatGuard(tokio::task::JoinHandle<()>);

impl Drop for HeartbeatGuard {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// 批次执行期间周期性续约 `claimed_at`，handle 交给 [`HeartbeatGuard`] 在作用域结束时 abort。
///
/// 间隔取 `claim_stale_secs / 3`：一次续约失败（例如数据库忙）也还有两次机会补上。
/// 本批全部落地后 `renew_claims` 返回 0，任务自行退出，不会留下常驻协程。
fn spawn_claim_heartbeat(pool: SqlitePool, run_id: String) -> tokio::task::JoinHandle<()> {
    let interval = Duration::from_secs((crate::config::get().wiki.claim_stale_secs / 3).clamp(10, 600));
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(interval).await;
            match queue::renew_claims(&pool, &run_id).await {
                Ok(0) => break,
                Ok(renewed) => debug!("wiki queue: renewed {} claimed tasks for run {}", renewed, run_id),
                Err(error) => warn!("wiki queue: failed to renew claims for run {}: {}", run_id, error),
            }
        }
    })
}

/// 只有在确实存在 claimed 行时才发起复位写入，避免每轮空转都产生一次写事务。
async fn recover_stale_if_needed(pool: &SqlitePool) -> Result<()> {
    let has_claimed: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM wiki_tasks WHERE status = 'claimed')").fetch_one(pool).await?;
    if has_claimed {
        queue::recover_stale(pool).await?;
    }
    Ok(())
}

async fn run_task(pool: &SqlitePool, task: &WikiTask) -> Result<()> {
    match task.task_type.as_str() {
        TASK_INGEST => {
            let file_id = task.file_id.ok_or_else(|| anyhow!("ingest task {} has no file_id", task.id))?;
            let kb_id = current_kb_id(pool, file_id).await?;
            match kb_id {
                Some(kb_id) => {
                    super::ingest::ingest_file(pool, kb_id, file_id).await?;
                }
                // 文件已删除或不属于任何知识库：Wiki 是知识库级能力，无处可写。
                None => debug!("wiki ingest: file {} has no knowledge base, skipping", file_id),
            }
            Ok(())
        }
        TASK_RETRACT => {
            let payload = parse_retract_payload(&task.payload);
            super::retract::retract_pages(pool, &payload.page_ids).await?;
            Ok(())
        }
        TASK_REFRESH => {
            if task.slug.is_empty() {
                return Err(anyhow!("refresh task {} has no slug", task.id));
            }
            super::ingest::refresh_page(pool, task.kb_id, &task.slug).await?;
            Ok(())
        }
        TASK_FINALIZE => {
            super::finalize::finalize_kb(pool, task.kb_id).await?;
            Ok(())
        }
        other => Err(anyhow!("unknown wiki task type {}", other)),
    }
}

/// 解析文件当前所属知识库。文件被移动后任务里的 kb_id 会过期，必须以库中现值为准。
async fn current_kb_id(pool: &SqlitePool, file_id: i64) -> Result<Option<i64>> {
    let kb_id: Option<Option<i64>> =
        sqlx::query_scalar("SELECT kb_id FROM files WHERE id = ?").bind(file_id).fetch_optional(pool).await?;
    Ok(kb_id.flatten())
}
