//! Markdown 感知的 Wiki 交叉链接注入。
//!
//! 目标：把正文里出现的「已有页面标题/别名」自动改写成 `[[slug|显示名]]`，
//! 同时绝不破坏代码块、行内代码、已有链接与 HTML 标签。
//!
//! 实现上用 aho-corasick 一次扫出全部候选命中（leftmost-longest），所有替换都基于
//! **原文偏移**一次性拼接，因此不需要在每次插入后重算禁止区间——知识库有上千个
//! 页面时，逐页面 × 逐条目的重复扫描是不可接受的。

use std::collections::HashSet;

use aho_corasick::{AhoCorasick, MatchKind};

use super::page::PageSurface;

/// 字节偏移区间 `[start, end)`。所有分隔符都是 ASCII，按字节扫描不会切断 UTF-8。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    fn contains(&self, start: usize, end: usize) -> bool {
        start < self.end && end > self.start
    }
}

/// 一条待插入的链接。
#[derive(Debug, Clone)]
struct Insertion {
    start: usize,
    end: usize,
    slug: String,
    label: String,
}

/// 链接化结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkifyOutcome {
    pub content: String,
    pub out_links: Vec<String>,
    pub changed: bool,
}

fn line_ranges(content: &str) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut start = 0usize;
    for (offset, _) in content.match_indices('\n') {
        ranges.push((start, offset));
        start = offset + 1;
    }
    if start <= content.len() {
        ranges.push((start, content.len()));
    }
    ranges
}

/// 围栏标记长度（``` 或 ~~~，至少 3 个），非围栏行返回 0。
fn fence_marker_len(trimmed: &str) -> usize {
    let bytes = trimmed.as_bytes();
    let Some(&first) = bytes.first() else { return 0 };
    if first != b'`' && first != b'~' {
        return 0;
    }
    let len = bytes.iter().take_while(|&&b| b == first).count();
    if len >= 3 { len } else { 0 }
}

/// 行内引用定义 `[label]: url`，整行禁止插入链接。
fn is_reference_definition(trimmed: &str) -> bool {
    if !trimmed.starts_with('[') {
        return false;
    }
    let Some(close) = find_closing_bracket(trimmed, 0) else { return false };
    trimmed[close + 1..].trim_start().starts_with(':')
}

/// 从 `open` 处的 `[` 找到配对的 `]`，考虑反斜杠转义与嵌套。
fn find_closing_bracket(text: &str, open: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut depth = 0i32;
    let mut i = open;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'[' => {
                depth += 1;
                i += 1;
            }
            b']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    None
}

/// 行内禁止区间：行内代码、已有 wiki 链接、Markdown 链接/图片、HTML 标签与自动链接。
fn inline_forbidden(content: &str, start: usize, end: usize) -> Vec<Span> {
    let bytes = content.as_bytes();
    let mut spans = Vec::new();
    let mut i = start;
    while i < end {
        match bytes[i] {
            b'`' => {
                let run = (i..end).take_while(|&j| bytes[j] == b'`').count();
                let close = content[i + run..end].find(&"`".repeat(run));
                match close {
                    Some(offset) => {
                        let close_start = i + run + offset;
                        spans.push(Span { start: i, end: close_start + run });
                        i = close_start + run;
                    }
                    None => {
                        spans.push(Span { start: i, end });
                        i = end;
                    }
                }
            }
            b'[' if content[i..].starts_with("[[") => match content[i..end].find("]]") {
                Some(offset) => {
                    spans.push(Span { start: i, end: i + offset + 2 });
                    i += offset + 2;
                }
                None => {
                    spans.push(Span { start: i, end });
                    i = end;
                }
            },
            b'!' | b'[' => {
                let open = if bytes[i] == b'!' { i + 1 } else { i };
                if open >= end || bytes[open] != b'[' {
                    i += 1;
                    continue;
                }
                let Some(close) = find_closing_bracket(&content[..end], open) else {
                    i += 1;
                    continue;
                };
                let after = close + 1;
                if after < end && bytes[after] == b'(' {
                    match content[after..end].find(')') {
                        Some(offset) => {
                            spans.push(Span { start: i, end: after + offset + 1 });
                            i = after + offset + 1;
                        }
                        None => {
                            spans.push(Span { start: i, end });
                            i = end;
                        }
                    }
                } else if after < end && bytes[after] == b'[' {
                    match find_closing_bracket(&content[..end], after) {
                        Some(second) => {
                            spans.push(Span { start: i, end: second + 1 });
                            i = second + 1;
                        }
                        None => {
                            spans.push(Span { start: i, end });
                            i = end;
                        }
                    }
                } else {
                    i += 1;
                }
            }
            b'<' => {
                let next = bytes.get(i + 1).copied();
                let looks_like_markup = matches!(next, Some(b) if b.is_ascii_alphanumeric() || b == b'/' || b == b'!');
                if !looks_like_markup {
                    i += 1;
                    continue;
                }
                match content[i..end].find('>') {
                    Some(offset) => {
                        spans.push(Span { start: i, end: i + offset + 1 });
                        i += offset + 1;
                    }
                    None => {
                        i += 1;
                    }
                }
            }
            _ => i += 1,
        }
    }
    spans
}

/// 计算正文中所有不允许插入 wiki 链接的区间。
pub fn forbidden_spans(content: &str) -> Vec<Span> {
    let mut spans = Vec::new();
    let mut open_fence: Option<(usize, usize, u8)> = None;
    for (start, end) in line_ranges(content) {
        let line = &content[start..end];
        let trimmed = line.trim_start();
        let marker_len = fence_marker_len(trimmed);
        let marker_byte = trimmed.as_bytes().first().copied().unwrap_or(0);

        if let Some((_, fence_len, fence_byte)) = open_fence {
            spans.push(Span { start, end });
            let is_close = marker_len >= fence_len
                && marker_byte == fence_byte
                && trimmed.trim_end_matches(fence_byte as char).trim().is_empty();
            if is_close {
                open_fence = None;
            }
            continue;
        }
        if marker_len >= 3 {
            spans.push(Span { start, end });
            open_fence = Some((start, marker_len, marker_byte));
            continue;
        }
        if is_reference_definition(trimmed) {
            spans.push(Span { start, end });
            continue;
        }
        spans.extend(inline_forbidden(content, start, end));
    }
    // 未闭合的围栏：从此处到文末都算代码。
    if let Some((fence_start, _, _)) = open_fence {
        spans.push(Span { start: fence_start, end: content.len() });
    }
    spans.sort_by_key(|span| span.start);
    spans
}

fn span_contains(spans: &[Span], start: usize, end: usize) -> bool {
    spans.iter().any(|span| span.contains(start, end))
}

fn is_ascii_word_byte(byte: Option<u8>) -> bool {
    matches!(byte, Some(b) if b.is_ascii_alphanumeric() || b == b'_')
}

/// 命中处是否具备词边界。仅对 ASCII 边缘生效：中文没有词边界概念，
/// 强制要求会漏掉「在腾讯工作」这类正常表述。
fn has_word_boundary(content: &str, start: usize, end: usize) -> bool {
    let bytes = content.as_bytes();
    let left_ok = !is_ascii_word_byte(bytes.get(start).copied())
        || !is_ascii_word_byte(bytes.get(start.wrapping_sub(1)).copied());
    let right_ok = !is_ascii_word_byte(bytes.get(end - 1).copied()) || !is_ascii_word_byte(bytes.get(end).copied());
    left_ok && right_ok
}

/// 解析 `[[slug]]` / `[[slug|label]]`，返回 (slug, label)。
pub fn parse_wiki_link(inner: &str) -> Option<(String, String)> {
    let inner = inner.trim();
    if inner.is_empty() {
        return None;
    }
    match inner.split_once('|') {
        Some((slug, label)) => {
            let slug = slug.trim();
            let label = label.trim();
            if slug.is_empty() {
                return None;
            }
            Some((slug.to_string(), if label.is_empty() { slug.to_string() } else { label.to_string() }))
        }
        None => Some((inner.to_string(), inner.to_string())),
    }
}

/// 提取正文中全部 wiki 链接。
pub fn extract_wiki_links(content: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut cursor = 0usize;
    while let Some(offset) = content[cursor..].find("[[") {
        let start = cursor + offset;
        let Some(close) = content[start..].find("]]") else { break };
        let inner = &content[start + 2..start + close];
        if let Some(parsed) = parse_wiki_link(inner) {
            out.push(parsed);
        }
        cursor = start + close + 2;
    }
    out
}

/// 正文中的出链 slug 列表（去重、保序、排除自链）。
pub fn out_links(content: &str, self_slug: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for (slug, _) in extract_wiki_links(content) {
        if slug == self_slug || !seen.insert(slug.clone()) {
            continue;
        }
        out.push(slug);
    }
    out
}

/// 把失效的 `[[slug|label]]` 还原成纯文本 label。
///
/// 页面被删除或 reduce 失败后，摘要页可能仍指向不存在的 slug；
/// 留着会渲染成断链，直接抹掉链接比留 404 更好。
pub fn strip_dead_links(content: &str, live_slugs: &HashSet<String>) -> (String, bool) {
    let mut out = String::with_capacity(content.len());
    let mut cursor = 0usize;
    let mut changed = false;
    while let Some(offset) = content[cursor..].find("[[") {
        let start = cursor + offset;
        let Some(close) = content[start..].find("]]") else { break };
        let inner = &content[start + 2..start + close];
        out.push_str(&content[cursor..start]);
        match parse_wiki_link(inner) {
            Some((slug, label)) if !live_slugs.contains(&slug) => {
                out.push_str(&label);
                changed = true;
            }
            _ => {
                out.push_str(&content[start..start + close + 2]);
            }
        }
        cursor = start + close + 2;
    }
    out.push_str(&content[cursor..]);
    (out, changed)
}

/// 交叉链接匹配器：一次构建，供整个知识库的所有页面复用。
pub struct Linkifier {
    automaton: Option<AhoCorasick>,
    /// pattern index -> (slug, 表面词)
    patterns: Vec<(String, String)>,
}

impl Linkifier {
    /// 由页面标题与别名构建。索引页不参与匹配，避免正文到处链到目录。
    pub fn new(surfaces: &[PageSurface]) -> Self {
        let mut patterns: Vec<(String, String)> = Vec::new();
        let mut seen: HashSet<(String, String)> = HashSet::new();
        for surface in surfaces {
            if surface.page_type == super::PAGE_TYPE_INDEX || surface.slug.is_empty() {
                continue;
            }
            let mut names = vec![surface.title.clone()];
            names.extend(surface.aliases.iter().cloned());
            for name in names {
                let trimmed = name.trim();
                // 单字符表面词误命中率太高（例如别名 "R"），直接跳过。
                if trimmed.chars().count() < 2 {
                    continue;
                }
                if seen.insert((surface.slug.clone(), trimmed.to_string())) {
                    patterns.push((surface.slug.clone(), trimmed.to_string()));
                }
            }
        }
        // 长词优先：同一位置命中多个表面词时选更长的，避免「腾讯控股」被链成「腾讯」。
        patterns.sort_by(|a, b| b.1.chars().count().cmp(&a.1.chars().count()).then_with(|| a.1.cmp(&b.1)));
        let automaton = if patterns.is_empty() {
            None
        } else {
            AhoCorasick::builder()
                .match_kind(MatchKind::LeftmostLongest)
                .build(patterns.iter().map(|(_, name)| name.as_str()))
                .ok()
        };
        Self { automaton, patterns }
    }

    pub fn is_empty(&self) -> bool {
        self.automaton.is_none()
    }

    /// 为单个页面注入交叉链接。每个 slug 只链接**首次**出现，避免满屏重复链接。
    pub fn linkify(&self, content: &str, self_slug: &str) -> LinkifyOutcome {
        let links = out_links(content, self_slug);
        let Some(automaton) = &self.automaton else {
            return LinkifyOutcome { content: content.to_string(), out_links: links, changed: false };
        };

        let forbidden = forbidden_spans(content);
        let mut insertions: Vec<Insertion> = Vec::new();
        let mut linked_slugs: HashSet<String> = HashSet::new();
        // 已有的 wiki 链接计入 out_links，但不重复链接。
        for (slug, _) in extract_wiki_links(content) {
            linked_slugs.insert(slug);
        }

        for matched in automaton.find_iter(content) {
            let pattern = matched.pattern().as_usize();
            let Some((slug, name)) = self.patterns.get(pattern) else { continue };
            if slug == self_slug || linked_slugs.contains(slug) {
                continue;
            }
            let (start, end) = (matched.start(), matched.end());
            if span_contains(&forbidden, start, end) || !has_word_boundary(content, start, end) {
                continue;
            }
            if insertions.iter().any(|item| start < item.end && end > item.start) {
                continue;
            }
            linked_slugs.insert(slug.clone());
            insertions.push(Insertion { start, end, slug: slug.clone(), label: name.clone() });
        }

        if insertions.is_empty() {
            return LinkifyOutcome { content: content.to_string(), out_links: links, changed: false };
        }
        insertions.sort_by_key(|item| item.start);

        let mut out = String::with_capacity(content.len() + insertions.len() * 16);
        let mut cursor = 0usize;
        for item in &insertions {
            out.push_str(&content[cursor..item.start]);
            out.push_str("[[");
            out.push_str(&item.slug);
            out.push('|');
            out.push_str(&item.label);
            out.push_str("]]");
            cursor = item.end;
        }
        out.push_str(&content[cursor..]);
        let links = out_links(&out, self_slug);
        LinkifyOutcome { content: out, out_links: links, changed: true }
    }
}
