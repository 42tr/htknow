//! Wiki 生成用的 prompt 模板。
//!
//! 采用 `{{Key}}` 占位符 + `render` 的极简替换，而不是引入模板引擎：模板里含大量
//! JSON 示例（带字面 `{}`），`format!` 会冲突，模板引擎又只为这几处调用而增加依赖。
//!
//! 块顺序刻意把「静态规则 + 文档内稳定内容」放在前、「每批变化的切片」放在后，
//! 便于命中上游 provider 的前缀缓存。

use super::page::PageSurface;

/// 替换 `{{Key}}` 占位符。未提供的键保留原样，便于测试时观察缺失项。
pub fn render(template: &str, values: &[(&str, &str)]) -> String {
    let mut out = template.to_string();
    for (key, value) in values {
        out = out.replace(&format!("{{{{{}}}}}", key), value);
    }
    out
}

pub const CANDIDATE_EXTRACT_SYSTEM: &str = "你是一个知识抽取系统。你只输出合法 JSON，不输出任何解释、前言或代码围栏。";

/// 模式 B（图谱不可用时）的候选条目抽取：只要骨架，事实留给引用归类那一遍。
pub const CANDIDATE_EXTRACT_USER: &str = r#"请分析下面的文档，列出其中重要的实体与关键概念，作为候选条目清单。
后续会另有一遍负责为每个条目挂上具体的原文切片证据，因此这里不需要写出详尽的事实描述。

<抽取范围>
{{Granularity}}
{{ExtraInstructions}}
</抽取范围>

<已有页面>
下列 slug 已经存在。若某条目与其中之一指向同一对象，请复用该 slug，不要另造：
{{ExistingSlugs}}
</已有页面>

<文档>
{{Content}}
</文档>

字段要求：
- name：条目名称，使用{{Language}}
- slug："entity/<名称>" 或 "concept/<名称>"，小写化 ASCII、分隔符用连字符，中文名称可直接保留中文
- aliases：指代**完全相同**对象的别名（官方缩写、全称/简称、译名）。不要放上位类别、相关产品或泛化术语；没有则给 []
- description：一句话（15-40 字）说明该条目是什么，用于索引列表，必须自洽
- details：1-3 句兜底简介，仅在下游引用归类失败时使用，不超过 200 字

归类规则：
- 具体命名事物（人物、公司、产品、地点）只放进 entities
- 抽象方法、理论、主题只放进 concepts
- 同一条目不得同时出现在两个数组
- 不要把仅被顺带提及的名称提升为条目

只输出合法 JSON。字符串值内不要使用字面换行，需要换行时写 \n。

输出示例：
{"entities":[{"name":"腾讯","slug":"entity/腾讯","aliases":["腾讯控股"],"description":"一家中国的互联网科技公司。","details":"总部位于深圳，主营社交与游戏业务。"}],"concepts":[{"name":"检索增强生成","slug":"concept/检索增强生成","aliases":["RAG"],"description":"一种把信息检索与大模型生成结合的技术。","details":"先检索文档，再作为上下文交给模型作答。"}]}"#;

pub const CHUNK_CITATION_SYSTEM: &str = "你是一个精确的引用系统。你只输出合法 JSON，不输出任何解释、前言或代码围栏。";

/// 模式 B 的第二遍：判定哪些切片实质性讨论了每个候选条目。
/// 让事实以原文切片的形式保留，而不是让模型转述，可显著降低幻觉并保留可溯源性。
pub const CHUNK_CITATION_USER: &str = r#"下面给出一批文档切片和一份候选条目清单。请判断每个候选条目被哪些切片**实质性讨论**。

"实质性讨论"指该切片至少陈述了关于该候选条目的一条具体事实、属性、步骤、日期、数字或关系，而不是顺带提及。

规则：
- 只能引用下面 <chunks> 中出现的切片，使用其 id 属性原值（例如 "c003"）
- 若某候选条目在本批切片中完全没有被实质讨论，就不要让它出现在输出里（不要给空数组）
- 同一切片可以被多个候选条目引用

<candidate_slugs>
{{Candidates}}
</candidate_slugs>

<chunks>
{{Chunks}}
</chunks>

只输出合法 JSON：
{"citations":{"entity/腾讯":["c001","c004"],"concept/检索增强生成":["c002"]}}"#;

pub const DOC_SUMMARY_SYSTEM: &str = "你是一名百科编辑，负责为一篇源文档撰写结构化摘要页。";

/// 文档摘要页。刻意不把文件名交给模型：扫描件常以设备型号命名，喂进去只会诱导幻觉。
pub const DOC_SUMMARY_USER: &str = r#"请根据下面的文档内容，写一页结构化的百科摘要（Markdown）。

<document>
{{Content}}
</document>

<available_wiki_pages>
{{AvailablePages}}
</available_wiki_pages>

要求：
1. 输出的**第一行**必须是：SUMMARY: 一句话（15-40 字）概括本文档讲的是什么，用于索引列表
2. 之后写完整的 Markdown 摘要，覆盖关键事实、论点与结论
3. 使用正确的标题层级（## 小节，### 子小节）
4. 内链规则：<available_wiki_pages> 的每一行格式是 "slug = 显示名 (别名: a, b)"。凡是提到其中出现的名称或别名，必须写成 [[slug|显示名]]，不要写成加粗，也不要写裸 slug。只能使用清单里给出的 slug，不要编造
5. 只依据文档内容写作，不要引入外部知识，也不要提及文件名
6. 使用{{Language}}"#;

pub const PAGE_BODY_SYSTEM: &str = "你是一名百科编辑，负责撰写或更新一个条目页面。";

/// 条目页正文生成/合并。新页面与增量更新共用一套模板，通过 `ExistingSection` 区分。
pub const PAGE_BODY_USER: &str = r#"请为条目「{{Title}}」（类型：{{PageType}}）撰写百科页面正文。

<条目简介>
{{Description}}
</条目简介>

<原文证据>
下面是来自源文档的原文切片。页面内容必须**只**依据这些原文组织，可以引用其中的事实、数字、日期与表述，不得引入证据之外的信息：
{{Evidence}}
</原文证据>
{{ExistingSection}}
<可链接页面>
{{AvailablePages}}
</可链接页面>

要求：
1. 只输出 Markdown 正文，不要输出页面标题（标题由系统写入），不要用代码围栏包裹整篇输出
2. 用 ## 组织小节，内容较多时可用 ###
3. 每条事实性表述都要能对应到上面的原文证据；证据之间冲突时并列说明，不要臆断取舍
4. 内链规则：凡是提到「可链接页面」中出现的名称或别名，必须写成 [[slug|显示名]]；只能使用清单里给出的 slug
5. 不要编造引用编号、脚注或来源链接
6. 使用{{Language}}"#;

/// 已有页面需要增量合并时插入的段落。
pub const PAGE_BODY_EXISTING_SECTION: &str = r#"
<现有页面内容>
下面是该页面的当前内容。请把「原文证据」中的新信息合并进去：保留仍然成立的既有内容，补充新事实，删除与新证据矛盾且已无来源支撑的表述。
{{ExistingContent}}
</现有页面内容>
"#;

pub const INDEX_INTRO_SYSTEM: &str = "你是一名百科编辑，负责撰写知识库 Wiki 的导言。";

/// 索引页导言。目录本身由系统确定性生成，模型只负责写导言，避免它编造条目。
pub const INDEX_INTRO_USER: &str = r#"下面是知识库 Wiki 的目录清单。请写一段简短导言（80-200 字），说明这个 Wiki 覆盖了哪些主题、适合怎样浏览。

只输出导言正文：不要标题，不要罗列目录（目录由系统自动生成），不要编造清单之外的条目。使用{{Language}}。

<目录>
{{Directory}}
</目录>"#;

/// 渲染 `<available_wiki_pages>` 清单：`slug = 显示名 (别名: a, b)`。
///
/// 索引页自身不进入清单，避免页面链到目录页造成噪声。
pub fn available_pages(surfaces: &[PageSurface], limit: usize) -> String {
    let mut lines: Vec<String> = Vec::new();
    for surface in surfaces {
        if surface.page_type == super::PAGE_TYPE_INDEX {
            continue;
        }
        if limit > 0 && lines.len() >= limit {
            break;
        }
        if surface.aliases.is_empty() {
            lines.push(format!("{} = {}", surface.slug, surface.title));
        } else {
            lines.push(format!("{} = {} (别名: {})", surface.slug, surface.title, surface.aliases.join(", ")));
        }
    }
    if lines.is_empty() {
        return "(暂无可链接页面)".to_string();
    }
    lines.join("\n")
}

/// 一条原文证据。
#[derive(Debug, Clone)]
pub struct EvidenceChunk {
    /// 对外暴露的短句柄（c001…），模型只需回引句柄，不接触真实切片 ID。
    pub handle: String,
    pub slice_id: i64,
    pub content: String,
}

/// 渲染 `<chunks>` / `<原文证据>` 块，总字符数受 `budget` 约束。
///
/// 超预算时按顺序截断并在末尾标注省略条数，保证模型看到的每条证据都是完整的，
/// 不会出现半句话被当成事实引用。
pub fn render_evidence(chunks: &[EvidenceChunk], budget: usize) -> String {
    let mut out = String::new();
    let mut used = 0usize;
    let mut included = 0usize;
    for chunk in chunks {
        let block = format!("<c id=\"{}\">\n{}\n</c>\n", chunk.handle, chunk.content.trim());
        if budget > 0 && used + block.chars().count() > budget && included > 0 {
            out.push_str(&format!("<!-- 另有 {} 条证据因长度限制未纳入 -->\n", chunks.len() - included));
            break;
        }
        out.push_str(&block);
        used += block.chars().count();
        included += 1;
    }
    if out.is_empty() {
        return "(无可用原文证据)".to_string();
    }
    out
}

/// 渲染引用归类用的候选清单（XML 形式，便于模型逐条对齐）。
pub fn render_candidates(candidates: &[(&str, &str, &str)]) -> String {
    let mut out = String::new();
    for (slug, name, description) in candidates {
        out.push_str(&format!(
            "<item slug=\"{}\" name=\"{}\" description=\"{}\" />\n",
            xml_escape(slug),
            xml_escape(name),
            xml_escape(description)
        ));
    }
    if out.is_empty() {
        return "(无候选条目)".to_string();
    }
    out
}

pub fn xml_escape(value: &str) -> String {
    value.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// 按字符数截断长文本，用于控制送入模型的文档正文规模。
pub fn truncate_chars(value: &str, max_chars: usize) -> String {
    let count = value.chars().count();
    if max_chars == 0 || count <= max_chars {
        return value.to_string();
    }
    let head: String = value.chars().take(max_chars).collect();
    format!("{}\n<!-- 正文过长，已截断，原文共 {} 字 -->", head, count)
}

/// 从摘要页输出中拆出 `SUMMARY:` 首行与正文。
pub fn split_summary_line(raw: &str) -> (String, String) {
    let trimmed = raw.trim_start();
    let (first, rest) = match trimmed.split_once('\n') {
        Some((first, rest)) => (first, rest),
        None => (trimmed, ""),
    };
    let stripped = first
        .trim()
        .strip_prefix("SUMMARY:")
        .or_else(|| first.trim().strip_prefix("SUMMARY："))
        .map(str::trim)
        .unwrap_or("");
    if stripped.is_empty() {
        // 模型没按格式给首行：整篇当正文，摘要留空由上层兜底。
        return (String::new(), trimmed.to_string());
    }
    (stripped.to_string(), rest.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_replaces_placeholders_and_keeps_unknown_keys() {
        let out = render("{{A}} 与 {{B}}", &[("A", "甲")]);
        assert_eq!(out, "甲 与 {{B}}");
    }

    #[test]
    fn templates_have_no_unbalanced_placeholders() {
        // 模板里的 JSON 示例含字面 {}，替换逻辑必须只认 {{Key}} 双花括号。
        let out = render(CANDIDATE_EXTRACT_USER, &[("Granularity", "G"), ("Language", "中文")]);
        assert!(out.contains("{\"entities\""));
        assert!(out.contains("G"));
    }

    #[test]
    fn available_pages_skips_index_and_renders_aliases() {
        let surfaces = vec![
            PageSurface { slug: "index".into(), title: "目录".into(), page_type: "index".into(), aliases: vec![] },
            PageSurface {
                slug: "entity/腾讯".into(),
                title: "腾讯".into(),
                page_type: "entity".into(),
                aliases: vec!["腾讯控股".into()],
            },
            PageSurface {
                slug: "concept/rag".into(),
                title: "RAG".into(),
                page_type: "concept".into(),
                aliases: vec![],
            },
        ];
        let out = available_pages(&surfaces, 0);
        assert!(!out.contains("index"));
        assert!(out.contains("entity/腾讯 = 腾讯 (别名: 腾讯控股)"));
        assert!(out.contains("concept/rag = RAG"));
        assert_eq!(available_pages(&surfaces, 1).lines().count(), 1);
        assert_eq!(available_pages(&[], 0), "(暂无可链接页面)");
    }

    #[test]
    fn render_evidence_respects_budget_with_whole_chunks() {
        let chunks: Vec<EvidenceChunk> = (0..5)
            .map(|i| EvidenceChunk { handle: format!("c{:03}", i), slice_id: i, content: "证据内容".repeat(50) })
            .collect();
        let full = render_evidence(&chunks, 0);
        assert_eq!(full.matches("<c id=").count(), 5);

        let tight = render_evidence(&chunks, 300);
        assert!(tight.matches("<c id=").count() < 5);
        assert!(tight.contains("未纳入"));
        // 至少保留一条，且被保留的证据是完整的。
        assert!(tight.contains("</c>"));
        assert_eq!(render_evidence(&[], 0), "(无可用原文证据)");
    }

    #[test]
    fn split_summary_line_handles_both_colons_and_missing_prefix() {
        assert_eq!(split_summary_line("SUMMARY: 一句话\n\n正文"), ("一句话".to_string(), "正文".to_string()));
        assert_eq!(split_summary_line("SUMMARY：一句话\n正文"), ("一句话".to_string(), "正文".to_string()));
        let (summary, content) = split_summary_line("没有前缀\n正文");
        assert_eq!(summary, "");
        assert!(content.contains("没有前缀"));
    }

    #[test]
    fn truncate_chars_marks_omission() {
        assert_eq!(truncate_chars("短文本", 10), "短文本");
        let out = truncate_chars("一二三四五六", 3);
        assert!(out.starts_with("一二三"));
        assert!(out.contains("原文共 6 字"));
    }

    #[test]
    fn xml_escape_covers_attribute_breakers() {
        assert_eq!(xml_escape("a<b>&\"c\""), "a&lt;b&gt;&amp;&quot;c&quot;");
    }
}
