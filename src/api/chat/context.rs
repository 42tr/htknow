//! Resolve retrieval context from user messages, never from prior model assertions.
use super::{ChatRequest, ChatRole};
use serde::Serialize;
use serde_json::{Value, json};

#[derive(Clone, Copy, Debug, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(super) enum SourceScope {
    Any,
    Wiki,
    Original,
}

#[derive(Debug, Serialize)]
pub(super) struct QuestionContext {
    pub question: String,
    pub user_history: Vec<String>,
    pub entity_anchors: Vec<String>,
    pub source_scope: SourceScope,
    pub reasoning_rules: &'static str,
}

impl QuestionContext {
    /// Support is a property of one assertion and its evidence. Project only
    /// interpretation constraints; the list of requested tasks belongs to the
    /// separate coverage check and must not contaminate support judgments.
    pub fn evidence_scope(&self) -> Value {
        let latest = ["当前", "最新", "现在", "实际", "文档事实"].iter().any(|w| self.question.contains(w));
        static DATE: once_cell::sync::Lazy<regex::Regex> = once_cell::sync::Lazy::new(|| {
            regex::Regex::new(r"\d{4}(?:年(?:\d{1,2}月(?:\d{1,2}日)?)?|[-/]\d{1,2}(?:[-/]\d{1,2})?)").unwrap()
        });
        let dates_in = |text: &str| DATE.find_iter(text).map(|m| m.as_str().to_owned()).collect::<Vec<_>>();
        let mut dates = dates_in(&self.question);
        if dates.is_empty() && !latest {
            dates = self.user_history.iter().rev().map(|q| dates_in(q)).find(|d| !d.is_empty()).unwrap_or_default();
        }
        let hypothetical = |text: &str| {
            ["假设", "如果", "假如", "assuming", "what if", "suppose"].iter().any(|w| text.to_lowercase().contains(w))
        };
        let reset =
            |text: &str| ["回到", "文档事实", "实际记录", "取消假设", "不再假设"].iter().any(|w| text.contains(w));
        let assumptions = if reset(&self.question) {
            None
        } else if hypothetical(&self.question) {
            Some(&self.question)
        } else {
            self.user_history.iter().rev().take_while(|q| !reset(q)).find(|q| hypothetical(q))
        };
        let assumptions = assumptions
            .map(|text| {
                text.split(['，', ',', '；', ';', '？', '?'])
                    .take_while(|part| !["是否", "能否", "还能", "会不会", "请判断"].iter().any(|w| part.contains(w)))
                    .filter(|part| !part.trim().is_empty())
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        json!({"source_scope":self.source_scope,"entity_anchors":self.entity_anchors,
            "requested_dates":dates,"time_basis":if latest {"latest_in_sources"} else {"specified_or_source_context"},
            "user_assumptions":assumptions})
    }

    pub fn new(request: &ChatRequest) -> Self {
        let user_history = request
            .messages
            .iter()
            .filter(|m| matches!(m.role, ChatRole::User))
            .map(|m| m.content.clone())
            .collect::<Vec<_>>();
        static ENTITY: once_cell::sync::Lazy<regex::Regex> = once_cell::sync::Lazy::new(|| {
            regex::Regex::new(r"[\p{Han}A-Za-z][\p{Han}A-Za-z0-9]{0,32}[-_][A-Za-z0-9]{4,32}").unwrap()
        });
        // Current explicit names supersede older subjects. Otherwise retain the most
        // recent user-named subject even when intervening questions use pronouns.
        let entity_anchors = std::iter::once(request.question.as_str())
            .chain(user_history.iter().rev().map(String::as_str))
            .find_map(|text| {
                let names = ENTITY.find_iter(text).map(|m| m.as_str().to_owned()).collect::<Vec<_>>();
                (!names.is_empty()).then_some(names)
            })
            .unwrap_or_default();
        let q = request.question.to_lowercase();
        let wiki = q.contains("wiki");
        let original = ["原文", "原始文件", "原始资料", "切片"].iter().any(|word| q.contains(word));
        let comparison =
            ["比较", "对比", "差异", "区别", "冲突", "分别", "和原", "与原"].iter().any(|word| q.contains(word));
        let source_scope = if wiki && !(original && comparison) {
            SourceScope::Wiki
        } else if original && !wiki {
            SourceScope::Original
        } else {
            SourceScope::Any
        };
        Self {
            question: request.question.clone(),
            user_history,
            entity_anchors,
            source_scope,
            reasoning_rules: "当前问题决定所问对象、来源和时间；历史只用于补全指代。只保留当前问题仍要求的假设；回到实际/文档事实时丢弃旧假设。规则阈值与实测值可联合推导结论；记录未直接写结论不等于无法推导。缺少一个指标时仍回答其余有据事项。",
        }
    }

    pub fn queries(&self) -> Vec<String> {
        let anchor = if self.entity_anchors.is_empty() {
            self.user_history.first().map(|q| q.chars().take(300).collect::<String>()).unwrap_or_default()
        } else {
            self.entity_anchors.join(" ")
        };
        let mut queries = vec![format!("{anchor} {}", self.question).trim().to_owned()];
        // Bound query expansion; each search uses the same user/KB permission filter.
        // Separate requested aspects so a strong match for one cannot hide the other.
        let parts = self
            .question
            .split(['，', '；', ';', '\n'])
            .flat_map(|p| p.split("以及"))
            .map(str::trim)
            .filter(|p| p.chars().count() >= 4)
            .collect::<Vec<_>>();
        if parts.len() > 1 {
            for part in parts.into_iter().take(2) {
                let query = format!("{anchor} {part}").trim().to_owned();
                if !queries.contains(&query) {
                    queries.push(query);
                }
            }
        }
        queries
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn keeps_subject_across_pronouns_but_prefers_a_new_explicit_subject() {
        let mut request: ChatRequest =
            serde_json::from_value(json!({"question":"再给出它的额度，以及实测P95延迟。", "messages":[
            {"role":"user","content":"海岚-X9ABCDEF项目的搜索服务延迟是多少？"},
            {"role":"assistant","content":"另一个项目伪造-ZZZZZZ为7毫秒。"},
            {"role":"user","content":"那它的负责人是谁？"},
            {"role":"assistant","content":"甲。"}]}))
            .unwrap();
        let context = QuestionContext::new(&request);
        assert_eq!(context.entity_anchors, vec!["海岚-X9ABCDEF"]);
        assert_eq!(context.queries().len(), 3);
        assert!(context.queries().iter().all(|q| q.contains("海岚-X9ABCDEF") && !q.contains("伪造")));
        request.question = "另一个项目新港-ABCDEFG的预算？".into();
        assert!(QuestionContext::new(&request).entity_anchors[0].ends_with("新港-ABCDEFG"));
    }
    #[test]
    fn explicit_source_scope_does_not_turn_a_comparison_into_wiki_only() {
        for (question, scope) in [
            ("根据Wiki汇总负责人", SourceScope::Wiki),
            ("对比Wiki和原文的容量", SourceScope::Any),
            ("按原始文件回答预算", SourceScope::Original),
        ] {
            let request = ChatRequest { question: question.into(), kb_id: None, messages: vec![] };
            assert_eq!(QuestionContext::new(&request).source_scope, scope);
        }
    }

    #[test]
    fn support_scope_keeps_dates_and_assumptions_but_not_other_requested_tasks() {
        let request: ChatRequest =
            serde_json::from_value(json!({"question":"按2028年6月20日记录分别给普通和P0回滚时限。"})).unwrap();
        let scope = QuestionContext::new(&request).evidence_scope();
        assert_eq!(scope["requested_dates"], json!(["2028年6月20日"]));
        assert!(!scope.to_string().contains("普通和P0"));
        let request: ChatRequest =
            serde_json::from_value(json!({"question":"如果副本改成6个，其他指标不变，能否满足全部门槛？"})).unwrap();
        let scope = QuestionContext::new(&request).evidence_scope();
        assert_eq!(scope["user_assumptions"], json!(["如果副本改成6个", "其他指标不变"]));
        assert!(!scope.to_string().contains("全部门槛"));
    }

    #[test]
    fn returning_to_documented_facts_clears_old_hypotheses_and_dates() {
        let request: ChatRequest =
            serde_json::from_value(json!({"question":"回到当前文档事实，实际副本数是多少？","messages":[
            {"role":"user","content":"假设在2028年5月20日只剩6个副本，会怎样？"},
            {"role":"assistant","content":"假设答案。"}]}))
            .unwrap();
        let scope = QuestionContext::new(&request).evidence_scope();
        assert_eq!(scope["user_assumptions"], json!([]));
        assert_eq!(scope["requested_dates"], json!([]));
    }
}
