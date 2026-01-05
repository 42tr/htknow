use std::collections::HashSet;

use anyhow::Result;
use regex::Regex;

use super::{Entity, Relation, RelationType};

/// 共现关系分析器
pub struct CooccurrenceAnalyzer {
    window_size: usize, // 滑动窗口大小（默认15个词，更小的窗口减少噪音）
    min_weight: f32,    // 最小权重阈值（只保留高置信度的关系）
}

impl CooccurrenceAnalyzer {
    pub fn new(window_size: usize, min_weight: f32) -> Self {
        Self { window_size, min_weight }
    }

    /// 在窗口内发现共现的实体对
    pub fn find_cooccurrences(&self, text: &str, entities: &[Entity]) -> Vec<Relation> {
        use std::collections::HashMap;

        let mut relations = Vec::new();

        // 将文本分词（简单按空白符分割）
        let words: Vec<&str> = text.split_whitespace().collect();

        // 为每个实体找到其在文本中的位置（词索引）
        let mut entity_positions: Vec<(usize, &Entity)> = Vec::new();
        for entity in entities {
            for (i, window) in words.windows(entity.name.split_whitespace().count()).enumerate() {
                let window_text = window.join(" ");
                if window_text.contains(&entity.name) {
                    entity_positions.push((i, entity));
                }
            }
        }

        // 排序以便按位置处理
        entity_positions.sort_by_key(|(pos, _)| *pos);

        // 统计实体对的共现次数和最小距离
        let mut pair_stats: HashMap<(String, String), (usize, usize)> = HashMap::new(); // (count, min_distance)

        for i in 0..entity_positions.len() {
            for j in (i + 1)..entity_positions.len() {
                let (pos1, entity1) = &entity_positions[i];
                let (pos2, entity2) = &entity_positions[j];

                // 检查是否在窗口范围内
                let distance = pos2 - pos1;
                if distance <= self.window_size {
                    let pair = if entity1.name < entity2.name {
                        (entity1.name.clone(), entity2.name.clone())
                    } else {
                        (entity2.name.clone(), entity1.name.clone())
                    };

                    pair_stats
                        .entry(pair)
                        .and_modify(|(count, min_dist)| {
                            *count += 1;
                            *min_dist = (*min_dist).min(distance);
                        })
                        .or_insert((1, distance));
                }
            }
        }

        // 只保留共现次数 >= 2 或距离很近（< 5个词）的实体对
        for ((source, target), (count, min_distance)) in pair_stats {
            // 计算权重：距离越近、共现次数越多权重越高
            let distance_score = 1.0 - (min_distance as f32 / self.window_size as f32);
            let frequency_score = (count as f32).min(5.0) / 5.0; // 最多计5次
            let weight = (distance_score * 0.6 + frequency_score * 0.4).max(0.0).min(1.0);

            // 只保留高质量的共现关系
            if (count >= 2 || min_distance < 5) && weight >= self.min_weight {
                relations.push(
                    Relation::new(source.clone(), target.clone(), RelationType::CoOccurs)
                        .with_weight(weight)
                        .with_property("distance".to_string(), min_distance.to_string())
                        .with_property("count".to_string(), count.to_string()),
                );
            }
        }

        relations
    }
}

impl Default for CooccurrenceAnalyzer {
    fn default() -> Self {
        Self::new(15, 0.6) // 窗口15个词，最小权重0.6
    }
}

/// 关系模式
struct RelationPattern {
    regex: Regex,
    relation_type: RelationType,
}

/// 模式匹配器
pub struct PatternMatcher {
    patterns: Vec<RelationPattern>,
}

impl PatternMatcher {
    pub fn new() -> Result<Self> {
        let patterns = vec![
            // "X是Y的一种" -> IsA关系
            RelationPattern {
                regex: Regex::new(r"([^，。！？]+)是([^，。！？]+)的一种")?,
                relation_type: RelationType::IsA,
            },
            // "X属于Y" -> PartOf关系
            RelationPattern {
                regex: Regex::new(r"([^，。！？]+)属于([^，。！？]+)")?,
                relation_type: RelationType::PartOf,
            },
            // "X包含Y" -> Contains关系
            RelationPattern {
                regex: Regex::new(r"([^，。！？]+)包含([^，。！？]+)")?,
                relation_type: RelationType::Contains,
            },
            // "X依赖Y" -> DependsOn关系
            RelationPattern {
                regex: Regex::new(r"([^，。！？]+)依赖([^，。！？]+)")?,
                relation_type: RelationType::DependsOn,
            },
            // "X在Y之前" -> Before关系
            RelationPattern {
                regex: Regex::new(r"([^，。！？]+)在([^，。！？]+)之前")?,
                relation_type: RelationType::Before,
            },
            // "X在Y之后" -> After关系
            RelationPattern {
                regex: Regex::new(r"([^，。！？]+)在([^，。！？]+)之后")?,
                relation_type: RelationType::After,
            },
        ];

        Ok(Self { patterns })
    }

    /// 使用预定义模式匹配关系
    pub fn match_patterns(&self, text: &str, entities: &[Entity]) -> Vec<Relation> {
        let mut relations = Vec::new();
        let entity_names: HashSet<String> = entities.iter().map(|e| e.name.clone()).collect();

        for pattern in &self.patterns {
            for cap in pattern.regex.captures_iter(text) {
                if cap.len() >= 3 {
                    let source = cap.get(1).map(|m| m.as_str().trim()).unwrap_or("");
                    let target = cap.get(2).map(|m| m.as_str().trim()).unwrap_or("");

                    // 检查是否是已提取的实体
                    if entity_names.contains(source) && entity_names.contains(target) {
                        relations.push(
                            Relation::new(source.to_string(), target.to_string(), pattern.relation_type.clone())
                                .with_property("source".to_string(), "pattern_matching".to_string()),
                        );
                    }
                }
            }
        }

        relations
    }
}

impl Default for PatternMatcher {
    fn default() -> Self {
        Self::new().expect("Failed to create PatternMatcher")
    }
}

/// 关系提取器（组合多种策略）
pub struct RelationExtractor {
    cooccurrence_analyzer: CooccurrenceAnalyzer,
    pattern_matcher: PatternMatcher,
}

impl RelationExtractor {
    pub fn new() -> Result<Self> {
        Ok(Self { cooccurrence_analyzer: CooccurrenceAnalyzer::default(), pattern_matcher: PatternMatcher::new()? })
    }

    /// 从文本中提取所有关系
    pub fn extract_relations(&self, text: &str, entities: &[Entity]) -> Vec<Relation> {
        let mut relations = Vec::new();

        // 1. 共现关系
        relations.extend(self.cooccurrence_analyzer.find_cooccurrences(text, entities));

        // 2. 模式匹配关系
        relations.extend(self.pattern_matcher.match_patterns(text, entities));

        // 去重
        relations.sort_by(|a, b| {
            (&a.source_name, &a.target_name, a.relation_type.as_str()).cmp(&(
                &b.source_name,
                &b.target_name,
                b.relation_type.as_str(),
            ))
        });
        relations.dedup_by(|a, b| {
            a.source_name == b.source_name
                && a.target_name == b.target_name
                && a.relation_type.as_str() == b.relation_type.as_str()
        });

        relations
    }
}

impl Default for RelationExtractor {
    fn default() -> Self {
        Self::new().expect("Failed to create RelationExtractor")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::EntityType;

    #[test]
    fn test_cooccurrence() {
        let analyzer = CooccurrenceAnalyzer::new(10, 0.5);
        let entities = vec![
            Entity::new("Rust".to_string(), EntityType::Technology),
            Entity::new("Axum".to_string(), EntityType::Technology),
        ];

        let text = "我们使用 Rust 语言和 Axum 框架开发了这个应用 我们使用 Rust 语言和 Axum 框架开发了这个应用";
        let relations = analyzer.find_cooccurrences(text, &entities);

        assert!(relations.len() > 0);
    }

    #[test]
    fn test_pattern_matching() {
        let matcher = PatternMatcher::new().unwrap();
        let entities = vec![
            Entity::new("Rust".to_string(), EntityType::Technology),
            Entity::new("编程语言".to_string(), EntityType::Concept),
        ];

        let text = "Rust是编程语言的一种";
        let relations = matcher.match_patterns(text, &entities);

        assert!(relations.iter().any(|r| matches!(r.relation_type, RelationType::IsA)));
    }
}
