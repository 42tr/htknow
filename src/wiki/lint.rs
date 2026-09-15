//! 知识库 Wiki 体检：只报告问题，不自动改写内容。
//!
//! 自动修复（合并重复页、补写摘要）都要过模型，成本与风险都高于收益；
//! 这里给出可操作的清单，由用户在前端逐条处理，`POST /wiki/rebuild-links`
//! 负责其中唯一可以确定性修复的部分——链接收敛。

use std::collections::{HashMap, HashSet};

use anyhow::Result;
use serde::Serialize;
use sqlx::SqlitePool;
use utoipa::ToSchema;

use super::{STATUS_PUBLISHED, linkify, page};

pub const ISSUE_BROKEN_LINK: &str = "broken_link";
pub const ISSUE_EMPTY_CONTENT: &str = "empty_content";
pub const ISSUE_MISSING_SUMMARY: &str = "missing_summary";
pub const ISSUE_ORPHAN_PAGE: &str = "orphan_page";
pub const ISSUE_STALE_SOURCE: &str = "stale_source";
pub const ISSUE_DUPLICATE_TITLE: &str = "duplicate_title";

/// 单次体检扫描的页面上限。
const MAX_PAGES_PER_LINT: i64 = 20000;
/// 报告里最多返回的问题条数，超出部分只计入 `issue_count`。
const MAX_REPORTED_ISSUES: usize = 500;

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct LintIssue {
    /// broken_link / empty_content / missing_summary / orphan_page / stale_source / duplicate_title
    pub kind: String,
    pub page_id: i64,
    pub slug: String,
    pub title: String,
    /// 相关对象：死链指向的 slug、失效来源的 file_id、重名页面的 slug
    pub target: String,
    pub message: String,
    /// info / warning，前端据此分色
    pub severity: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct LintKindCount {
    pub kind: String,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct LintReport {
    pub kb_id: i64,
    pub scanned: usize,
    /// 发现的全部问题数（可能大于 `issues.len()`）
    pub issue_count: usize,
    pub truncated: bool,
    pub by_kind: Vec<LintKindCount>,
    pub issues: Vec<LintIssue>,
}

impl LintIssue {
    fn new(kind: &str, severity: &str, page_id: i64, slug: &str, title: &str, target: &str, message: String) -> Self {
        Self {
            kind: kind.to_string(),
            page_id,
            slug: slug.to_string(),
            title: title.to_string(),
            target: target.to_string(),
            message,
            severity: severity.to_string(),
        }
    }

    fn warning(kind: &str, page: &super::WikiPage, target: &str, message: String) -> Self {
        Self::new(kind, "warning", page.id, &page.slug, &page.title, target, message)
    }

    fn info(kind: &str, page: &super::WikiPage, target: &str, message: String) -> Self {
        Self::new(kind, "info", page.id, &page.slug, &page.title, target, message)
    }
}

/// 收集器：统一处理条数上限与分类计数。
#[derive(Default)]
struct Collector {
    issues: Vec<LintIssue>,
    counts: HashMap<&'static str, usize>,
    total: usize,
}

impl Collector {
    fn push(&mut self, kind: &'static str, issue: LintIssue) {
        self.total += 1;
        *self.counts.entry(kind).or_default() += 1;
        if self.issues.len() < MAX_REPORTED_ISSUES {
            self.issues.push(issue);
        }
    }
}

/// 对一个知识库做体检。索引页只参与「死链目标」判定，不检查自身。
pub async fn lint_kb(pool: &SqlitePool, kb_id: i64) -> Result<LintReport> {
    // status = None：草稿页也在体检范围内，归档页已被 page::list 排除。
    let pages = page::list(pool, kb_id, None, None, MAX_PAGES_PER_LINT, None).await?;
    let live: HashSet<&str> = pages.iter().filter(|p| p.status == STATUS_PUBLISHED).map(|p| p.slug.as_str()).collect();
    let mut collector = Collector::default();

    let mut titles: HashMap<String, Vec<&super::WikiPage>> = HashMap::new();
    for current in pages.iter().filter(|p| !p.is_index()) {
        if current.status == STATUS_PUBLISHED {
            titles.entry(normalize_title(&current.title)).or_default().push(current);
        }
        if current.content.trim().is_empty() {
            collector.push(
                ISSUE_EMPTY_CONTENT,
                LintIssue::warning(ISSUE_EMPTY_CONTENT, current, "", "页面正文为空".to_string()),
            );
        }
        if current.summary.trim().is_empty() {
            collector.push(
                ISSUE_MISSING_SUMMARY,
                LintIssue::info(ISSUE_MISSING_SUMMARY, current, "", "缺少摘要，目录与搜索结果里会显示为空".to_string()),
            );
        }
        if current.in_links.is_empty() {
            collector.push(
                ISSUE_ORPHAN_PAGE,
                LintIssue::info(ISSUE_ORPHAN_PAGE, current, "", "没有任何页面链接到本页".to_string()),
            );
        }
        for target in linkify::out_links(&current.content, &current.slug) {
            if live.contains(target.as_str()) {
                continue;
            }
            collector.push(
                ISSUE_BROKEN_LINK,
                LintIssue::warning(
                    ISSUE_BROKEN_LINK,
                    current,
                    &target,
                    format!("链接指向不存在或未发布的页面：{}", target),
                ),
            );
        }
    }

    for (_, group) in titles.iter().filter(|(_, group)| group.len() > 1) {
        for current in group {
            let others: Vec<&str> = group.iter().filter(|p| p.id != current.id).map(|p| p.slug.as_str()).collect();
            collector.push(
                ISSUE_DUPLICATE_TITLE,
                LintIssue::info(
                    ISSUE_DUPLICATE_TITLE,
                    current,
                    &others.join(","),
                    format!("与其他页面标题重复：{}", others.join("、")),
                ),
            );
        }
    }

    // 来源失效：文件解析未完成（或已被删但关系残留）时，页面内容无法再被重建。
    let stale: Vec<(i64, String, String, i64)> = sqlx::query_as(
        "SELECT p.id, p.slug, p.title, s.file_id
           FROM wiki_page_sources s
           JOIN wiki_pages p ON p.id = s.page_id
           LEFT JOIN files f ON f.id = s.file_id
          WHERE p.kb_id = ? AND p.status NOT IN ('archived', 'withdrawn') AND (f.id IS NULL OR f.status != 1)",
    )
    .bind(kb_id)
    .fetch_all(pool)
    .await?;
    for (page_id, slug, title, file_id) in stale {
        collector.push(
            ISSUE_STALE_SOURCE,
            LintIssue::new(
                ISSUE_STALE_SOURCE,
                "warning",
                page_id,
                &slug,
                &title,
                &file_id.to_string(),
                format!("来源文件 {} 已删除或解析未完成", file_id),
            ),
        );
    }

    let mut by_kind: Vec<LintKindCount> =
        collector.counts.into_iter().map(|(kind, count)| LintKindCount { kind: kind.to_string(), count }).collect();
    by_kind.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.kind.cmp(&b.kind)));
    Ok(LintReport {
        kb_id,
        scanned: pages.len(),
        issue_count: collector.total,
        truncated: collector.total > collector.issues.len(),
        by_kind,
        issues: collector.issues,
    })
}

/// 标题归一化：去空白、ASCII 小写。只用于「可能是同一个东西」的提示，不做合并决策。
fn normalize_title(title: &str) -> String {
    title.split_whitespace().collect::<Vec<&str>>().join(" ").to_lowercase()
}
