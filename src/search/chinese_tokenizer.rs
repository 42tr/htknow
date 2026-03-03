//! 中文分词器模块 - 提供高性能的中英文混合分词功能

use std::{collections::HashSet, sync::RwLock};

use anyhow::anyhow;
use jieba_rs::{Jieba, TokenizeMode};
use lazy_static::lazy_static;
use log::info;
use tantivy::tokenizer::{Token, TokenStream, Tokenizer};

#[derive(Debug, Clone)]
pub struct LexiconEntry {
    pub term: String,
    pub freq: Option<usize>,
    pub tag: Option<String>,
}

// 使用全局静态 Jieba 实例，避免重复初始化
lazy_static! {
    static ref JIEBA: RwLock<Jieba> = RwLock::new(Jieba::new());
    // 基础停用词集合，可按需扩展
    static ref STOP_WORDS: HashSet<&'static str> = {
        let mut set = HashSet::new();
        let content = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/stopwords.txt"));
        for line in content.lines() {
            let word = line.trim();
            if word.is_empty() || word.starts_with('#') {
                continue;
            }
            set.insert(word);
        }
        set
    };
}

pub fn reload_custom_words(entries: &[LexiconEntry]) -> anyhow::Result<usize> {
    let mut jieba = Jieba::new();
    let mut loaded = 0usize;
    for entry in entries {
        let term = entry.term.trim();
        if term.is_empty() {
            continue;
        }
        let tag = entry.tag.as_deref().map(str::trim).filter(|s| !s.is_empty());
        let freq = entry.freq.filter(|f| *f > 0);
        jieba.add_word(term, freq, tag);
        loaded += 1;
    }

    let mut guard = JIEBA.write().map_err(|_| anyhow!("failed to lock jieba for writing"))?;
    *guard = jieba;
    info!("Reloaded Jieba custom lexicon with {} words", loaded);
    Ok(loaded)
}

/// 分词模式
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SegmentationMode {
    /// 搜索模式：适合搜索引擎（召回率高但较慢）
    Search,
    /// 全模式：速度较慢，但能识别更多的词汇
    All,
}

impl Default for SegmentationMode {
    fn default() -> Self {
        SegmentationMode::Search
    }
}

/// 快速中文分词器 - 针对性能优化
#[derive(Clone)]
pub struct FastChineseTokenizer {
    mode: SegmentationMode,
}

impl FastChineseTokenizer {
    /// 创建新的分词器
    pub fn new(mode: SegmentationMode) -> Self {
        FastChineseTokenizer { mode }
    }
    /// 创建全模式分词器
    pub fn all() -> Self {
        Self::new(SegmentationMode::All)
    }

    /// 执行分词
    pub fn segment(&self, text: &str) -> Vec<String> {
        let jieba = JIEBA.read().unwrap_or_else(|e| e.into_inner());
        let words = match self.mode {
            SegmentationMode::Search => {
                // 搜索模式：使用 cut_for_search，召回率高
                jieba
                    .cut_for_search(text, false)
                    .into_iter()
                    .filter(|s| {
                        let word = s.trim();
                        !word.is_empty() && !STOP_WORDS.contains(word)
                    })
                    .map(|s| s.to_string())
                    .collect()
            }
            SegmentationMode::All => jieba
                .cut_all(text)
                .into_iter()
                .filter(|s| {
                    let word = s.trim();
                    !word.is_empty() && !STOP_WORDS.contains(word)
                })
                .map(|s| s.to_string())
                .collect(),
        };
        info!("Segmented words: {:?}", words);
        words
    }

    fn tokenize_with_offsets(&self, text: &str) -> Vec<TokenInfo> {
        let mode = match self.mode {
            SegmentationMode::Search => TokenizeMode::Search,
            SegmentationMode::All => TokenizeMode::Search,
        };
        let jieba = JIEBA.read().unwrap_or_else(|e| e.into_inner());
        jieba
            .tokenize(text, mode, false)
            .into_iter()
            .filter(|t| {
                let word = t.word.trim();
                !word.is_empty() && !STOP_WORDS.contains(word)
            })
            .map(|t| TokenInfo { text: t.word.to_string(), start: t.start, end: t.end })
            .collect()
    }
}

impl Tokenizer for FastChineseTokenizer {
    type TokenStream<'a> = ChineseTokenStream<'a>;

    fn token_stream<'a>(&'a mut self, text: &'a str) -> Self::TokenStream<'a> {
        let tokens = self.tokenize_with_offsets(text);
        ChineseTokenStream::new(text, tokens)
    }
}

/// Token stream for Chinese text
pub struct ChineseTokenStream<'a> {
    tokens: Vec<TokenInfo>,
    current_index: usize,
    token: Token,
    char_to_byte: Vec<usize>,
    _phantom: std::marker::PhantomData<&'a ()>,
}

struct TokenInfo {
    text: String,
    start: usize,
    end: usize,
}

impl<'a> ChineseTokenStream<'a> {
    fn new(text: &'a str, tokens: Vec<TokenInfo>) -> Self {
        let mut char_to_byte = Vec::with_capacity(text.chars().count() + 1);
        for (byte_idx, _) in text.char_indices() {
            char_to_byte.push(byte_idx);
        }
        char_to_byte.push(text.len());
        ChineseTokenStream {
            tokens,
            current_index: 0,
            token: Token::default(),
            char_to_byte,
            _phantom: std::marker::PhantomData,
        }
    }
}

impl<'a> TokenStream for ChineseTokenStream<'a> {
    fn advance(&mut self) -> bool {
        while self.current_index < self.tokens.len() {
            let token_info = &self.tokens[self.current_index];
            self.current_index += 1;

            // 跳过空的token
            if token_info.text.trim().is_empty() {
                continue;
            }

            let char_len = self.char_to_byte.len().saturating_sub(1);
            if token_info.start >= char_len || token_info.end > char_len || token_info.start >= token_info.end {
                continue;
            }
            let start = self.char_to_byte[token_info.start];
            let end = self.char_to_byte[token_info.end];

            self.token.text.clear();
            self.token.text.push_str(&token_info.text);
            self.token.offset_from = start;
            self.token.offset_to = end;
            self.token.position = self.token.position.wrapping_add(1);
            return true;
        }
        false
    }

    fn token(&self) -> &Token {
        &self.token
    }

    fn token_mut(&mut self) -> &mut Token {
        &mut self.token
    }
}
