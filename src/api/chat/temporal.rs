//! Apply only explicit validity metadata from visible original sources.
use super::{
    ChatSource,
    context::{QuestionContext, SourceScope},
};
use anyhow::{Result, ensure};
use chrono::NaiveDate;
use serde_json::{Value, json};

pub(super) fn dates(text: &str) -> Vec<NaiveDate> {
    static DATE: once_cell::sync::Lazy<regex::Regex> = once_cell::sync::Lazy::new(|| {
        regex::Regex::new(r"(\d{4})(?:年|[-/])(\d{1,2})(?:月|[-/])(\d{1,2})日?").unwrap()
    });
    DATE.captures_iter(text)
        .filter_map(|c| NaiveDate::from_ymd_opt(c[1].parse().ok()?, c[2].parse().ok()?, c[3].parse().ok()?))
        .collect()
}

fn requested_on(context: &QuestionContext) -> Option<NaiveDate> {
    if context.source_scope == SourceScope::Wiki
        || ["比较", "对比", "差异", "区别", "各自"].iter().any(|w| context.question.contains(w))
    {
        return None;
    }
    if ["生效日期", "审批日期", "何时生效"].iter().any(|w| context.question.contains(w))
        && !requirement(&context.question)
    {
        return None;
    }
    let scope = context.evidence_scope();
    let requested =
        scope["requested_dates"].as_array()?.iter().filter_map(Value::as_str).flat_map(dates).collect::<Vec<_>>();
    match requested.as_slice() {
        [on] => Some(*on),
        _ => None,
    }
}

fn requirement(text: &str) -> bool {
    ["适用", "时限", "要求", "门槛", "阈值", "准入", "限额", "上限", "下限", "必须", "至少", "不超过"]
        .iter()
        .any(|w| text.contains(w))
}

struct Validity {
    from: Option<NaiveDate>,
    until: Option<NaiveDate>,
    draft: bool,
}
fn validity(source: &ChatSource) -> Option<Validity> {
    if source.result.wiki.is_some() {
        return None;
    }
    let content = &source.result.content;
    static FROM: once_cell::sync::Lazy<regex::Regex> =
        once_cell::sync::Lazy::new(|| regex::Regex::new(r"(?:^|[\n。])\s*生效日期\s*[:：]\s*([^。\n]+)").unwrap());
    static UNTIL: once_cell::sync::Lazy<regex::Regex> =
        once_cell::sync::Lazy::new(|| regex::Regex::new(r"有效期(?:截至|截止|至)\s*[:：]?\s*([^。\n]+)").unwrap());
    let from = FROM.captures(content).and_then(|c| dates(&c[1]).into_iter().next());
    let until = UNTIL.captures(content).and_then(|c| dates(&c[1]).into_iter().next());
    let header = content.lines().next().unwrap_or("");
    let draft = ["待审批", "未审批", "草案"].iter().any(|w| header.contains(w));
    (from.is_some() || until.is_some() || draft).then_some(Validity { from, until, draft })
}
impl Validity {
    fn contains(&self, on: NaiveDate) -> bool {
        !self.draft && self.from.is_none_or(|from| on >= from) && self.until.is_none_or(|until| on <= until)
    }
}

pub(super) fn scope(context: &QuestionContext, sources: &[ChatSource]) -> Value {
    let Some(on) = requested_on(context) else { return Value::Null };
    json!({"requested_on":on.to_string(),"original_source_validity":sources.iter().filter_map(|s|{
        validity(s).map(|v|json!({"source_id":s.id,"valid_from":v.from.map(|d|d.to_string()),"valid_until_inclusive":v.until.map(|d|d.to_string()),
            "unapproved_draft":v.draft,"applicable_on_requested_date":v.contains(on)}))
    }).collect::<Vec<_>>(),"rule":"日期边界仅来自本轮原文明示的生效日期与有效期，审批日期不等于生效日期。历史归档在当时有效期内仍适用。只用适用于请求日期的版本给出该日要求，未生效版本可用于明确的元数据说明，不能用于该日的时限或阈值。"})
}

pub(super) fn validate(text: &str, source: &ChatSource, context: &QuestionContext) -> Result<()> {
    let Some(on) = requested_on(context) else { return Ok(()) };
    // These statements describe chronology, rather than applying a rule.
    static NOT_EFFECTIVE: once_cell::sync::Lazy<regex::Regex> = once_cell::sync::Lazy::new(|| {
        regex::Regex::new(r"(?:尚未|还未|未|没有|并未)(?:正式|审批并|获批并|批准并)?生效").unwrap()
    });
    if ((text.contains("生效日期") || text.contains("审批日期")) && !requirement(text))
        || NOT_EFFECTIVE.is_match(text)
        || ["未审批", "待审批", "未批准"].iter().any(|w| text.contains(w))
        || ["尚未生效", "未生效", "不适用", "不能适用", "不应适用"].iter().any(|w| text.contains(w))
    {
        return Ok(());
    }
    if let Some(v) = validity(source) {
        ensure!(
            v.contains(on),
            "日期范围不匹配：问题指定{on}，来源{}的生效日期为{:?}、有效期截至{:?}、未审批草案={}。不能用该版本回答这一天的要求；请从本轮资料选择当日有效版本，不能把审批日当生效日。",
            source.id,
            v.from,
            v.until,
            v.draft
        );
    }
    Ok(())
}

pub(super) fn date_fact_candidates(text: &str, selected: &[String]) -> bool {
    let claimed = dates(text);
    !claimed.is_empty() && claimed.iter().all(|date| selected.iter().any(|p| dates(p).contains(date)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::chat::ChatRequest;
    fn source(id: usize, text: &str) -> ChatSource {
        ChatSource {
            id,
            result: crate::api::search::SearchResultItem {
                id: id as i64,
                file_id: 1,
                score: 1.0,
                content: text.into(),
                file: None,
                kb: None,
                image_filename: None,
                image_content: None,
                wiki: None,
            },
        }
    }
    fn context(question: &str) -> QuestionContext {
        QuestionContext::new(&ChatRequest { question: question.into(), kb_id: None, messages: vec![] })
    }
    #[test]
    fn historical_validity_uses_effective_dates_and_inclusive_boundaries_not_approval_or_archive_status() {
        let old =
            source(1, "项目规则V1（历史归档）\n生效日期：2028年4月1日。有效期截至2028年5月31日。\n普通回滚30分钟。");
        let new =
            source(2, "项目规则V2（已审批）\n审批日期：2028年5月28日。生效日期：2028年6月1日。\n普通回滚15分钟。");
        for on in ["2028年5月20日", "2028-05-29", "2028/5/31"] {
            let c = context(&format!("查{on}适用的普通回滚时限"));
            assert!(validate("普通回滚30分钟。", &old, &c).is_ok());
            assert!(validate("普通回滚15分钟。", &new, &c).is_err());
        }
        let c = context("查2028年6月1日适用的普通回滚时限");
        assert!(validate("普通回滚30分钟。", &old, &c).is_err());
        assert!(validate("普通回滚15分钟。", &new, &c).is_ok());
        assert!(
            validate(
                "根据生效日期，2028年5月20日回滚时限为15分钟。",
                &new,
                &context("查2028年5月20日适用的普通回滚时限，请核对生效日期")
            )
            .is_err()
        );
    }
    #[test]
    fn draft_and_metadata_scopes_are_distinct_and_invalid_calendar_dates_are_not_guessed() {
        let draft =
            source(3, "项目规则V3（待审批草案）\n草案发布时间：2028年6月18日。审批状态：未审批；没有生效日期。");
        assert!(validate("回滚8分钟。", &draft, &context("2028年6月20日适用什么回滚时限")).is_err());
        assert!(validate("V3尚未生效。", &draft, &context("2028年6月20日适用什么回滚时限")).is_ok());
        assert!(validate("V3尚未正式生效。", &draft, &context("2028年6月20日适用什么回滚时限")).is_ok());
        assert!(scope(&context("比较2028年5月20日V1和V2的差异"), &[draft]).is_null());
        assert!(dates("2028年2月30日").is_empty());
        assert!(date_fact_candidates("生效日期为2028年6月1日", &["生效日期：2028-06-01".into()]));
        assert!(!date_fact_candidates("生效日期为2028年6月1日", &["验收记录日期：2028年6月20日".into()]));
    }
}
